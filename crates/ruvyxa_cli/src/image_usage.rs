//! Detects images that bypass the build's image pipeline.
//!
//! The optimizer converts every public PNG/JPEG to one WebP by default, but a
//! plain `<img src="/logo.png">` keeps pointing at the source extension. Without
//! `keepOriginal`, that URL is absent on static hosts; with it, the page ships
//! the larger original instead of using the converted output.
//!
//! This scanner names those references so the author can switch them to
//! `<Image>`. It reports, never fails: a raw `<img>` is legal, and some are
//! deliberate.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::image_optimizer::ImageManifestEntry;

/// Source extensions that can contain markup worth scanning.
const SOURCE_EXTENSIONS: [&str; 6] = ["tsx", "jsx", "ts", "js", "mdx", "md"];

/// Ignore savings below this. A few hundred bytes is not worth a build warning.
const MINIMUM_INTERESTING_SAVING: u64 = 8 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawImageUsage {
    /// Source file containing the `<img>`.
    pub file: PathBuf,
    /// 1-based line of the `<img>` tag.
    pub line: u32,
    /// Public URL as written in the source.
    pub url: String,
    /// Bytes the original costs.
    pub source_bytes: u64,
    /// Bytes the generated WebP costs.
    pub webp_bytes: u64,
}

impl RawImageUsage {
    pub fn saved_bytes(&self) -> u64 {
        self.source_bytes.saturating_sub(self.webp_bytes)
    }
}

/// Find raw `<img>` tags pointing at public images the build already optimized.
///
/// Only references with a worthwhile saving are returned, sorted by saving so
/// the loudest offender is reported first.
pub fn scan_raw_image_usage(app_dir: &Path, entries: &[ImageManifestEntry]) -> Vec<RawImageUsage> {
    if entries.is_empty() {
        return Vec::new();
    }
    let optimized: HashMap<String, &ImageManifestEntry> = entries
        .iter()
        .map(|entry| (public_url(&entry.source), entry))
        .collect();

    let mut findings = Vec::new();
    for file in source_files(app_dir) {
        let Ok(source) = fs::read_to_string(&file) else {
            continue;
        };
        for (line, url) in raw_image_urls(&source) {
            let Some(entry) = optimized.get(&url) else {
                continue;
            };
            let usage = RawImageUsage {
                file: file.clone(),
                line,
                url,
                source_bytes: entry.source_bytes,
                webp_bytes: entry.output_bytes,
            };
            if usage.saved_bytes() >= MINIMUM_INTERESTING_SAVING {
                findings.push(usage);
            }
        }
    }

    findings.sort_by(|left, right| {
        right
            .saved_bytes()
            .cmp(&left.saved_bytes())
            .then_with(|| left.file.cmp(&right.file))
            .then_with(|| left.line.cmp(&right.line))
    });
    findings
}

fn source_files(app_dir: &Path) -> Vec<PathBuf> {
    WalkDir::new(app_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .is_some_and(|extension| SOURCE_EXTENSIONS.contains(&extension.as_str()))
        })
        .collect()
}

/// Public URL for an image manifest source path such as `public/logo.png`.
fn public_url(source: &str) -> String {
    let normalized = source.replace('\\', "/");
    let relative = normalized
        .strip_prefix("public/")
        .unwrap_or_else(|| normalized.trim_start_matches('/'));
    format!("/{relative}")
}

/// Extract `(line, url)` for every root-relative `src` on a lowercase `<img` tag.
///
/// Deliberately literal: `<Image>` is a component and starts with a capital I,
/// so matching the lowercase tag name alone separates the two. A `src` built
/// from an expression has no string literal to read and is skipped rather than
/// guessed at.
fn raw_image_urls(source: &str) -> Vec<(u32, String)> {
    // Which bytes are code is decided by the workspace's scanner, not by a
    // third hand-rolled walk. This loop had no awareness of comments, string
    // literals, template literals or escapes, so a commented-out `<img>` or one
    // inside a string was reported as a real one. That is a small cost while the
    // consequence is a build advisory -- which is why it stayed -- but it is the
    // fifth-plus instance of a pattern this repository has a written rule
    // against, and the next edit to give this scan a harder consequence would
    // have inherited the blindness silently.
    //
    // The mask locates, the source reads. `masked_code` blanks a JSX attribute's
    // quoted value along with everything else that is data rather than code, so
    // the value this function exists to extract is not in it. Both strings have
    // identical byte offsets and identical line boundaries -- the masker blanks
    // to the same length and keeps every newline -- so a position found in one
    // indexes the other.
    let masked = ruvyxa_bundler::ast::masked_code(source);
    let mut found = Vec::new();
    for (index, (line, masked_line)) in source.lines().zip(masked.lines()).enumerate() {
        let mut cursor = 0;
        while let Some(offset) = masked_line[cursor..].find("<img") {
            let start = cursor + offset;
            let after = masked_line[start + 4..].chars().next();
            // `<img` must be the whole tag name: `<imgur>` is not an image.
            if after.is_some_and(|char| char.is_alphanumeric() || char == '-') {
                cursor = start + 4;
                continue;
            }
            // The tag may wrap across lines; attributes on later lines are not
            // read. A `src` written on the same line as the tag is the common
            // shape and the only one worth reporting without a real parser.
            // Read from the source: the tag was located in the mask, but the
            // attribute value lives only in the original.
            let tag = &line[start..];
            if let Some(url) = attribute_value(tag, "src").filter(|url| url.starts_with('/')) {
                found.push((index as u32 + 1, url));
            }
            cursor = start + 4;
        }
    }
    found
}

