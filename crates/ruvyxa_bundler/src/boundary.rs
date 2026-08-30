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

use std::collections::BTreeSet;
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
    //
    // Deliberately sequential. Every rule now reads a short list off the
    // module's already-parsed AST, so a module check is a handful of
    // comparisons; rayon would cost more to schedule than it saves, and the
    // sequential walk is what makes "first error in module order" and the
    // diagnostic order in `out` stable across runs.
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

    // RUV1007 – "server-only" import
    if module
        .ast
        .imports
        .iter()
        .any(|edge| is_server_only_specifier(&edge.specifier))
    {
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
    //
    // One diagnostic per *name*, not per read. `private_env_reads` reports
    // occurrences in source order and unfiltered, because that is the shared
    // extraction contract the JavaScript half is held to; the conclusion drawn
    // from it here is about the variable, and repeating it once per mention
    // made the report longest exactly where the reader most needs to see the
    // set of leaked names. First-seen order is kept rather than sorting, so the
    // list still reads down the file — and it is deterministic either way,
    // which matters because these render into strings the shared-chunk artifact
    // cache stores.
    let mut already_reported = BTreeSet::new();
    for var_name in private_env_reads(&module.ast) {
        if !already_reported.insert(var_name.clone()) {
            continue;
        }
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

    // client-only import in SSR graph
    if imports_marker(&module.ast, "client-only") {
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

fn imports_marker(module: &ast::ModuleAst, marker: &str) -> bool {
    module.imports.iter().any(|edge| edge.specifier == marker)
}

fn is_server_only_specifier(specifier: &str) -> bool {
    matches!(
        specifier,
        "server-only" | "@ruvyxa/auth" | "@ruvyxa/database"
    )
}

fn is_inside_server_dir(path: &Path, project_root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(project_root) else {
        return false;
    };
    rel.components()
        .next()
        .is_some_and(|component| component.as_os_str() == "server")
}

/// Whether a `process.env` name is one a browser bundle must not carry.
///
/// `NODE_ENV` is substituted with a literal at build time and never reaches the
/// output as an env read; `RUVYXA_PUBLIC_*` is public by contract. Everything
/// else statically named is a leak.
///
/// This predicate is the single home of that policy. [`ast`] deliberately keeps
/// policy out of the scanner and leaves it to this module — but "this module"
/// had become two: `ruvyxa_graph` carried its own copy for the `check` and
/// `analyze` diagnostics, and the copy was missing the `NODE_ENV` exemption. A
/// client component containing the most ordinary line in React —
/// `process.env.NODE_ENV !== 'production'` — therefore built fine and failed
/// validation with RUV1008. Sharing the fact (`env_reads`) while duplicating the
/// rule that reads it left exactly the disagreement the shared scanner exists to
/// prevent.
#[must_use]
pub fn env_read_is_private(name: &str) -> bool {
    name != "NODE_ENV" && !name.starts_with("RUVYXA_PUBLIC_")
}

/// The `process.env` reads in `module` that must not reach a browser bundle.
///
/// Which reads exist is decided by the one scanner in [`ast`]; this function
/// only applies [`env_read_is_private`]. It used to walk the bytes itself, and
/// that second walk is exactly where a regex-literal bug hid every env read in a
/// module twice over.
fn private_env_reads(module: &ast::ModuleAst) -> Vec<String> {
    module
        .env_reads
        .iter()
        .filter(|name| env_read_is_private(name))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The private-env policy has two implementations — this one, which
    /// `ruvyxa_graph` also calls, and `envReadIsPrivate` in
    /// `packages/ruvyxa/runtime/compiler.mjs`. They decide which secrets may be
    /// serialized into a browser bundle, they cannot share code across the
    /// language boundary, and the rule has already drifted once.
    ///
    /// `tests/packages/ruvyxa/env-policy.test.mjs` replays the same file through
    /// the Node half. A change made in one language and not the other fails here.
    #[test]
    fn matches_the_shared_cross_language_env_policy_table() {
        let fixture = env_policy_fixture();

        let cases = fixture["cases"].as_array().expect("fixture declares cases");
        assert!(!cases.is_empty(), "the fixture must carry cases");
        for case in cases {
            let name = case["name"].as_str().expect("case name");
            let expected = case["private"].as_bool().expect("case verdict");
            let why = case["why"].as_str().unwrap_or("");
            assert_eq!(
                env_read_is_private(name),
                expected,
                "process.env.{name} — {why}"
            );
        }
    }

    fn env_policy_fixture() -> serde_json::Value {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/env-policy-conformance.json");
        serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("conformance fixture is valid JSON")
    }

    /// The other half of the same policy: which name a read *is*.
    ///
    /// Classifying correctly is worth nothing while the two graphs extract
    /// different names from the same source, and they did — `env_read_name`
    /// reads the name with `skip_identifier` while `parsePrivateEnvName` in
    /// `packages/ruvyxa/runtime/compiler.mjs` matched upper-case only, so
    /// `process.env.databaseUrl` was invisible to `ruvyxa dev` and refused by
    /// `ruvyxa build`. `tests/packages/ruvyxa/env-policy.test.mjs` replays the
    /// same table through the Node half.
    #[test]
    fn matches_the_shared_cross_language_env_extraction_table() {
        let fixture = env_policy_fixture();
        let cases = fixture["extraction"]["cases"]
            .as_array()
            .expect("fixture declares extraction cases");
        assert!(!cases.is_empty(), "the fixture must carry extraction cases");
        for case in cases {
            let source = case["source"].as_str().expect("case source");
            let expected: Vec<&str> = case["privateNames"]
                .as_array()
                .expect("case names")
                .iter()
                .map(|name| name.as_str().expect("case name"))
                .collect();
            let why = case["why"].as_str().unwrap_or("");
            assert_eq!(
                find_private_env_reads(source),
                expected,
                "{source:?} — {why}"
            );
        }
    }

    /// Every name the classification table judges must be a name the extractor
    /// can produce, or the row tests nothing end to end. Two of them —
    /// `node_env` and `ruvyxa_public_key` — were unreachable from any source
    /// through the Node graph, which is how the extraction divergence stayed
    /// invisible while the classification table passed on both sides.
    #[test]
    fn every_classified_name_is_a_name_the_scanner_can_extract() {
        let fixture = env_policy_fixture();
        for case in fixture["cases"].as_array().expect("fixture declares cases") {
            let name = case["name"].as_str().expect("case name");
            // The empty name is the one deliberate exception: neither graph
            // reports a zero-length name, so that row exists only as a unit of
            // the predicate.
            if name.is_empty() || !case["private"].as_bool().expect("case verdict") {
                continue;
            }
            let source = format!("export const value = process.env[{name:?}]\n");
            assert_eq!(
                find_private_env_reads(&source),
                vec![name.to_owned()],
                "no source can spell {name}, so the classification case is untested end to end"
            );
        }
    }

    /// Parse then filter, the same two steps the boundary check performs.
    ///
    /// These cases predate the scanner merge and are kept whole: they are the
    /// regressions that a second byte scanner introduced, and they must still
    /// hold now that the answer comes from `ast`.
    fn find_private_env_reads(source: &str) -> Vec<String> {
        private_env_reads(&ast::parse_module(source))
    }

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

    /// RUV1008 answers *which* private names a module leaks, not how many times
    /// each one is written.
    ///
    /// One name is one conclusion, so a module reading `DATABASE_URL` in three
    /// places produced three byte-identical diagnostics — and the noise scales
    /// with how badly the module violates the rule, so the output is least
    /// readable exactly where the reader most needs the *set* of leaked names.
    /// These are carried as rendered strings through the shared-chunk artifact
    /// cache, so the duplication was stored as well as printed.
    ///
    /// First-seen order, not sorted: the diagnostic list is what the reader
    /// walks against the file, and `private_env_reads` reports in source order.
    #[test]
    fn a_private_name_read_more_than_once_is_reported_once() {
        let source = concat!(
            "const a = process.env.DATABASE_URL;\n",
            "const b = process.env.API_KEY;\n",
            "const c = process.env.DATABASE_URL;\n",
            "const d = process.env['API_KEY'];\n",
            "const e = process.env.DATABASE_URL;\n",
        );
        let module = crate::compiler::CompiledModule::new(
            Path::new("/project/app/page.js").to_path_buf(),
            source.to_string(),
            Vec::new(),
            std::collections::BTreeMap::new(),
            false,
            false,
        );

        let mut out = Vec::new();
        check_client_module(&module, Path::new("/project"), &mut out)
            .expect("a private env read is non-fatal");

        let reported: Vec<&str> = out
            .iter()
            .inspect(|diagnostic| assert_eq!(diagnostic.code, "RUV1008"))
            .map(|diagnostic| {
                if diagnostic.explanation.contains("DATABASE_URL") {
                    "DATABASE_URL"
                } else {
                    "API_KEY"
                }
            })
            .collect();
        assert_eq!(reported, ["DATABASE_URL", "API_KEY"]);
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
            &ast::parse_module(
                "export const documentation = 'Use server-only modules for secrets.';"
            ),
            "server-only"
        ));
        assert!(imports_marker(
            &ast::parse_module("import 'server-only';"),
            "server-only"
        ));
        assert!(is_server_only_specifier("@ruvyxa/auth"));
        assert!(is_server_only_specifier("@ruvyxa/database"));
        assert!(!is_server_only_specifier("@ruvyxa/auth/client"));
    }
}
