use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub file: PathBuf,
    pub line: Option<u32>,
    pub column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    pub code: &'static str,
    pub title: String,
    pub explanation: String,
    pub span: Option<SourceSpan>,
    pub import_chain: Vec<PathBuf>,
    pub suggested_fix: Option<String>,
    pub affected_routes: Vec<String>,
}

impl Diagnostic {
    pub fn new(code: &'static str, title: impl Into<String>) -> Self {
        Self {
            code,
            title: title.into(),
            explanation: String::new(),
            span: None,
            import_chain: Vec::new(),
            suggested_fix: None,
            affected_routes: Vec::new(),
        }
    }

    pub fn explain(mut self, explanation: impl Into<String>) -> Self {
        self.explanation = explanation.into();
        self
    }

    pub fn at_file(mut self, file: impl Into<PathBuf>) -> Self {
        self.span = Some(SourceSpan {
            file: file.into(),
            line: None,
            column: None,
        });
        self
    }

    /// Attach a file path with line and column info.
    pub fn at_file_with_span(mut self, file: impl Into<PathBuf>, line: u32, column: u32) -> Self {
        self.span = Some(SourceSpan {
            file: file.into(),
            line: Some(line),
            column: Some(column),
        });
        self
    }

    pub fn suggest(mut self, fix: impl Into<String>) -> Self {
        self.suggested_fix = Some(fix.into());
        self
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "{}: {}", self.code, self.title)?;

        if let Some(span) = &self.span {
            match (span.line, span.column) {
                (Some(line), Some(col)) => {
                    writeln!(formatter, "File: {}:{}:{}", span.file.display(), line, col)?;
                }
                (Some(line), None) => {
                    writeln!(formatter, "File: {}:{}", span.file.display(), line)?;
                }
                _ => {
                    writeln!(formatter, "File: {}", span.file.display())?;
                }
            }
        }

        if !self.explanation.is_empty() {
            writeln!(formatter, "\nWhy:\n  {}", self.explanation)?;
        }

        if let Some(fix) = &self.suggested_fix {
            writeln!(formatter, "\nFix:\n  {fix}")?;
        }

        if !self.affected_routes.is_empty() {
            writeln!(
                formatter,
                "\nAffected routes:\n  {}",
                self.affected_routes.join("\n  ")
            )?;
        }

        Ok(())
    }
}

#[derive(Debug, Error)]
pub enum RuvyxaError {
    #[error("{0}")]
    Diagnostic(Box<Diagnostic>),

    #[error("{message}")]
    Io {
        message: String,
        #[source]
        source: std::io::Error,
    },

    #[error("{0}")]
    Message(String),
}

impl From<Diagnostic> for RuvyxaError {
    fn from(diagnostic: Diagnostic) -> Self {
        Self::Diagnostic(Box::new(diagnostic))
    }
}

impl From<std::io::Error> for RuvyxaError {
    fn from(source: std::io::Error) -> Self {
        Self::Io {
            message: source.to_string(),
            source,
        }
    }
}

pub type Result<T> = std::result::Result<T, RuvyxaError>;

/// Convert framework diagnostics into a deterministic SARIF 2.1.0 log.
///
/// This is intentionally a serializer over the existing validation results,
/// not a second scanner. Human, JSON, and SARIF output therefore share the
/// same rules, locations, and failure status.
#[must_use]
pub fn diagnostics_to_sarif(
    diagnostics: &[Diagnostic],
    tool_name: &str,
    tool_version: &str,
    project_root: &Path,
) -> serde_json::Value {
    let mut rules = BTreeMap::<&str, &Diagnostic>::new();
    for diagnostic in diagnostics {
        rules.entry(diagnostic.code).or_insert(diagnostic);
    }

    let rules = rules
        .into_iter()
        .map(|(code, diagnostic)| {
            let mut rule = serde_json::json!({
                "id": code,
                "name": code,
                "shortDescription": { "text": diagnostic.title },
                "fullDescription": { "text": diagnostic.explanation },
                "defaultConfiguration": { "level": "error" },
            });
            if let Some(fix) = &diagnostic.suggested_fix {
                rule["help"] = serde_json::json!({ "text": fix });
            }
            rule
        })
        .collect::<Vec<_>>();

    let normalized_root = normalized_canonical_path(project_root);
    let results = diagnostics
        .iter()
        .map(|diagnostic| {
            let locations = diagnostic.span.as_ref().map_or_else(Vec::new, |span| {
                let normalized_file = normalized_canonical_path(&span.file);
                let file = normalized_file
                    .strip_prefix(&normalized_root)
                    .unwrap_or(&normalized_file)
                    .to_string_lossy()
                    .replace('\\', "/");
                let mut region = serde_json::Map::new();
                if let Some(line) = span.line {
                    region.insert("startLine".to_string(), line.into());
                }
                if let Some(column) = span.column {
                    region.insert("startColumn".to_string(), column.into());
                }
                vec![serde_json::json!({
                    "physicalLocation": {
                        "artifactLocation": { "uri": file },
                        "region": region,
                    }
                })]
            });
            let message = if diagnostic.explanation.is_empty() {
                diagnostic.title.clone()
            } else {
                format!("{}: {}", diagnostic.title, diagnostic.explanation)
            };
            serde_json::json!({
                "ruleId": diagnostic.code,
                "level": "error",
                "message": { "text": message },
                "locations": locations,
                "properties": {
                    "suggestedFix": diagnostic.suggested_fix,
                    "affectedRoutes": diagnostic.affected_routes,
                    "importChain": diagnostic.import_chain.iter().map(|path| path.to_string_lossy()).collect::<Vec<_>>(),
                },
            })
        })
        .collect::<Vec<_>>();

    serde_json::json!({
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "version": "2.1.0",
        "runs": [{
            "tool": { "driver": {
                "name": tool_name,
                "version": tool_version,
                "informationUri": PROJECT_URL,
                "rules": rules,
            }},
            "results": results,
        }],
    })
}

