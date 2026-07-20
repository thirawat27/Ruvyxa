use std::fmt;
use std::path::PathBuf;

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

/// Windows extended-length ("verbatim") path prefix that `canonicalize` adds.
#[cfg(windows)]
const WINDOWS_VERBATIM_PREFIX: &str = "\\\\?\\";
#[cfg(windows)]
const WINDOWS_VERBATIM_UNC_PREFIX: &str = "\\\\?\\UNC\\";

/// Canonicalizes a path without a Windows verbatim (`\\?\`) prefix.
///
/// `std::fs::canonicalize` returns extended-length paths on Windows. Those
/// leak into JavaScript runtime scripts where `pathToFileURL` under Bun
/// rejects them, so every canonicalization that can reach a subprocess or a
/// user-facing string goes through this helper. Falls back to the original
/// path when canonicalization fails (for example, the path does not exist).
#[must_use]
pub fn normalized_canonical_path(path: &std::path::Path) -> std::path::PathBuf {
    let canonical = match path.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => return path.to_path_buf(),
    };
    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy();
        if let Some(stripped) = text.strip_prefix(WINDOWS_VERBATIM_UNC_PREFIX) {
            return std::path::PathBuf::from(format!("\\\\{stripped}"));
        }
        if let Some(stripped) = text.strip_prefix(WINDOWS_VERBATIM_PREFIX) {
            return std::path::PathBuf::from(stripped);
        }
    }
    canonical
}

#[cfg(test)]
mod path_tests {
    use super::normalized_canonical_path;

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
}
