//! Build-time public image optimization with original-file fallbacks.

use std::fs;
use std::path::{Path, PathBuf};

use image::codecs::avif::AvifEncoder;
use image::codecs::webp::WebPEncoder;
use image::{ExtendedColorType, GenericImageView, ImageEncoder};
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImageOptimizationOptions {
    pub optimize: bool,
    pub formats: Vec<String>,
    pub quality: u8,
}

impl Default for ImageOptimizationOptions {
    fn default() -> Self {
        Self {
            optimize: true,
            formats: vec!["avif".to_string(), "webp".to_string()],
            quality: 80,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageOptimizationReport {
    pub optimized_images: usize,
    pub generated_variants: usize,
    pub source_bytes: u64,
    pub generated_bytes: u64,
    pub entries: Vec<ImageManifestEntry>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageManifestEntry {
    pub source: String,
    pub width: u32,
    pub height: u32,
    pub source_bytes: u64,
    pub variants: Vec<ImageVariant>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageVariant {
    pub format: String,
    pub path: String,
    pub bytes: u64,
}

/// Generate AVIF/WebP sidecars next to copied public images.
///
/// The original file is never modified or removed. Unsupported or malformed
/// images are left untouched so enabling optimization cannot break an existing
/// public asset directory.
pub fn optimize_public_images(
    assets_dir: &Path,
    options: &ImageOptimizationOptions,
) -> anyhow::Result<ImageOptimizationReport> {
    let mut report = ImageOptimizationReport::default();
    if !options.optimize || !assets_dir.exists() {
        write_manifest(assets_dir, &report)?;
        return Ok(report);
    }

    let formats = normalized_formats(&options.formats);
    let sources = WalkDir::new(assets_dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .filter(|path| is_optimizable_source(path))
        .collect::<Vec<_>>();

    for source in sources {
        let Ok(decoded) = image::open(&source) else {
            continue;
        };
        let source_bytes = fs::metadata(&source)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let (width, height) = decoded.dimensions();
        let rgba = decoded.to_rgba8();
        let mut variants = Vec::new();

        for format in &formats {
            let output = sidecar_path(&source, format);
            let mut encoded = Vec::new();
            let result = match format.as_str() {
                "avif" => AvifEncoder::new_with_speed_quality(
                    &mut encoded,
                    7,
                    options.quality.clamp(1, 100),
                )
                .write_image(&rgba, width, height, ExtendedColorType::Rgba8),
                "webp" => WebPEncoder::new_lossless(&mut encoded).write_image(
                    &rgba,
                    width,
                    height,
                    ExtendedColorType::Rgba8,
                ),
                _ => continue,
            };
            if result.is_err() {
                continue;
            }
            fs::write(&output, &encoded)?;
            let relative = relative_url(assets_dir, &output);
            variants.push(ImageVariant {
                format: format.clone(),
                path: relative,
                bytes: encoded.len() as u64,
            });
            report.generated_variants += 1;
            report.generated_bytes += encoded.len() as u64;
        }

        if !variants.is_empty() {
            report.optimized_images += 1;
            report.source_bytes += source_bytes;
            report.entries.push(ImageManifestEntry {
                source: relative_url(assets_dir, &source),
                width,
                height,
                source_bytes,
                variants,
            });
        }
    }

    report
        .entries
        .sort_by(|left, right| left.source.cmp(&right.source));
    write_manifest(assets_dir, &report)?;
    Ok(report)
}

fn write_manifest(assets_dir: &Path, report: &ImageOptimizationReport) -> anyhow::Result<()> {
    if assets_dir.exists() {
        fs::write(
            assets_dir.join(".ruvyxa-images.json"),
            serde_json::to_string_pretty(report)?,
        )?;
    }
    Ok(())
}

fn normalized_formats(formats: &[String]) -> Vec<String> {
    let mut output = formats
        .iter()
        .map(|format| format.trim().to_ascii_lowercase())
        .filter(|format| matches!(format.as_str(), "avif" | "webp"))
        .collect::<Vec<_>>();
    output.sort();
    output.dedup();
    output
}

fn is_optimizable_source(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg")
    )
}

fn sidecar_path(source: &Path, format: &str) -> PathBuf {
    let mut name = source
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_default();
    name.push(format!(".{format}"));
    source.with_file_name(name)
}

fn relative_url(root: &Path, path: &Path) -> String {
    format!(
        "/{}",
        path.strip_prefix(root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn emits_avif_webp_and_manifest_without_replacing_source() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("hero.png");
        let image = ImageBuffer::from_pixel(4, 3, Rgba([20u8, 40, 60, 255]));
        image.save(&source).unwrap();

        let report =
            optimize_public_images(temp.path(), &ImageOptimizationOptions::default()).unwrap();

        assert!(source.exists());
        assert!(temp.path().join("hero.png.avif").is_file());
        assert!(temp.path().join("hero.png.webp").is_file());
        assert!(temp.path().join(".ruvyxa-images.json").is_file());
        assert_eq!(report.optimized_images, 1);
        assert_eq!(report.generated_variants, 2);
        assert_eq!(report.entries[0].width, 4);
        assert_eq!(report.entries[0].height, 3);
    }

    #[test]
    fn disabled_optimization_preserves_assets_and_writes_empty_manifest() {
        let temp = tempfile::tempdir().unwrap();
        let options = ImageOptimizationOptions {
            optimize: false,
            ..ImageOptimizationOptions::default()
        };
        let report = optimize_public_images(temp.path(), &options).unwrap();
        assert_eq!(report.optimized_images, 0);
        assert!(temp.path().join(".ruvyxa-images.json").is_file());
    }
}