/// Where a reader of a SARIF report is sent to find out what this tool is.
///
/// It named `github.com/ruvyxa/ruvyxa` — an owner this project has never used —
/// so every report uploaded to a code-scanning dashboard linked somewhere that
/// does not exist. Nothing pointed at it, in Rust or in the docs, so nothing
/// noticed. Spelled once here and asserted below; the casing is the canonical
/// one GitHub reports for the repository.
const PROJECT_URL: &str = "https://github.com/thirawat27/Ruvyxa";

/// Windows extended-length ("verbatim") path prefix that `canonicalize` adds.
#[cfg(windows)]
const WINDOWS_VERBATIM_PREFIX: &str = "\\\\?\\";
#[cfg(windows)]
const WINDOWS_VERBATIM_UNC_PREFIX: &str = "\\\\?\\UNC\\";

/// Spell a path already in hand without its Windows extended-length prefix.
///
/// Separate from [`normalized_canonical_path`] because the two answer different
/// questions. That one asks the file system what a path really is; this one only
/// respells the path it is given, touching no disk — which is what a lookup key
/// needs, and a key that canonicalized would pay a syscall for every module a
/// bundle loads.
///
/// The prefix matters because it is contagious: a root carrying it hands it to
/// every path derived from it, while a path the same build receives from a Node
/// worker never has one. Two spellings of one file then fail to compare equal,
/// and the failure surfaces nowhere near the comparison — a server-components
/// build whose root had been canonicalized lost every `'use server'`
/// substitution and was refused as `RUV1820`, naming an import the project is
/// right to have.
///
/// Join a diagnostic code and a message without repeating a code the message
/// already carries.
///
/// A worker reports `{ code, message }`, and the message is usually the error a
/// hook threw — whose own class already prefixes its code, because that is what
/// makes the code visible in a browser console and in a raw stack. Prepending
/// the reported code as well produced two of them, the outer one naming nothing
/// but which worker relayed it:
///
/// - `RUV1700 RUV3201 native collaboration requires a long-lived Node/Bun build`
/// - `RUV2200 RUV2202 adapter static supports ssg, csr; unsupported routes: …`
///
/// The inner code wins, because it is the one that names the decision and the
/// one a reader can search for.
#[must_use]
pub fn label_with_code(code: &str, message: &str) -> String {
    if starts_with_diagnostic_code(message) {
        return message.to_string();
    }
    format!("{code} {message}")
}

/// Whether this text opens with `RUV` and four digits, as a whole token.
fn starts_with_diagnostic_code(message: &str) -> bool {
    let Some(rest) = message.strip_prefix("RUV") else {
        return false;
    };
    let digits = rest.as_bytes();
    if digits.len() < 4 || !digits[..4].iter().all(u8::is_ascii_digit) {
        return false;
    }
    // A whole token: `RUV1700` and `RUV1700 …` both count, `RUV17005` does not,
    // so a five-digit code added later cannot be read as a four-digit one with a
    // stray character after it.
    matches!(rest[4..].chars().next(), None | Some(' ') | Some(':'))
}

/// A no-op on other platforms and on any Windows path without the prefix.
#[must_use]
pub fn without_verbatim_prefix(path: &std::path::Path) -> std::path::PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(stripped) = text.strip_prefix(WINDOWS_VERBATIM_UNC_PREFIX) {
            return std::path::PathBuf::from(format!("\\\\{stripped}"));
        }
        if let Some(stripped) = text.strip_prefix(WINDOWS_VERBATIM_PREFIX) {
            return std::path::PathBuf::from(stripped);
        }
    }
    path.to_path_buf()
}

