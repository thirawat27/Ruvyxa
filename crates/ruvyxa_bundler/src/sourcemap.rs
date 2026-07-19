//! Source Map v3 generator for the Ruvyxa bundler.
//!
//! Produces a standard [Source Map v3](https://sourcemaps.info/spec.html) JSON
//! string that maps bundled output positions back to original source files.
//!
//! ## Design
//!
//! Each stage of the pipeline (compiler, linker, output wrapper) appends
//! mappings via [`SourceMapBuilder`].  The builder tracks:
//!
//! - `sources`: ordered list of original file paths
//! - `sourcesContent`: inline original source text (optional)
//! - `mappings`: VLQ-encoded segment data
//!
//! The final `.map` JSON is emitted by [`SourceMapBuilder::to_json`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A single mapping segment: one generated position → one original position.
#[derive(Debug, Clone)]
pub struct Mapping {
    /// 0-based line in the generated output.
    pub gen_line: u32,
    /// 0-based column in the generated output.
    pub gen_col: u32,
    /// Index into the `sources` array.
    pub source_idx: u32,
    /// 0-based line in the original source.
    pub orig_line: u32,
    /// 0-based column in the original source.
    pub orig_col: u32,
}

/// Builder that accumulates mappings and source entries, then serializes
/// the final source map JSON.
#[derive(Debug, Clone)]
pub struct SourceMapBuilder {
    /// The output filename this map corresponds to.
    file: String,
    /// Ordered source file paths (relative to source root).
    sources: Vec<String>,
    /// Inline source content per source (parallel to `sources`).
    sources_content: Vec<Option<String>>,
    /// All mapping segments, in generation order.
    mappings: Vec<Mapping>,
    /// Project root used to relativize paths.
    source_root: PathBuf,
    /// Symbol name table (for the optional `names` array).
    names: Vec<String>,
    /// Indices into `sources` that belong to generated/framework code and
    /// should be ignored by debuggers (`x_google_ignoreList`).
    ignore_list: Vec<u32>,
}

impl SourceMapBuilder {
    /// Create a new builder for the given output file.
    pub fn new(file: impl Into<String>, source_root: impl Into<PathBuf>) -> Self {
        Self {
            file: file.into(),
            sources: Vec::new(),
            sources_content: Vec::new(),
            mappings: Vec::new(),
            source_root: source_root.into(),
            names: Vec::new(),
            ignore_list: Vec::new(),
        }
    }

    /// Register a symbol name in the `names` table and return its index.
    ///
    /// The returned index can be used as the optional 5th VLQ segment in a
    /// [`Mapping`] to attach a name to a generated position.
    pub fn add_name(&mut self, name: impl Into<String>) -> u32 {
        let name = name.into();
        if let Some(idx) = self.names.iter().position(|n| *n == name) {
            return idx as u32;
        }
        let idx = self.names.len() as u32;
        self.names.push(name);
        idx
    }

    /// Mark a source index as an "ignore-listed" library file.
    ///
    /// DevTools that support `x_google_ignoreList` will skip these files in
    /// call stacks and source panels, keeping focus on user code.
    pub fn add_to_ignore_list(&mut self, source_idx: u32) {
        if !self.ignore_list.contains(&source_idx) {
            self.ignore_list.push(source_idx);
        }
    }

    /// Register a source file and return its index.
    ///
    /// If `content` is provided it will be inlined in `sourcesContent`.
    pub fn add_source(&mut self, path: &Path, content: Option<&str>) -> u32 {
        let relative = path
            .strip_prefix(&self.source_root)
            .unwrap_or(path)
            .display()
            .to_string()
            .replace('\\', "/");

        if let Some(idx) = self.sources.iter().position(|s| *s == relative) {
            return idx as u32;
        }

        let idx = self.sources.len() as u32;
        self.sources.push(relative);
        self.sources_content.push(content.map(|s| s.to_string()));
        idx
    }

    /// Add a mapping segment.
    pub fn add_mapping(&mut self, mapping: Mapping) {
        self.mappings.push(mapping);
    }

