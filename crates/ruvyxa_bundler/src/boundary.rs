//! Server/Client boundary enforcement.
//!
//! Mirrors the rules implemented in `client-renderer.mjs` and
//! `ruvyxa_graph::validate_client_module`, but operates directly on the
//! compiled module graph and emits structured [`Diagnostic`] values.
//!
//! Rules enforced:
//! - **RUV1007** – `"server-only"` import reachable from a client bundle.
//! - **RUV1008** – Private `process.env.*` variable read in a client bundle.
//! - **RUV1010** – File inside `server/` directory reachable by a client graph.

use std::path::Path;

use ruvyxa_diagnostics::Diagnostic;

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
    if input.target == BundleTarget::Ssr {
        // SSR bundles run on Node.js – enforce only the client-only rule.
        for module in modules {
            check_ssr_module(module, out)?;
        }
        return Ok(());
    }

    // Client bundles: enforce all three rules.
    for module in modules {
        check_client_module(module, &input.project_root, out)?;
    }

    Ok(())
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
    if source.contains("\"server-only\"") || source.contains("'server-only'") {
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
    if source.contains("\"client-only\"") || source.contains("'client-only'") {
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

fn is_inside_server_dir(path: &Path, project_root: &Path) -> bool {
    let Ok(rel) = path.strip_prefix(project_root) else {
        return false;
    };
    rel.components().any(|c| c.as_os_str() == "server")
}

/// Scan source text for `process.env.VAR_NAME` reads that are not
/// `RUVYXA_PUBLIC_*` or `NODE_ENV`.
fn find_private_env_reads(source: &str) -> Vec<String> {
    let mut names = Vec::new();
    let marker = "process.env.";
    let mut rest = source;

    while let Some(idx) = rest.find(marker) {
        rest = &rest[idx + marker.len()..];
        let name: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect();

        if !name.is_empty() && name != "NODE_ENV" && !name.starts_with("RUVYXA_PUBLIC_") {
            names.push(name);
        }
    }

    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_private_env_reads() {
        let source =
            "const db = process.env.DATABASE_URL; const pub = process.env.RUVYXA_PUBLIC_API;";
        let names = find_private_env_reads(source);
        assert_eq!(names, vec!["DATABASE_URL"]);
    }

    #[test]
    fn allows_public_env_and_node_env() {
        let source = "if (process.env.NODE_ENV === 'production') {}";
        assert!(find_private_env_reads(source).is_empty());
    }
}
