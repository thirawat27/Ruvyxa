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

    let normalized_root = normalized_canonical_path(project_root);
    let redact = |text: &str, diagnostic: &Diagnostic| {
        redact_report_text(text, diagnostic, project_root, &normalized_root)
    };

    let rules = rules
        .into_iter()
        .map(|(code, diagnostic)| {
            let mut rule = serde_json::json!({
                "id": code,
                "name": code,
                "shortDescription": { "text": diagnostic.title },
                // The rule carries the first diagnostic's explanation verbatim,
                // so it discloses everything `message` does and had to be
                // rewritten with it.
                "fullDescription": { "text": redact(&diagnostic.explanation, diagnostic) },
                "defaultConfiguration": { "level": "error" },
            });
            if let Some(fix) = &diagnostic.suggested_fix {
                rule["help"] = serde_json::json!({ "text": redact(fix, diagnostic) });
            }
            rule
        })
        .collect::<Vec<_>>();

    let results = diagnostics
        .iter()
        .map(|diagnostic| {
            let locations = diagnostic.span.as_ref().map_or_else(Vec::new, |span| {
                let file = report_path(&span.file, project_root, &normalized_root);
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
                format!(
                    "{}: {}",
                    diagnostic.title,
                    redact(&diagnostic.explanation, diagnostic)
                )
            };
            serde_json::json!({
                "ruleId": diagnostic.code,
                "level": "error",
                "message": { "text": message },
                "locations": locations,
                "properties": {
                    "suggestedFix": diagnostic
                        .suggested_fix
                        .as_deref()
                        .map(|fix| redact(fix, diagnostic)),
                    "affectedRoutes": diagnostic.affected_routes,
                    "importChain": diagnostic
                        .import_chain
                        .iter()
                        .map(|path| report_path(path, project_root, &normalized_root))
                        .collect::<Vec<_>>(),
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

/// The placeholder for a file the project root cannot explain.
///
/// A module reached through a workspace `file:` link, a package store, or a
/// sibling checkout is outside the root and cannot be relativised — and
/// `normalized_canonical_path` resolves symlinks, so pnpm's linked packages
/// land here routinely. Naming the file alone keeps the result pointing
/// somewhere without describing the machine it was produced on.
const OUTSIDE_PROJECT: &str = "<outside-project>";

/// A path as it may appear in an uploaded SARIF report.
///
/// Relative to the project root where that is possible, by the raw spelling as
/// well as the canonical one — a project that lives under a symlinked directory
/// canonicalizes to somewhere the root does not prefix, and falling back to the
/// absolute path is exactly the disclosure this avoids. A path that was already
/// relative is left alone: it names no machine, and rewriting it to
/// [`OUTSIDE_PROJECT`] would lose the only location the diagnostic had.
fn report_path(path: &Path, project_root: &Path, normalized_root: &Path) -> String {
    let normalized = normalized_canonical_path(path);
    for (candidate, root) in [
        (normalized.as_path(), normalized_root),
        (path, project_root),
    ] {
        if let Ok(relative) = candidate.strip_prefix(root) {
            return relative.to_string_lossy().replace('\\', "/");
        }
    }

    if !path.is_absolute() {
        return path.to_string_lossy().replace('\\', "/");
    }

    match path.file_name() {
        Some(name) => format!("{OUTSIDE_PROJECT}/{}", name.to_string_lossy()),
        None => OUTSIDE_PROJECT.to_string(),
    }
}

/// Rewrite the paths written into report prose.
///
/// `message`, a rule's `fullDescription`, and its `help` are free text with
/// paths interpolated into them — `RUV1003` names two files in one sentence,
/// `RUV1013` one — so there is no field to relativise, and this rewrites the
/// text instead. Two passes, because they can reach different things:
///
/// 1. Every path the diagnostic also carries **structurally** — its span, its
///    import chain — is replaced by exactly the spelling
///    [`report_path`] gives that field. This is the pass that reaches a path
///    *outside* the project, which no root prefix can.
/// 2. What is left has the project root taken off the front. Both spellings of
///    the root are stripped, raw and canonical, and on Windows each with
///    forward slashes as well — a path that came back from a Node worker is
///    spelled `/` and one this process built is spelled `\`.
///
/// What neither pass reaches is an absolute path the prose names and no field
/// records, from outside the project. Finding those would mean guessing which
/// runs of text in a sentence are paths, and on a platform where `/` opens both
/// an absolute path and every URL this framework prints, that guess is not
/// available. A diagnostic that names a file should put it in `span` or
/// `import_chain`, which is where the two other consumers of this type — the
/// terminal renderer and the JSON report — read it from anyway.
fn redact_report_text(
    text: &str,
    diagnostic: &Diagnostic,
    project_root: &Path,
    normalized_root: &Path,
) -> String {
    let mut redacted = text.to_string();

    let mut known = diagnostic
        .span
        .iter()
        .map(|span| span.file.clone())
        .chain(diagnostic.import_chain.iter().cloned())
        .collect::<Vec<_>>();
    // Longest first: one known path may be a prefix of another, and rewriting
    // the shorter one first would leave the tail of the longer one behind.
    known.sort_by_key(|path| std::cmp::Reverse(path.as_os_str().len()));
    known.dedup();
    for path in known {
        let replacement = report_path(&path, project_root, normalized_root);
        for spelling in path_spellings(&path) {
            redacted = redacted.replace(&spelling, &replacement);
        }
    }

    for root in root_spellings(project_root, normalized_root) {
        if root.is_empty() {
            continue;
        }
        redacted = redacted.replace(&format!("{root}\\"), "");
        redacted = redacted.replace(&format!("{root}/"), "");
        // The root named on its own is the project directory itself.
        redacted = redacted.replace(root.as_str(), ".");
    }
    redacted
}

/// Every way one path may be written in report prose, longest first.
fn path_spellings(path: &Path) -> Vec<String> {
    let mut spellings = Vec::new();
    for candidate in [normalized_canonical_path(path), path.to_path_buf()] {
        let text = candidate.to_string_lossy().into_owned();
        if cfg!(windows) {
            let slashed = text.replace('\\', "/");
            if slashed != text {
                spellings.push(slashed);
            }
        }
        spellings.push(text);
    }
    spellings.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    spellings.dedup();
    spellings
}

/// Every way the project root may be written in report prose, longest first.
fn root_spellings(project_root: &Path, normalized_root: &Path) -> Vec<String> {
    let mut spellings = Vec::new();
    for root in [normalized_root, project_root] {
        let text = root.to_string_lossy().into_owned();
        // On Unix a backslash is an ordinary character in a file name, so only
        // Windows has a second spelling of the same directory.
        if cfg!(windows) {
            let slashed = text.replace('\\', "/");
            if slashed != text {
                spellings.push(slashed);
            }
        }
        spellings.push(text);
    }
    spellings.sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
    spellings.dedup();
    spellings
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

/// The message a worker sent with its failure, or a truthful stand-in.
///
/// A worker that answers `ok: false` and no `message` has broken its own half
/// of the protocol. Four call sites filled that gap with the literal `unknown
/// error`, which reads as though the framework knows the cause and will not say
/// — the reader has nowhere to go from it, and no reason to suspect the
/// omission is upstream.
///
/// This says what actually happened and names the one place the detail can
/// still be. A worker's diagnostics go to its stderr, because stdout carries
/// the NDJSON response protocol; `ruvyxa` logs those lines at the severity the
/// worker tagged them with, and the default `RUST_LOG` filter is `warn`, so
/// anything it tagged `debug` or `info` was collected and then hidden.
#[must_use]
pub fn worker_failure_message(message: Option<String>) -> String {
    message.unwrap_or_else(|| {
        "the worker reported a failure without sending a message; \
         re-run with RUST_LOG=debug to see the output it wrote to stderr"
            .to_string()
    })
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
    use super::{
        Diagnostic, diagnostics_to_sarif, label_with_code, normalized_canonical_path,
        without_verbatim_prefix, worker_failure_message,
    };

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

    /// A SARIF report is written to be uploaded — to GitHub code scanning, to a
    /// vendor dashboard — so nothing in it may spell out where the build ran.
    ///
    /// Three places used to. `locations` relativised but fell back to the
    /// absolute path whenever `strip_prefix` failed, which it does for anything
    /// reached through a symlink because the path is canonicalized first;
    /// `message` was never relativised at all, and route diagnostics
    /// interpolate absolute paths straight into `explanation`; and
    /// `importChain` was serialized verbatim. Between them the uploaded report
    /// carried the developer's or CI runner's directory layout, the username in
    /// a home-directory path, and the names of sibling workspaces.
    #[test]
    fn an_uploaded_sarif_report_carries_no_absolute_developer_path() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixture-project");
        let inside = root.join("app/page.tsx");
        // A module outside the project: a workspace `file:` link, a package
        // store, or simply a sibling checkout. `strip_prefix` cannot relativise
        // it, and printing it anyway is the disclosure.
        let outside = root
            .parent()
            .expect("the fixture root has a parent")
            .join("private-sibling/lib/data.ts");

        let mut inside_diagnostic = Diagnostic::new("RUV1003", "Conflicting route paths")
            .explain(format!(
                "{} and {} resolve to the same URL match shape.",
                inside.display(),
                root.join("app/(marketing)/page.tsx").display()
            ))
            .at_file_with_span(&inside, 1, 1);
        inside_diagnostic.import_chain = vec![inside.clone(), outside.clone()];

        let outside_diagnostic = Diagnostic::new("RUV1013", "Edge route reaches a Node built-in")
            .explain(format!("{} imports `node:fs`.", outside.display()))
            .at_file(&outside);

        let sarif = diagnostics_to_sarif(
            &[inside_diagnostic, outside_diagnostic],
            "Ruvyxa",
            "1.0.23",
            &root,
        );
        let serialized = serde_json::to_string(&sarif).expect("sarif serializes");

        for leaked in [
            root.display().to_string(),
            inside.display().to_string(),
            outside.display().to_string(),
        ] {
            // The report is JSON, so a Windows separator arrives escaped.
            let escaped = leaked.replace('\\', "\\\\");
            assert!(
                !serialized.contains(&leaked) && !serialized.contains(&escaped),
                "an uploaded report must not spell `{leaked}`:\n{serialized}"
            );
        }

        assert_eq!(
            sarif["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]
                ["uri"],
            "app/page.tsx"
        );
        // A path the project root cannot explain is named by its file alone, so
        // the result still points somewhere without describing the machine.
        assert_eq!(
            sarif["runs"][0]["results"][1]["locations"][0]["physicalLocation"]["artifactLocation"]
                ["uri"],
            "<outside-project>/data.ts"
        );
        assert_eq!(
            sarif["runs"][0]["results"][0]["properties"]["importChain"][1],
            "<outside-project>/data.ts"
        );
        assert!(
            sarif["runs"][0]["results"][0]["message"]["text"]
                .as_str()
                .expect("message text")
                .contains("app/page.tsx"),
            "the message must still say which file: {}",
            sarif["runs"][0]["results"][0]["message"]["text"]
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

    /// No host may join a code to a message by hand again.
    ///
    /// This started as one doubled code in a Vercel build log and turned out to
    /// be fifteen call sites across three crates, each formatting `{code}` and
    /// `{message}` itself. Six were fixed and nine were not, which is what a
    /// list-shaped sweep does to a class — so the rule is asserted over the
    /// source rather than left to whoever reads the next one.
    ///
    /// **And then the assertion missed three.** Its first version listed two
    /// literal strings, `format!("{code}: {message}")` and its `{explanation}`
    /// twin — the exact spellings that had just been fixed. Three sites spelled
    /// the same mistake with a space instead of a colon, inside `anyhow::bail!`
    /// rather than `format!`, and shipped: `RUV1700 RUV1863` came out of a
    /// server-components build months later. A gate written from the examples
    /// you already fixed only proves you fixed them.
    ///
    /// So it matches the *shape* now — a code placeholder immediately followed
    /// by a message placeholder, whatever separates them and whatever macro
    /// surrounds them — rather than a list of spellings.
    #[test]
    fn no_crate_formats_a_code_beside_a_message_by_hand() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        // Every name a call site has used for the two halves. The pair is what
        // matters: `{code}` alone is fine, and so is `{message}` alone.
        const CODES: [&str; 2] = ["{code}", "{diagnostic_code}"];
        const MESSAGES: [&str; 4] = ["{message}", "{explanation}", "{detail}", "{error}"];
        // What a call site may legitimately put between them. A longer run of
        // text means the code is being used as a word in a sentence rather than
        // pasted onto the front of a message.
        const SEPARATORS: [&str; 3] = [" ", ": ", " - "];

        let mut offenders = Vec::new();
        // Every crate, discovered rather than listed. The first version named
        // the three crates that happened to hold the fifteen known sites, which
        // is the same mistake as listing the two known spellings: a rule that
        // only covers where the bug has already been found does not stop the
        // next one, and a new crate would join uncovered and silent.
        let mut crate_directories = std::fs::read_dir(root.join("crates"))
            .expect("crates directory")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        crate_directories.sort();
        assert!(
            crate_directories.len() >= 2,
            "expected to discover the workspace crates, found {}",
            crate_directories.len()
        );

        // `label_with_code` *is* the join, so the crate defining it is the one
        // place the shape is allowed. The exemption is checked rather than
        // assumed: if the helper moves, this fails instead of quietly
        // un-covering whichever crate it moved to.
        let owner = env!("CARGO_PKG_NAME");
        let owner_source =
            std::fs::read_to_string(root.join("crates").join(owner).join("src").join("lib.rs"))
                .expect("the crate that owns label_with_code");
        assert!(
            owner_source.contains("pub fn label_with_code"),
            "{owner} no longer defines label_with_code; move this exemption to whichever crate does"
        );

        for crate_directory in crate_directories {
            if crate_directory.file_name().and_then(|name| name.to_str()) == Some(owner) {
                continue;
            }
            let directory = crate_directory.join("src");
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                // Comment lines are dropped first. A comment explaining the
                // rule names the shape it forbids, and a gate that fires on
                // the prose describing it is a gate people route around.
                //
                // Blanked rather than dropped, so an offender's reported line
                // number still points at the real file. Filtering them out
                // shifted every index below the first comment.
                let source = source
                    .lines()
                    .map(|line| {
                        if line.trim_start().starts_with("//") {
                            ""
                        } else {
                            line
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                for code in CODES {
                    for separator in SEPARATORS {
                        for message in MESSAGES {
                            let shape = format!("{code}{separator}{message}");
                            if source.contains(shape.as_str()) {
                                offenders.push(format!("{} contains `{shape}`", path.display()));
                            }
                        }
                    }
                }

                // The shape check above reads the format string, so it is blind
                // to a positional join: `"{} {}", result.code…, result.message…`
                // is the same defect spelled differently, and one such site
                // survived the shape check and shipped.
                //
                // This watches the *origin* instead. Pulling a worker's `code`
                // out of its response is only ever a prelude to labelling a
                // message with it, so an extraction with no `label_with_code`
                // anywhere near it is either the bug or a join about to become
                // one.
                let lines = source.lines().collect::<Vec<_>>();
                for (index, line) in lines.iter().enumerate() {
                    if !line.contains(".code.unwrap_or") {
                        continue;
                    }
                    // Symmetric: `label_with_code(&result.code.unwrap_or(…), …)`
                    // puts the call on the line *above* the extraction, so a
                    // forward-only window flagged every correct site.
                    let window_start = index.saturating_sub(6);
                    let window_end = lines.len().min(index + 16);
                    // Two consumers are correct, and both had to be listed
                    // before this stopped reporting working code. A structured
                    // `Diagnostic` keeps the code in its own field and the
                    // worker's message in `explanation`, so nothing is spliced
                    // and nothing doubles; `label_with_code` is the string
                    // join. Anything else is building the message by hand.
                    const CONSUMERS: [&str; 2] = ["label_with_code", "Diagnostic::new("];
                    let consumed = lines[window_start..window_end]
                        .iter()
                        .any(|candidate| CONSUMERS.iter().any(|name| candidate.contains(name)));
                    if !consumed {
                        offenders.push(format!(
                            "{}:{} takes a worker's code without joining through label_with_code",
                            path.display(),
                            index + 1,
                        ));
                    }
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "a worker's message usually already opens with its own code, so pasting one in \
             front prints both. Join through label_with_code instead:
  {}",
            offenders.join(
                "
  "
            )
        );
    }

    /// Skip ASCII whitespace, in place.
    fn skip_whitespace(bytes: &[u8], cursor: &mut usize) {
        while bytes.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
            *cursor += 1;
        }
    }

    /// Read the Rust string literal at `cursor`, leading whitespace skipped,
    /// and leave `cursor` just past its closing quote.
    ///
    /// `None` when what is there is not a plain literal — an expression, a raw
    /// string, anything this deliberately does not understand. The caller drops
    /// such a call site rather than guessing at it.
    fn read_string_literal<'source>(
        source: &'source str,
        cursor: &mut usize,
    ) -> Option<&'source str> {
        let bytes = source.as_bytes();
        skip_whitespace(bytes, cursor);
        if bytes.get(*cursor) != Some(&b'"') {
            return None;
        }
        let start = *cursor + 1;
        let mut index = start;
        while let Some(&byte) = bytes.get(index) {
            match byte {
                // Whatever is escaped, it is not the terminator.
                b'\\' => index += 2,
                b'"' => {
                    *cursor = index + 1;
                    return Some(&source[start..index]);
                }
                _ => index += 1,
            }
        }
        None
    }

    /// Every `Diagnostic::new("RUV####", "title")` in one Rust source, with the
    /// line each call starts on.
    ///
    /// This parses the call instead of matching a line. `RUV1011` spells each of
    /// its three calls across three lines, so a single-line regex sees none of
    /// them and a gate written that way would pass the exact defect it was
    /// written for. A `format!` title counts as its format string: the shape is
    /// the meaning, and the values only fill it in.
    ///
    /// Two things are skipped on purpose. A call whose code is an expression —
    /// `Diagnostic::new(code, title)`, `Diagnostic::new(action_error_code(…), …)`
    /// — picks its code at run time and has no one title to pin. And everything
    /// from a top-level `#[cfg(test)]` immediately followed by a `mod` line to
    /// the end of the file is test scaffolding, which invents codes freely.
    ///
    /// That last rule is matched on the *module*, not on the attribute. Cutting
    /// at the first `#[cfg(test)]` is the obvious spelling and it is wrong here:
    /// `ruvyxa_dev_server`'s `lib.rs` opens with a `#[cfg(test)] use` on line 1,
    /// so that spelling silently discards a 4,900-line crate and reports it as
    /// clean.
    fn diagnostic_code_titles(source: &str) -> Vec<(usize, String, String)> {
        let lines = source.lines().collect::<Vec<_>>();
        let end = (0..lines.len())
            .find(|index| {
                lines[*index] == "#[cfg(test)]"
                    && lines
                        .get(index + 1)
                        .is_some_and(|next| next.starts_with("mod "))
            })
            .unwrap_or(lines.len());
        // Blanked rather than dropped, so a reported line still points at the
        // real file — the same reason the join gate above blanks them.
        let text = lines[..end]
            .iter()
            .map(|line| {
                if line.trim_start().starts_with("//") {
                    ""
                } else {
                    *line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        const CALL: &str = "Diagnostic::new(";
        let mut found = Vec::new();
        for (offset, _) in text.match_indices(CALL) {
            let mut cursor = offset + CALL.len();
            let Some(code) = read_string_literal(&text, &mut cursor) else {
                continue;
            };
            if !code.starts_with("RUV") {
                continue;
            }
            let bytes = text.as_bytes();
            skip_whitespace(bytes, &mut cursor);
            if bytes.get(cursor) != Some(&b',') {
                continue;
            }
            cursor += 1;
            skip_whitespace(bytes, &mut cursor);
            if text[cursor..].starts_with("format!(") {
                cursor += "format!(".len();
            }
            let code = code.to_string();
            let Some(title) = read_string_literal(&text, &mut cursor) else {
                continue;
            };
            found.push((
                text[..offset].matches('\n').count() + 1,
                code,
                title.to_string(),
            ));
        }
        found
    }

    /// One code, one meaning.
    ///
    /// `diagnostics_to_sarif` keys its rule table by code —
    /// `rules.entry(diagnostic.code).or_insert(diagnostic)` — so the **first**
    /// diagnostic carrying a code supplies the title and explanation that
    /// describe *every* result carrying it. Three codes carried six meanings
    /// between them, and an uploaded report labelled a missing interception
    /// target as a marker climbing above the app root. A code is a search term
    /// before it is anything else, and one that answers two questions answers
    /// neither.
    ///
    /// Because the code is a `&'static str`, nothing at compile time and
    /// nothing in CI could notice a fourth meaning joining `RUV1011`. That is
    /// the reason this reads the source rather than a registry someone
    /// maintains: a registry is only correct while somebody remembers it.
    #[test]
    fn one_diagnostic_code_carries_one_meaning() {
        // Codes that still carry more than one title, with the exact set each
        // carries and why it is still here. The *set* is pinned, not just the
        // code, so a new meaning joining one of these fails, and so does an
        // entry that is no longer needed. Nothing may be added to this list to
        // make a new collision pass.
        //
        // `RUV1007`/`RUV1008`/`RUV1009` are one meaning in two spellings: the
        // route graph says "graph" where the bundler says "bundle"/"SSR graph"
        // for the same boundary violation. `RUV1402`/`RUV1403`/`RUV1500` are
        // genuine collisions inside `ruvyxa_dev_server` and need the same split
        // `RUV1002`/`RUV1006`/`RUV1011` just had.
        const KNOWN_DIVERGENCES: [(&str, &[&str]); 6] = [
            (
                "RUV1007",
                &[
                    "Server-only module imported into client bundle",
                    "Server-only module imported into client graph",
                ],
            ),
            (
                "RUV1008",
                &[
                    "Private environment variable used in client bundle",
                    "Private environment variable used in client graph",
                ],
            ),
            (
                "RUV1009",
                &[
                    "Client-only module imported into SSR graph",
                    "Client-only module imported into server graph",
                ],
            ),
            (
                "RUV1402",
                &[
                    "CSS preprocessor requires an explicit transform plugin",
                    "Sass compilation failed",
                ],
            ),
            (
                "RUV1403",
                &[
                    "CSS @import could not be resolved",
                    "Configured CSS entry was not found",
                    "Stylesheet import could not be resolved",
                ],
            ),
            (
                "RUV1500",
                &["SSG render failed", "Server-components render failed"],
            ),
        ];

        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        // Discovered, not listed, for the same reason as the join gate above: a
        // new crate would otherwise join uncovered and silent.
        let mut crate_directories = std::fs::read_dir(root.join("crates"))
            .expect("crates directory")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect::<Vec<_>>();
        crate_directories.sort();

        type Meanings =
            std::collections::BTreeMap<String, std::collections::BTreeMap<String, Vec<String>>>;
        let mut meanings = Meanings::new();
        let mut sites = 0usize;
        for crate_directory in crate_directories {
            let Ok(entries) = std::fs::read_dir(crate_directory.join("src")) else {
                continue;
            };
            let mut files = entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("rs"))
                .collect::<Vec<_>>();
            files.sort();
            for path in files {
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                for (line, code, title) in diagnostic_code_titles(&source) {
                    sites += 1;
                    meanings
                        .entry(code)
                        .or_default()
                        .entry(title)
                        .or_default()
                        .push(format!(
                            "{}:{line}",
                            without_verbatim_prefix(&path).display()
                        ));
                }
            }
        }

        // A floor, because the failure mode of a source-scanning gate is
        // finding nothing and calling that a pass. The `#[cfg(test)]` cut above
        // has already been wrong once in exactly that direction.
        assert!(
            sites >= 50,
            "found only {sites} `Diagnostic::new` call sites; the scan stopped seeing the source"
        );

        let mut offenders = Vec::new();
        for (code, titles) in &meanings {
            let observed = titles.keys().map(String::as_str).collect::<Vec<_>>();
            match KNOWN_DIVERGENCES
                .iter()
                .find(|(known, _)| known == code)
                .map(|(_, expected)| *expected)
            {
                Some(expected) if observed.as_slice() == expected => {}
                Some(expected) => offenders.push(format!(
                    "{code} is listed as a known divergence carrying {expected:?}, but now carries \
                     {observed:?}. Give the new meaning its own code, or — if the code has one \
                     meaning again — delete its entry from KNOWN_DIVERGENCES."
                )),
                None if observed.len() > 1 => offenders.push(format!(
                    "{code} carries {} meanings:\n      {}",
                    observed.len(),
                    titles
                        .iter()
                        .map(|(title, where_raised)| format!(
                            "\"{title}\" at {}",
                            where_raised.join(", ")
                        ))
                        .collect::<Vec<_>>()
                        .join("\n      ")
                )),
                None => {}
            }
        }
        for (code, _) in KNOWN_DIVERGENCES {
            if !meanings.contains_key(code) {
                offenders.push(format!(
                    "{code} is listed as a known divergence but is no longer raised at all; \
                     delete its entry"
                ));
            }
        }

        assert!(
            offenders.is_empty(),
            "a diagnostic code is a search term, and SARIF describes every result carrying a code \
             with the first one it saw. Give the rarer meaning its own code and document it in \
             docs/*/16-troubleshooting-upgrades.md:
  {}",
            offenders.join(
                "
  "
            )
        );
    }

    /// "unknown error" told the reader nothing and implied the framework knew
    /// more than it was saying. The replacement has to name the real situation
    /// and point somewhere, so this asserts both halves rather than just that
    /// the string changed.
    #[test]
    fn a_worker_that_sends_no_message_says_so_and_says_where_to_look() {
        let absent = worker_failure_message(None);
        assert!(
            absent.contains("without sending a message"),
            "the reader must learn the omission is upstream: {absent}"
        );
        assert!(
            absent.contains("RUST_LOG=debug"),
            "a message with nowhere to go is the defect being fixed: {absent}"
        );
        assert!(!absent.contains("unknown error"), "{absent}");

        // A worker that did send one is passed through untouched.
        assert_eq!(
            worker_failure_message(Some("RUV1863 react-server-dom-webpack".to_string())),
            "RUV1863 react-server-dom-webpack"
        );
    }

    /// The fallback was four copies of one literal across two files, so the
    /// class is closed by scanning rather than by having fixed the four.
    #[test]
    fn no_crate_invents_its_own_unknown_error_text() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("workspace root");
        // Skipped for the same reason as the join gate above: this crate defines
        // the replacement, and its own test names the literal it forbids. The
        // exemption is checked rather than assumed.
        let owner = env!("CARGO_PKG_NAME");
        let owner_source =
            std::fs::read_to_string(root.join("crates").join(owner).join("src").join("lib.rs"))
                .expect("the crate that owns worker_failure_message");
        assert!(
            owner_source.contains("pub fn worker_failure_message"),
            "{owner} no longer defines worker_failure_message; move this exemption"
        );

        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(root.join("crates")).expect("crates directory") {
            let Ok(entry) = entry else { continue };
            if entry.file_name().to_str() == Some(owner) {
                continue;
            }
            let Ok(files) = std::fs::read_dir(entry.path().join("src")) else {
                continue;
            };
            for file in files.flatten() {
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                    continue;
                }
                let Ok(source) = std::fs::read_to_string(&path) else {
                    continue;
                };
                // Any invented `"unknown … error"` stand-in, not the one
                // spelling that was fixed. The first version of this gate
                // matched the literal `"unknown error"` and sat one file away
                // from `"unknown adapter error"`, which is the same defect
                // wearing one extra word.
                if source
                    .lines()
                    .filter(|line| !line.trim_start().starts_with("//"))
                    .any(|line| {
                        line.contains("\"unknown")
                            && line.contains("error")
                            && line.contains("unwrap_or")
                    })
                {
                    offenders.push(path.display().to_string());
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "use worker_failure_message instead:
  {}",
            offenders.join(
                "
  "
            )
        );
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
