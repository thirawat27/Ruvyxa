//! Terminal formatting helpers shared by the dev server's startup banner,
//! watcher logs, and diagnostics printing.

use std::io::IsTerminal;
use std::path::Path;

use chrono::Local;
use ruvyxa_middleware::MiddlewareConfig;

pub(crate) fn print_field(name: &str, value: String) {
    let padding = " ".repeat(20usize.saturating_sub(name.len()));
    println!("  {}{} {}", dim(name), padding, value);
}

pub(crate) fn current_timestamp() -> String {
    Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

pub(crate) fn enabled_text(enabled: bool) -> &'static str {
    if enabled { "on" } else { "off" }
}

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

pub(crate) fn heading(value: impl AsRef<str>) -> String {
    paint(value, "1;35")
}

pub(crate) fn accent(value: impl AsRef<str>) -> String {
    paint(value, "36")
}

pub(crate) fn dim(value: impl AsRef<str>) -> String {
    paint(value, "90")
}

pub(crate) fn ok(value: impl AsRef<str>) -> String {
    paint(value, "32")
}

pub(crate) fn warn_text(value: impl AsRef<str>) -> String {
    paint(value, "33")
}

pub(crate) fn link(value: impl AsRef<str>) -> String {
    paint(value, "34")
}

pub(crate) fn path_text(path: &Path) -> String {
    paint(path.display().to_string(), "34")
}

pub(crate) fn paint(value: impl AsRef<str>, code: &str) -> String {
    let value = value.as_ref();
    if !std::io::stdout().is_terminal()
        || std::env::var_os("NO_COLOR").is_some()
        || std::env::var("TERM")
            .map(|term| term.eq_ignore_ascii_case("dumb"))
            .unwrap_or(false)
    {
        return value.to_string();
    }

    format!("\x1b[{code}m{value}\x1b[0m")
}
