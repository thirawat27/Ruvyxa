//! `.env` / `.env.local` loading for project config and JavaScript runtimes.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use ruvyxa_diagnostics::{Result, RuvyxaError};

/// Loads `.env` and `.env.local` from the project root, later files winning.
pub fn project_env(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut values = BTreeMap::new();

    for file_name in [".env", ".env.local"] {
        let file = root.join(file_name);
        if !file.exists() {
            continue;
        }

        let source = fs::read_to_string(&file).map_err(|source| RuvyxaError::Io {
            message: format!("Failed to read {}", file.display()),
            source,
        })?;

        for (key, value) in parse_env_source(&source) {
            values.insert(key, value);
        }
    }

    Ok(values)
}

/// Parse `.env` text into assignments.
///
/// A value is not a line. This walked `source.lines()` and unquoted within one
/// line, so a `.env` holding a PEM key — routine for the auth and deploy
/// integrations this framework ships — set the variable to the opening fence
/// with its quote still attached, and then read the base64 body, which contains
/// `=`, as further assignments. Those junk names went into every worker
/// process and folded into `build_dependency_hash` and the artifact cache key.
/// A quoted value that does not close on its line is continued until it does.
pub(crate) fn parse_env_source(source: &str) -> BTreeMap<String, String> {
    let mut values = BTreeMap::new();
    let lines = source.lines().collect::<Vec<_>>();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index].trim();
        index += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        // dotenv-style `export KEY=value`: files written for `source` keep
        // the shell prefix, which is not part of the variable name. A literal
        // `export=value` line still assigns the key `export`.
        let line = line
            .strip_prefix("export ")
            .or_else(|| line.strip_prefix("export\t"))
            .map(str::trim_start)
            .unwrap_or(line);

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        let key = key.trim();
        if key.is_empty() {
            continue;
        }

        let (value, consumed) = read_env_value(value.trim_start(), &lines[index..]);
        index += consumed;
        values.insert(key.to_string(), value);
    }

    values
}

/// The value an assignment carries, and how many further lines it took.
///
/// `first` is everything after the `=` with leading whitespace removed;
/// `rest` is the lines below it, in order.
fn read_env_value(first: &str, rest: &[&str]) -> (String, usize) {
    let Some(quote) = first
        .chars()
        .next()
        .filter(|char| *char == '"' || *char == '\'')
    else {
        return (strip_unquoted_comment(first), 0);
    };
    let body = &first[quote.len_utf8()..];

    if let Some(end) = find_closing_quote(body, quote) {
        // Anything after the closing quote on the same line is a comment or
        // nothing. When it is neither, the line is malformed in a way the
        // line-based parser used to hand back raw, so hand it back raw rather
        // than inventing a truncation.
        let trailing = body[end + quote.len_utf8()..].trim();
        if trailing.is_empty() || trailing.starts_with('#') {
            return (body[..end].to_string(), 0);
        }
        return (first.trim_end().to_string(), 0);
    }

    let mut buffer = body.to_string();
    for (offset, line) in rest.iter().enumerate() {
        buffer.push('\n');
        if let Some(end) = find_closing_quote(line, quote) {
            buffer.push_str(&line[..end]);
            return (buffer, offset + 1);
        }
        buffer.push_str(line);
    }

    // No closing quote anywhere below. Consuming to end-of-file would turn one
    // stray quote into "every variable under it disappeared", so this keeps the
    // single-line answer — quote included, exactly as before — and lets the
    // lines below be parsed normally.
    (first.trim_end().to_string(), 0)
}

/// The byte offset of the unescaped `quote` in `text`, if it has one.
///
/// A backslash escapes the next byte inside a double-quoted value, so
/// `KEY="a\"b"` keeps closing where it always did. Single quotes carry no
/// escapes, in shells and in dotenv alike.
fn find_closing_quote(text: &str, quote: char) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if quote == '"' && byte == b'\\' {
            index += 2;
            continue;
        }
        if byte == quote as u8 {
            return Some(index);
        }
        index += 1;
    }
    None
}

/// Drop a trailing `# comment` from an unquoted value.
///
/// Only a `#` that opens the value or follows whitespace starts a comment, so
/// `HASH=abc#def` keeps its `#` the way every dotenv implementation does.
fn strip_unquoted_comment(value: &str) -> String {
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        if *byte != b'#' {
            continue;
        }
        if index == 0 || bytes[index - 1].is_ascii_whitespace() {
            return value[..index].trim_end().to_string();
        }
    }
    value.trim_end().to_string()
}
