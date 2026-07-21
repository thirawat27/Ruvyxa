//! Server/Client boundary enforcement.
//!
//! Mirrors the rules implemented in `compiler.mjs` (`checkClientBoundary`) and
//! `ruvyxa_graph::validate_client_module`, but operates directly on the
//! compiled module graph and emits structured [`Diagnostic`] values.
//!
//! Rules enforced:
//! - **RUV1007** – `"server-only"` import reachable from a client bundle.
//! - **RUV1008** – Private `process.env.*` variable read in a client bundle.
//! - **RUV1010** – File inside `server/` directory reachable by a client graph.

use std::path::Path;

use ruvyxa_diagnostics::Diagnostic;

use crate::ast;
use crate::compiler::CompiledModule;
use crate::{BundleInput, BundleTarget, Result};

/// Check all compiled modules for server/client boundary violations.
///
/// Non-fatal diagnostics are appended to `out`; hard violations (those that
/// would produce broken output) are returned as [`BundleError::Diagnostic`].
pub fn check(
    modules: &[CompiledModule],
    input: &BundleInput,
    out: &mut Vec<Diagnostic>,
) -> Result<()> {
    if matches!(input.target, BundleTarget::Ssr | BundleTarget::Edge) {
        // SSR/Edge bundles run on the server – enforce only the client-only rule.
        for module in modules {
            check_ssr_module(module, out)?;
        }
        return Ok(());
    }

    // Client bundles: enforce all three rules. Keep scanning after the
    // first hard violation so one build reports every affected module
    // instead of surfacing them one fix-and-rebuild cycle at a time.
    let mut first_error = None;
    for module in modules {
        if let Err(error) = check_client_module(module, &input.project_root, out)
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }

    match first_error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn check_client_module(
    module: &CompiledModule,
    project_root: &Path,
    out: &mut Vec<Diagnostic>,
) -> Result<()> {
    if module.is_external {
        return Ok(());
    }

    let source = &module.js;

    // RUV1007 – "server-only" import
    if imports_marker(source, "server-only") {
        return Err(Diagnostic::new(
            "RUV1007",
            "Server-only module imported into client bundle",
        )
        .explain(
            "This module is reachable from the browser hydration bundle but declares `server-only`.",
        )
        .at_file(&module.path)
        .suggest(
            "Move server-only code behind a loader/API route, or pass serialized data to the page.",
        )
        .into());
    }

    // RUV1010 – server/ directory in client graph
    if is_inside_server_dir(&module.path, project_root) {
        return Err(Diagnostic::new(
            "RUV1010",
            "Server directory module reached by client graph",
        )
        .explain("Files under server/ are reserved for server-only code.")
        .at_file(&module.path)
        .suggest(
            "Move shared browser-safe code outside server/, or import it from a server route only.",
        )
        .into());
    }

    // RUV1008 – private env var reads (non-fatal: recorded as diagnostic)
    for var_name in find_private_env_reads(source) {
        out.push(
            Diagnostic::new(
                "RUV1008",
                "Private environment variable used in client bundle",
            )
            .explain(format!(
                "`process.env.{var_name}` is reachable from browser code. \
                 Only `RUVYXA_PUBLIC_*` env vars may be exposed to client modules."
            ))
            .at_file(&module.path)
            .suggest(format!(
                "Rename `{var_name}` to `RUVYXA_PUBLIC_{var_name}` if it is safe to expose, \
                 or move the env read into server-only code."
            )),
        );
    }

    Ok(())
}

fn check_ssr_module(module: &CompiledModule, out: &mut Vec<Diagnostic>) -> Result<()> {
    if module.is_external {
        return Ok(());
    }

    let source = &module.js;

    // client-only import in SSR graph
    if imports_marker(source, "client-only") {
        out.push(
            Diagnostic::new("RUV1009", "Client-only module imported into SSR graph")
                .explain(
                    "This module is reachable from server runtime code but declares `client-only`.",
                )
                .at_file(&module.path)
                .suggest("Move browser-only code into a client component or client.tsx module."),
        );
    }

    Ok(())
}

fn imports_marker(source: &str, marker: &str) -> bool {
    ast::parse_module(source)
        .imports
        .iter()
        .any(|edge| edge.specifier == marker)
}

fn is_inside_server_dir(path: &Path, project_root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(project_root) else {
        return false;
    };
    rel.components()
        .next()
        .is_some_and(|component| component.as_os_str() == "server")
}

/// Scan source text for statically-known `process.env` reads that are not
/// `RUVYXA_PUBLIC_*` or `NODE_ENV`.
fn find_private_env_reads(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut index = 0;
    scan_code_for_env_reads(source.as_bytes(), &mut index, 0, &mut names);

    names
}

fn scan_code_for_env_reads(
    source: &[u8],
    index: &mut usize,
    mut template_expression_depth: usize,
    names: &mut Vec<String>,
) {
    // Index of the last byte that can end a JavaScript token. A `/` is only a
    // regular expression when no value precedes it; without this the scanner
    // treats `/["']/` as a division followed by an unterminated string and
    // silently skips the rest of the module, hiding every later env read.
    let mut previous_significant: Option<usize> = None;

    while *index < source.len() {
        let start = *index;
        match source[*index] {
            b'\'' | b'"' => {
                skip_quoted_bytes(source, index);
                previous_significant = Some(start);
            }
            b'`' => {
                skip_template_literal(source, index, names);
                previous_significant = Some(start);
            }
            b'/' if source.get(*index + 1) == Some(&b'/') => skip_line_comment_bytes(source, index),
            b'/' if source.get(*index + 1) == Some(&b'*') => {
                skip_block_comment_bytes(source, index)
            }
            b'/' if regex_can_start(source, previous_significant) => {
                skip_regex_literal(source, index);
                previous_significant = Some(start);
            }
            b'{' if template_expression_depth > 0 => {
                template_expression_depth += 1;
                *index += 1;
                previous_significant = Some(start);
            }
            b'}' if template_expression_depth > 0 => {
                template_expression_depth -= 1;
                *index += 1;
                if template_expression_depth == 0 {
                    return;
                }
                previous_significant = Some(start);
            }
            _ if starts_env_read(source, *index) => {
                if let Some(name) = parse_env_name(source, *index + b"process.env".len())
                    && name != "NODE_ENV"
                    && !name.starts_with("RUVYXA_PUBLIC_")
                {
                    names.push(name);
                }
                *index += b"process.env".len();
                previous_significant = Some(*index - 1);
            }
            byte => {
                if !byte.is_ascii_whitespace() {
                    previous_significant = Some(start);
                }
                *index += 1;
            }
        }
    }
}

/// Decide whether a `/` opens a regular expression rather than a division.
///
/// A regex may only appear where a value is expected. When the preceding token
/// could end a value (identifier, number, string, closing bracket) the slash is
/// division. Keywords such as `return` are values-expected positions.
fn regex_can_start(source: &[u8], previous_significant: Option<usize>) -> bool {
    let Some(index) = previous_significant else {
        return true;
    };
    match source[index] {
        b')' | b']' | b'}' | b'\'' | b'"' | b'`' => false,
        byte if is_identifier_byte(byte) => previous_token_is_keyword(source, index),
        _ => true,
    }
}

fn previous_token_is_keyword(source: &[u8], end: usize) -> bool {
    let mut start = end + 1;
    while start > 0 && is_identifier_byte(source[start - 1]) {
        start -= 1;
    }
    matches!(
        std::str::from_utf8(&source[start..=end]).unwrap_or_default(),
        "await"
            | "case"
            | "delete"
            | "do"
            | "else"
            | "in"
            | "instanceof"
            | "new"
            | "of"
            | "return"
            | "throw"
            | "typeof"
            | "void"
            | "yield"
    )
}

fn skip_regex_literal(source: &[u8], index: &mut usize) {
    *index += 1;
    let mut inside_character_class = false;
    while *index < source.len() {
        match source[*index] {
            b'\\' => *index = (*index + 2).min(source.len()),
            b'[' => {
                inside_character_class = true;
                *index += 1;
            }
            b']' if inside_character_class => {
                inside_character_class = false;
                *index += 1;
            }
            // An unterminated literal was a division after all. Stop here so the
            // rest of the line is still scanned normally.
            b'\n' => return,
            b'/' if !inside_character_class => {
                *index += 1;
                break;
            }
            _ => *index += 1,
        }
    }

    while source
        .get(*index)
        .is_some_and(|byte| is_identifier_byte(*byte))
    {
        *index += 1;
    }
}

fn starts_env_read(source: &[u8], index: usize) -> bool {
    const MARKER: &[u8] = b"process.env";
    source.get(index..index + MARKER.len()) == Some(MARKER)
        && source
            .get(index.wrapping_sub(1))
            .is_none_or(|previous| !is_identifier_byte(*previous) && *previous != b'.')
}

fn parse_env_name(source: &[u8], mut index: usize) -> Option<String> {
    while source.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }

    if source.get(index) == Some(&b'.') {
        index += 1;
        let start = index;
        while source
            .get(index)
            .is_some_and(|byte| is_identifier_byte(*byte))
        {
            index += 1;
        }
        return std::str::from_utf8(&source[start..index])
            .ok()
            .filter(|name| !name.is_empty())
            .map(str::to_owned);
    }

    if source.get(index) != Some(&b'[') {
        return None;
    }
    index += 1;
    while source.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    let quote = *source.get(index)?;
    if quote != b'\'' && quote != b'"' {
        return None;
    }
    index += 1;
    let start = index;
    while source
        .get(index)
        .is_some_and(|byte| is_identifier_byte(*byte))
    {
        index += 1;
    }
    let name = std::str::from_utf8(&source[start..index])
        .ok()
        .filter(|name| !name.is_empty())?
        .to_owned();
    if source.get(index) != Some(&quote) {
        return None;
    }
    index += 1;
    while source.get(index).is_some_and(u8::is_ascii_whitespace) {
        index += 1;
    }
    (source.get(index) == Some(&b']')).then_some(name)
}