/// Canonicalizes a path without a Windows verbatim (`\\?\`) prefix.
///
/// `std::fs::canonicalize` returns extended-length paths on Windows. Those
/// leak into JavaScript runtime scripts where `pathToFileURL` under Bun
/// rejects them, so every canonicalization that can reach a subprocess or a
/// user-facing string goes through this helper. Falls back to the original
/// path when canonicalization fails (for example, the path does not exist).
#[must_use]
pub fn normalized_canonical_path(path: &std::path::Path) -> std::path::PathBuf {
    match path.canonicalize() {
        Ok(canonical) => without_verbatim_prefix(&canonical),
        Err(_) => path.to_path_buf(),
    }
}

#[cfg(test)]
mod path_tests {
    use super::{Diagnostic, diagnostics_to_sarif, label_with_code, normalized_canonical_path};

    #[test]
    fn normalized_canonical_path_has_no_verbatim_prefix() {
        let current = std::env::current_dir().unwrap();
        let normalized = normalized_canonical_path(&current);
        #[cfg(windows)]
        assert!(!normalized.to_string_lossy().starts_with("\\\\?\\"));
        let _ = normalized;
    }

    #[test]
    fn normalized_canonical_path_keeps_missing_paths_unchanged() {
        let missing = std::path::Path::new("definitely-missing-ruvyxa-path");
        assert_eq!(normalized_canonical_path(missing), missing.to_path_buf());
    }

    #[test]
    fn sarif_uses_project_relative_locations_and_deduplicates_rules() {
        let root = std::path::Path::new("project");
        let diagnostics = vec![
            Diagnostic::new("RUV1001", "Private import")
                .explain("A client module imports server-only code.")
                .at_file_with_span(root.join("app/page.tsx"), 4, 7)
                .suggest("Move the import behind a server boundary."),
            Diagnostic::new("RUV1001", "Private import").at_file(root.join("app/other.tsx")),
        ];

        let sarif = diagnostics_to_sarif(&diagnostics, "Ruvyxa", "1.0.23", root);

        assert_eq!(sarif["version"], "2.1.0");
        // The link a code-scanning dashboard renders. It pointed at a
        // nonexistent owner for as long as this function had existed, because
        // no assertion and no doc named it.
        assert_eq!(
            sarif["runs"][0]["tool"]["driver"]["informationUri"],
            "https://github.com/thirawat27/Ruvyxa"
        );
        assert_eq!(
            sarif["runs"][0]["tool"]["driver"]["rules"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(sarif["runs"][0]["results"].as_array().unwrap().len(), 2);
        assert_eq!(
            sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]
                ["uri"],
            "app/page.tsx"
        );
        assert_eq!(
            sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["region"]["startLine"],
            4
        );

        let no_help = diagnostics_to_sarif(
            &[Diagnostic::new("RUV1002", "No fix available")],
            "Ruvyxa",
            "1.0.23",
            root,
        );
        assert!(
            no_help["runs"][0]["tool"]["driver"]["rules"][0]
                .get("help")
                .is_none()
        );
    }

    #[test]
    fn a_message_that_already_names_a_code_does_not_get_a_second_one() {
        // Both observed in real output: the outer code names only which worker
        // relayed the failure, and the reader has to search for the inner one.
        assert_eq!(
            label_with_code(
                "RUV1700",
                "RUV3201 native collaboration requires a long-lived build"
            ),
            "RUV3201 native collaboration requires a long-lived build"
        );
        assert_eq!(
            label_with_code("RUV2200", "RUV2202 adapter static supports ssg, csr"),
            "RUV2202 adapter static supports ssg, csr"
        );
    }

    #[test]
    fn a_message_that_names_no_code_still_gets_one() {
        assert_eq!(
            label_with_code("RUV1700", "the hook threw"),
            "RUV1700 the hook threw"
        );
        // Not a code: the prefix has to be a whole token, so a longer number or
        // a word that merely starts with the letters is left alone.
        assert_eq!(
            label_with_code("RUV1700", "RUV17005 x"),
            "RUV1700 RUV17005 x"
        );
        assert_eq!(
            label_with_code("RUV1700", "RUVYXA_LOCALE is unset"),
            "RUV1700 RUVYXA_LOCALE is unset"
        );
        assert_eq!(label_with_code("RUV1700", "RUV17"), "RUV1700 RUV17");
    }

    #[test]
    fn a_code_alone_is_a_code() {
        assert_eq!(label_with_code("RUV1700", "RUV3201"), "RUV3201");
        assert_eq!(
            label_with_code("RUV1700", "RUV3201: detail"),
            "RUV3201: detail"
        );
    }
}
