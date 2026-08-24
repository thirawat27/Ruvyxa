//! Terminal formatting for the dev server's startup banner, watcher logs, and
//! diagnostics printing.
//!
//! Colour and layout come from `ruvyxa_tui`, the same crate the CLI uses, so the
//! banner and the build summary share one palette and one label column. This
//! file previously carried its own copy of both; the copy is what let the two
//! drift to different field widths.
//!
//! What remains here is dev-server vocabulary: how a middleware set is
//! summarised for a human.

use ruvyxa_middleware::MiddlewareConfig;

pub(crate) use ruvyxa_tui::{
    accent, dim, enabled_text, info, link, note, number, ok_text as ok, paint, path_text,
    print_field, print_header, warn_text,
};

pub(crate) fn middleware_summary(config: &MiddlewareConfig) -> String {
    let mut enabled = Vec::new();

    if config.builtin.timing {
        enabled.push("timing");
    }
    if config.builtin.logging {
        enabled.push("logging");
    }
    if config.builtin.cors.is_some() {
        enabled.push("cors");
    }
    if config.builtin.rate_limit.is_some() {
        enabled.push("rate-limit");
    }
    if !config.builtin.headers.is_empty() {
        enabled.push("headers");
    }
    if enabled.is_empty() {
        "none".to_string()
    } else {
        enabled.join(", ")
    }
}