    /// Add identity mappings for a source file being appended at a given
    /// generated line offset.  This maps each line 1:1 from the source.
    pub fn add_identity_mappings(&mut self, source_idx: u32, source: &str, gen_line_offset: u32) {
        for (line_no, line) in source.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            self.mappings.push(Mapping {
                gen_line: gen_line_offset + line_no as u32,
                gen_col: 0,
                source_idx,
                orig_line: line_no as u32,
                orig_col: 0,
            });
            // Also map the first non-whitespace character
            let leading = line.len() - line.trim_start().len();
            if leading > 0 {
                self.mappings.push(Mapping {
                    gen_line: gen_line_offset + line_no as u32,
                    gen_col: leading as u32,
                    source_idx,
                    orig_line: line_no as u32,
                    orig_col: leading as u32,
                });
            }
        }
    }

    /// Import mappings from a Source Map v3 document at a generated-line offset.
    ///
    /// Returns `false` for malformed or empty maps so callers can retain their
    /// existing identity-mapping fallback when a transform does not provide a map.
    pub fn add_source_map(&mut self, json: &str, gen_line_offset: u32) -> bool {
        let mut candidate = self.clone();
        if !candidate.add_source_map_inner(json, gen_line_offset) {
            return false;
        }
        *self = candidate;
        true
    }

    fn add_source_map_inner(&mut self, json: &str, gen_line_offset: u32) -> bool {
        let Ok(source_map) = serde_json::from_str::<InputSourceMap>(json) else {
            return false;
        };
        if source_map.version != 3 || source_map.sources.is_empty() {
            return false;
        }

        let source_indices = source_map
            .sources
            .iter()
            .enumerate()
            .map(|(index, source)| {
                let path = if source_map.source_root.is_empty() {
                    PathBuf::from(source)
                } else {
                    PathBuf::from(&source_map.source_root).join(source)
                };
                self.add_source(
                    &path,
                    source_map
                        .sources_content
                        .get(index)
                        .and_then(Option::as_deref),
                )
            })
            .collect::<Vec<_>>();

        let mut previous_source = 0i64;
        let mut previous_original_line = 0i64;
        let mut previous_original_column = 0i64;
        let mut imported = Vec::new();

        for (generated_line, line) in source_map.mappings.split(';').enumerate() {
            let mut generated_column = 0i64;
            for segment in line.split(',').filter(|segment| !segment.is_empty()) {
                let Some(fields) = decode_vlq_segment(segment) else {
                    return false;
                };
                if fields.len() < 4 {
                    continue;
                }

                generated_column += fields[0];
                previous_source += fields[1];
                previous_original_line += fields[2];
                previous_original_column += fields[3];
                if generated_column < 0
                    || previous_source < 0
                    || previous_original_line < 0
                    || previous_original_column < 0
                {
                    return false;
                }
                let Some(&source_idx) = source_indices.get(previous_source as usize) else {
                    return false;
                };

                imported.push(Mapping {
                    gen_line: gen_line_offset + generated_line as u32,
                    gen_col: generated_column as u32,
                    source_idx,
                    orig_line: previous_original_line as u32,
                    orig_col: previous_original_column as u32,
                });
            }
        }

        if imported.is_empty() {
            return false;
        }
        self.mappings.extend(imported);
        true
    }

    /// Serialize the source map to a JSON string.
    pub fn to_json(&self) -> String {
        let mappings_str = self.encode_mappings();

        let sources_content: Vec<&str> = self
            .sources_content
            .iter()
            .map(|c| c.as_deref().unwrap_or(""))
            .collect();

        // Build ignore list only if non-empty.
        let ignore_list: Option<Vec<u32>> = if self.ignore_list.is_empty() {
            None
        } else {
            let mut sorted = self.ignore_list.clone();
            sorted.sort_unstable();
            Some(sorted)
        };

        let map = SourceMapJson {
            version: 3,
            file: &self.file,
            source_root: "",
            sources: &self.sources,
            sources_content: &sources_content,
            names: &self.names,
            mappings: &mappings_str,
            x_google_ignore_list: ignore_list.as_deref(),
        };

        serde_json::to_string(&map).unwrap_or_else(|_| "{}".into())
    }

    /// Encode all mappings into VLQ-encoded string per the Source Map v3 spec.
    fn encode_mappings(&self) -> String {
        if self.mappings.is_empty() {
            return String::new();
        }

        // Sort by generated position.
        let mut sorted = self.mappings.clone();
        sorted.sort_by(|a, b| a.gen_line.cmp(&b.gen_line).then(a.gen_col.cmp(&b.gen_col)));

        let mut result = String::new();
        let mut prev_gen_line: u32 = 0;
        let mut prev_gen_col: i64 = 0;
        let mut prev_source: i64 = 0;
        let mut prev_orig_line: i64 = 0;
        let mut prev_orig_col: i64 = 0;
        let mut first_segment_on_line = true;

        for mapping in &sorted {
            // Emit semicolons for line gaps.
            while prev_gen_line < mapping.gen_line {
                result.push(';');
                prev_gen_line += 1;
                prev_gen_col = 0;
                first_segment_on_line = true;
            }

            if !first_segment_on_line {
                result.push(',');
            }
            first_segment_on_line = false;

            // Field 1: generated column (relative)
            let gen_col_delta = mapping.gen_col as i64 - prev_gen_col;
            vlq_encode(&mut result, gen_col_delta);
            prev_gen_col = mapping.gen_col as i64;

            // Field 2: source index (relative)
            let source_delta = mapping.source_idx as i64 - prev_source;
            vlq_encode(&mut result, source_delta);
            prev_source = mapping.source_idx as i64;

            // Field 3: original line (relative)
            let orig_line_delta = mapping.orig_line as i64 - prev_orig_line;
            vlq_encode(&mut result, orig_line_delta);
            prev_orig_line = mapping.orig_line as i64;

            // Field 4: original column (relative)
            let orig_col_delta = mapping.orig_col as i64 - prev_orig_col;
            vlq_encode(&mut result, orig_col_delta);
            prev_orig_col = mapping.orig_col as i64;
        }

        result
    }
}