fn skip_quoted_bytes(source: &[u8], index: &mut usize) {
    let quote = source[*index];
    *index += 1;
    while *index < source.len() {
        match source[*index] {
            b'\\' => *index = (*index + 2).min(source.len()),
            byte if byte == quote => {
                *index += 1;
                return;
            }
            _ => *index += 1,
        }
    }
}

fn skip_template_literal(source: &[u8], index: &mut usize, names: &mut Vec<String>) {
    *index += 1;
    while *index < source.len() {
        match source[*index] {
            b'\\' => *index = (*index + 2).min(source.len()),
            b'`' => {
                *index += 1;
                return;
            }
            b'$' if source.get(*index + 1) == Some(&b'{') => {
                *index += 2;
                scan_code_for_env_reads(source, index, 1, names);
            }
            _ => *index += 1,
        }
    }
}

fn skip_line_comment_bytes(source: &[u8], index: &mut usize) {
    *index += 2;
    while source.get(*index).is_some_and(|byte| *byte != b'\n') {
        *index += 1;
    }
}

fn skip_block_comment_bytes(source: &[u8], index: &mut usize) {
    *index += 2;
    while *index + 1 < source.len() {
        if source[*index] == b'*' && source[*index + 1] == b'/' {
            *index += 2;
            return;
        }
        *index += 1;
    }
    *index = source.len();
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regex_literals_do_not_hide_later_env_reads() {
        // A quote inside a regex character class used to start a string skip
        // that ran to end-of-file, so every later private env read went
        // unreported and could reach the browser bundle unnoticed.
        let source = "const re = /[\"']/g; const db = process.env.DATABASE_URL;";
        assert_eq!(find_private_env_reads(source), vec!["DATABASE_URL"]);

        let source = r#"if (/^a\/b$/.test(x)) {} const key = process.env['API_KEY'];"#;
        assert_eq!(find_private_env_reads(source), vec!["API_KEY"]);
    }

    #[test]
    fn division_is_not_mistaken_for_a_regex_literal() {
        let source = "const ratio = total / count; const db = process.env.DATABASE_URL;";
        assert_eq!(find_private_env_reads(source), vec!["DATABASE_URL"]);

        let source = "const ratio = (a + b) / 2 / 4; const key = process.env.API_KEY;";
        assert_eq!(find_private_env_reads(source), vec!["API_KEY"]);
    }

    #[test]
    fn regex_after_a_keyword_is_still_a_regex() {
        let source = "function f() { return /['\"]/.source } const db = process.env.DATABASE_URL;";
        assert_eq!(find_private_env_reads(source), vec!["DATABASE_URL"]);
    }

    #[test]
    fn detects_private_env_reads() {
        let source = "const db = process.env.DATABASE_URL; const pub = process.env.RUVYXA_PUBLIC_API; const key = process.env['API_KEY'];";
        let names = find_private_env_reads(source);
        assert_eq!(names, vec!["DATABASE_URL", "API_KEY"]);
    }

    #[test]
    fn allows_public_env_and_node_env() {
        let source = "if (process.env.NODE_ENV === 'production') {}";
        assert!(find_private_env_reads(source).is_empty());
    }

    #[test]
    fn ignores_env_text_in_comments_and_strings_but_keeps_template_expressions() {
        let source = r#"
            const docs = "process.env.DATABASE_URL";
            // process.env.API_KEY
            const rendered = `${process.env.DATABASE_URL}`;
        "#;

        assert_eq!(find_private_env_reads(source), vec!["DATABASE_URL"]);
    }

    #[test]
    fn reserves_only_the_project_level_server_directory() {
        let root = Path::new("/project");
        assert!(is_inside_server_dir(
            Path::new("/project/server/secret.ts"),
            root
        ));
        assert!(!is_inside_server_dir(
            Path::new("/project/app/server/page.tsx"),
            root
        ));
    }

    #[test]
    fn only_treats_actual_imports_as_boundary_markers() {
        assert!(!imports_marker(
            "export const documentation = 'Use server-only modules for secrets.';",
            "server-only"
        ));
        assert!(imports_marker("import 'server-only';", "server-only"));
    }
}