/// Value of a quoted attribute, or `None` when it is absent or an expression.
fn attribute_value(tag: &str, name: &str) -> Option<String> {
    let mut cursor = 0;
    while let Some(offset) = tag[cursor..].find(name) {
        let start = cursor + offset;
        cursor = start + name.len();
        // Must be a standalone attribute name preceded by whitespace.
        if !tag[..start].ends_with(char::is_whitespace) {
            continue;
        }
        let rest = tag[cursor..].trim_start();
        let Some(rest) = rest.strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim_start();
        let quote = rest.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let value = &rest[1..];
        let end = value.find(quote)?;
        return Some(value[..end].to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(source: &str, source_bytes: u64, output_bytes: u64) -> ImageManifestEntry {
        ImageManifestEntry {
            source: source.to_string(),
            output: source.replace(".png", ".webp"),
            width: 1024,
            height: 1024,
            source_bytes,
            output_bytes,
            cache_hit: false,
            variants: Vec::new(),
        }
    }

    /// An `<img>` that is not code is not a bypassed image.
    ///
    /// The scan used to walk raw lines, so a commented-out tag and one inside a
    /// string literal were both reported. The advisory then told a developer to
    /// optimise an image that no page renders.
    #[test]
    fn an_img_outside_code_is_not_reported() {
        let source = concat!(
            "// <img src=\"/commented.png\" />\n",
            "/* <img src=\"/blocked.png\" /> */\n",
            "const sample = '<img src=\"/single.png\" />'\n",
            "const template = `<img src=\"/template.png\" />`\n",
            "export const real = <img src=\"/real.png\" />\n",
        );
        let found = raw_image_urls(source);
        assert_eq!(
            found,
            vec![(5, "/real.png".to_string())],
            "only the tag that is code counts; found {found:?}",
        );
    }

    /// A tag that follows real code on the same line is still found.
    ///
    /// The mask blanks what is not code and keeps every other byte in place, so
    /// this is the case that would break if it ever stopped preserving offsets.
    #[test]
    fn a_tag_after_a_comment_on_the_same_line_is_still_found() {
        let source = "export const x = /* pick one */ <img src=\"/after.png\" />\n";
        assert_eq!(raw_image_urls(source), vec![(1, "/after.png".to_string())]);
    }

    #[test]
    fn reads_a_root_relative_src_from_a_lowercase_img_tag() {
        let urls = raw_image_urls(
            r#"<img src="/logo.png" alt="Logo" />
<img alt="Hero" src='/hero.jpg'>
<Image src="/ignored.png" alt="" width={1} height={1} />
<img src={dynamic} alt="" />
<img src="https://cdn.example/logo.png" alt="" />
<imgur src="/nope.png">"#,
        );

        assert_eq!(
            urls,
            vec![
                (1, "/logo.png".to_string()),
                (2, "/hero.jpg".to_string()),
                // Line 6's `<imgur>` is a different tag; the rest are not
                // root-relative literals on a raw `<img>`.
            ]
        );
    }

    #[test]
    fn maps_manifest_sources_to_public_urls() {
        assert_eq!(public_url("public/logo.png"), "/logo.png");
        assert_eq!(public_url("public\\nested\\logo.png"), "/nested/logo.png");
        assert_eq!(public_url("/logo.png"), "/logo.png");
    }

    #[test]
    fn reports_bypassed_images_worst_first() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        fs::create_dir_all(app.join("about")).unwrap();
        fs::write(
            app.join("page.tsx"),
            "export default () => <img src=\"/hero.png\" alt=\"Hero\" />\n",
        )
        .unwrap();
        fs::write(
            app.join("about").join("page.tsx"),
            "export default () => <img src=\"/logo.png\" alt=\"Logo\" />\n",
        )
        .unwrap();

        let findings = scan_raw_image_usage(
            &app,
            &[
                entry("public/logo.png", 60_000, 20_000),
                entry("public/hero.png", 400_000, 90_000),
            ],
        );

        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].url, "/hero.png");
        assert_eq!(findings[0].saved_bytes(), 310_000);
        assert_eq!(findings[1].url, "/logo.png");
    }

    #[test]
    fn stays_quiet_for_small_savings_and_unoptimized_urls() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            "export default () => (<><img src=\"/tiny.png\" alt=\"\" /><img src=\"/icon.svg\" alt=\"\" /></>)\n",
        )
        .unwrap();

        // A WebP that saves almost nothing is not worth a build warning, and an
        // SVG never entered the optimizer at all.
        let findings = scan_raw_image_usage(&app, &[entry("public/tiny.png", 9_000, 8_000)]);
        assert!(findings.is_empty(), "{findings:?}");
    }

    #[test]
    fn ignores_projects_with_no_optimized_images() {
        let dir = tempfile::tempdir().unwrap();
        assert!(scan_raw_image_usage(dir.path(), &[]).is_empty());
    }
}