/// Source Map v3 JSON structure (extended with `names` and `x_google_ignoreList`).
#[derive(Serialize)]
struct SourceMapJson<'a> {
    version: u8,
    file: &'a str,
    #[serde(rename = "sourceRoot")]
    source_root: &'a str,
    sources: &'a [String],
    #[serde(rename = "sourcesContent")]
    sources_content: &'a [&'a str],
    names: &'a [String],
    mappings: &'a str,
    /// Indices of sources that should be ignored by DevTools step-through.
    /// Introduced by Chrome and now supported broadly.
    #[serde(
        rename = "x_google_ignoreList",
        skip_serializing_if = "Option::is_none"
    )]
    x_google_ignore_list: Option<&'a [u32]>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct InputSourceMap {
    version: u8,
    #[serde(default)]
    source_root: String,
    #[serde(default)]
    sources: Vec<String>,
    #[serde(default)]
    sources_content: Vec<Option<String>>,
    #[serde(default)]
    mappings: String,
}

// ─────────────────────────────────────────────────────────────────────────────
// VLQ encoding (Base64-VLQ per source map spec)
// ─────────────────────────────────────────────────────────────────────────────

const BASE64_CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encode a signed integer as Base64-VLQ and append to the output string.
fn vlq_encode(out: &mut String, value: i64) {
    // Convert signed to unsigned with sign in LSB.
    let mut vlq = if value < 0 {
        ((-value) << 1) | 1
    } else {
        value << 1
    } as u64;

    loop {
        let mut digit = (vlq & 0x1F) as u8; // 5 bits
        vlq >>= 5;
        if vlq > 0 {
            digit |= 0x20; // continuation bit
        }
        out.push(BASE64_CHARS[digit as usize] as char);
        if vlq == 0 {
            break;
        }
    }
}

fn decode_vlq_segment(segment: &str) -> Option<Vec<i64>> {
    let mut values = Vec::new();
    let mut value = 0u64;
    let mut shift = 0u32;

    for byte in segment.bytes() {
        let digit = BASE64_CHARS
            .iter()
            .position(|candidate| *candidate == byte)? as u64;
        value |= (digit & 0x1f) << shift;
        if digit & 0x20 == 0 {
            let magnitude = (value >> 1) as i64;
            values.push(if value & 1 == 1 {
                -magnitude
            } else {
                magnitude
            });
            value = 0;
            shift = 0;
        } else {
            shift += 5;
            if shift > 60 {
                return None;
            }
        }
    }

    (shift == 0).then_some(values)
}

/// Decode a single VLQ value from a Base64-VLQ string (for testing).
#[cfg(test)]
fn vlq_decode(input: &str) -> (i64, usize) {
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    let mut result: u64 = 0;
    let mut shift = 0u32;

    loop {
        if i >= chars.len() {
            break;
        }
        let ch = chars[i];
        i += 1;
        let digit = BASE64_CHARS
            .iter()
            .position(|&b| b == ch as u8)
            .unwrap_or(0) as u64;

        result |= (digit & 0x1F) << shift;
        shift += 5;

        if digit & 0x20 == 0 {
            break;
        }
    }

    // Undo sign encoding.
    let is_negative = (result & 1) == 1;
    let magnitude = (result >> 1) as i64;
    let value = if is_negative { -magnitude } else { magnitude };

    (value, i)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vlq_encode_zero() {
        let mut s = String::new();
        vlq_encode(&mut s, 0);
        assert_eq!(s, "A");
    }

    #[test]
    fn vlq_encode_positive() {
        let mut s = String::new();
        vlq_encode(&mut s, 1);
        // 1 → unsigned 2 → [2] → 'C'
        assert_eq!(s, "C");
    }

    #[test]
    fn vlq_encode_negative() {
        let mut s = String::new();
        vlq_encode(&mut s, -1);
        // -1 → unsigned 3 → [3] → 'D'
        assert_eq!(s, "D");
    }

    #[test]
    fn vlq_roundtrip() {
        for value in [-100, -1, 0, 1, 5, 16, 100, 1000] {
            let mut s = String::new();
            vlq_encode(&mut s, value);
            let (decoded, _) = vlq_decode(&s);
            assert_eq!(decoded, value, "roundtrip failed for {value}");
        }
    }

    #[test]
    fn source_map_builder_basic() {
        let mut builder = SourceMapBuilder::new("bundle.js", PathBuf::from("/project"));
        let idx = builder.add_source(Path::new("/project/src/app.tsx"), Some("const x = 1;"));
        assert_eq!(idx, 0);

        builder.add_mapping(Mapping {
            gen_line: 0,
            gen_col: 0,
            source_idx: 0,
            orig_line: 0,
            orig_col: 0,
        });

        let json = builder.to_json();
        assert!(json.contains("\"version\":3"));
        assert!(json.contains("\"file\":\"bundle.js\""));
        assert!(json.contains("src/app.tsx"));
        assert!(json.contains("\"mappings\":\"AAAA\""));
    }

    #[test]
    fn source_map_multiple_lines() {
        let mut builder = SourceMapBuilder::new("out.js", PathBuf::from("/root"));
        let idx = builder.add_source(Path::new("/root/a.ts"), None);

        builder.add_mapping(Mapping {
            gen_line: 0,
            gen_col: 0,
            source_idx: idx,
            orig_line: 0,
            orig_col: 0,
        });
        builder.add_mapping(Mapping {
            gen_line: 1,
            gen_col: 0,
            source_idx: idx,
            orig_line: 1,
            orig_col: 0,
        });
        builder.add_mapping(Mapping {
            gen_line: 2,
            gen_col: 4,
            source_idx: idx,
            orig_line: 2,
            orig_col: 4,
        });

        let json = builder.to_json();
        assert!(json.contains("\"version\":3"));
        // Mappings should have semicolons separating lines.
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let mappings = parsed["mappings"].as_str().unwrap();
        assert_eq!(mappings.matches(';').count(), 2);
    }

    #[test]
    fn identity_mappings() {
        let mut builder = SourceMapBuilder::new("bundle.js", PathBuf::from("/p"));
        let idx = builder.add_source(Path::new("/p/file.ts"), None);
        let source = "const x = 1;\nconst y = 2;\n\nconst z = 3;";
        builder.add_identity_mappings(idx, source, 5);

        // Should have mappings at gen lines 5, 6, 8 (skipping blank line 7)
        let lines: Vec<u32> = builder.mappings.iter().map(|m| m.gen_line).collect();
        assert!(lines.contains(&5));
        assert!(lines.contains(&6));
        assert!(lines.contains(&8));
        assert!(!lines.contains(&7)); // blank line skipped
    }

    #[test]
    fn add_source_deduplicates() {
        let mut builder = SourceMapBuilder::new("out.js", PathBuf::from("/root"));
        let idx1 = builder.add_source(Path::new("/root/foo.ts"), None);
        let idx2 = builder.add_source(Path::new("/root/foo.ts"), None);
        let idx3 = builder.add_source(Path::new("/root/bar.ts"), None);
        assert_eq!(idx1, idx2);
        assert_ne!(idx1, idx3);
        assert_eq!(builder.sources.len(), 2);
    }

    #[test]
    fn names_array_in_output() {
        let mut builder = SourceMapBuilder::new("bundle.js", PathBuf::from("/p"));
        let n1 = builder.add_name("MyComponent");
        let n2 = builder.add_name("helper");
        let n3 = builder.add_name("MyComponent"); // duplicate — should return same idx
        assert_eq!(n1, 0);
        assert_eq!(n2, 1);
        assert_eq!(n1, n3, "duplicate names should return the same index");

        let json = builder.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let names = parsed["names"].as_array().unwrap();
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].as_str().unwrap(), "MyComponent");
        assert_eq!(names[1].as_str().unwrap(), "helper");
    }

    #[test]
    fn x_google_ignore_list_emitted_when_set() {
        let mut builder = SourceMapBuilder::new("bundle.js", PathBuf::from("/p"));
        let idx = builder.add_source(Path::new("/p/generated.ts"), None);
        builder.add_to_ignore_list(idx);

        let json = builder.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let ignore_list = parsed["x_google_ignoreList"].as_array().unwrap();
        assert_eq!(ignore_list.len(), 1);
        assert_eq!(ignore_list[0].as_u64().unwrap(), 0);
    }

    #[test]
    fn x_google_ignore_list_omitted_when_empty() {
        let builder = SourceMapBuilder::new("bundle.js", PathBuf::from("/p"));
        let json = builder.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(
            parsed.get("x_google_ignoreList").is_none(),
            "should omit x_google_ignoreList when empty"
        );
    }

    #[test]
    fn source_map_json_contains_names_field() {
        let builder = SourceMapBuilder::new("bundle.js", PathBuf::from("/p"));
        let json = builder.to_json();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        // Even when empty, `names` field should be present as [].
        assert!(
            parsed.get("names").is_some(),
            "names field should always be present"
        );
        assert_eq!(
            parsed["names"].as_array().unwrap().len(),
            0,
            "names should be empty when none were added"
        );
    }

    #[test]
    fn imports_source_map_v3_mappings_with_offset() {
        let mut builder = SourceMapBuilder::new("bundle.js", PathBuf::from("/project"));
        let input = r#"{"version":3,"sourceRoot":"src","sources":["input.ts"],"sourcesContent":["const value = 1"],"names":[],"mappings":"AAAA"}"#;

        assert!(builder.add_source_map(input, 4));

        let parsed: serde_json::Value = serde_json::from_str(&builder.to_json()).unwrap();
        assert_eq!(parsed["sources"][0], "src/input.ts");
        assert_eq!(parsed["sourcesContent"][0], "const value = 1");
        assert_eq!(parsed["mappings"].as_str().unwrap().matches(';').count(), 4);
    }
}
