//! Command-line argument normalization.
//!
//! Clap parses the canonical spelling of every flag and command. This module
//! rewrites common variants into that spelling first — `--root=x`, a `—root`
//! typed with an em dash by a shell or editor, `test-parity` for `test:parity`
//! — so a user is not stopped by a form that is unambiguous to a reader.
//!
//! Normalization is deliberately narrow: it maps only spellings that already
//! resolve to exactly one known option or command. Anything else passes through
//! untouched, so clap produces its own error instead of this module guessing at
//! intent.

use std::ffi::OsString;

pub(crate) fn normalized_cli_args(args: impl IntoIterator<Item = OsString>) -> Vec<OsString> {
    let mut args = args.into_iter().collect::<Vec<_>>();
    normalize_option_args(&mut args);

    if let Some(command_index) = first_command_arg_index(&args) {
        normalize_command_arg(&mut args, command_index);

        if args[command_index] == "help"
            && let Some(help_target_index) = first_command_arg_index(&args[command_index..])
        {
            normalize_command_arg(&mut args, command_index + help_target_index);
        }
    }

    args
}

pub(crate) fn normalize_option_args(args: &mut [OsString]) {
    for arg in args.iter_mut().skip(1) {
        let Some(normalized) = normalized_option_arg(arg) else {
            continue;
        };

        *arg = OsString::from(normalized);
    }
}

/// The text after an option's leading dashes, however they were typed.
///
/// `--` is the canonical spelling. The other two are what a text editor or a
/// chat client leaves behind after "smart" punctuation has rewritten a typed
/// double hyphen: U+2014 EM DASH, and U+2013 EN DASH for the shorter
/// substitution some of them make. A user who pastes a command out of a
/// document is otherwise told the argument does not exist, which is true and
/// unhelpful.
///
/// The rewrite is safe only because the caller gates it: `canonical_option_name`
/// has to recognise what follows, so an ordinary positional argument that
/// happens to begin with a dash of any kind passes through untouched and clap
/// answers for it.
fn option_body(arg: &str) -> Option<&str> {
    const EM_DASH: &str = "\u{2014}";
    const EN_DASH: &str = "\u{2013}";
    arg.strip_prefix("--")
        .or_else(|| arg.strip_prefix(EM_DASH))
        .or_else(|| arg.strip_prefix(EN_DASH))
}

pub(crate) fn normalized_option_arg(arg: &OsString) -> Option<String> {
    let arg = arg.to_str()?;

    if arg.eq_ignore_ascii_case("-h") {
        return Some("-h".to_string());
    }

    let option = option_body(arg)?;
    let (name, value) = option
        .split_once('=')
        .map_or((option, None), |(name, value)| (name, Some(value)));
    let canonical = canonical_option_name(name)?;

    Some(match value {
        Some(value) => format!("--{canonical}={value}"),
        None => format!("--{canonical}"),
    })
}

pub(crate) fn canonical_option_name(option: &str) -> Option<&'static str> {
    match option.to_ascii_lowercase().as_str() {
        "help" => Some("help"),
        "root" => Some("root"),
        "host" => Some("host"),
        "port" => Some("port"),
        "target" => Some("target"),
        "runtime" => Some("runtime"),
        "adapter" => Some("adapter"),
        // clap's canonical spelling is the hyphenated one; the underscored
        // form matches the Rust field name a reader may have seen in docs.
        "server-only" | "server_only" => Some("server-only"),
        "format" => Some("format"),
        "output" => Some("output"),
        "samples" => Some("samples"),
        "json" => Some("json"),
        "html" => Some("html"),
        _ => None,
    }
}

pub(crate) fn first_command_arg_index(args: &[OsString]) -> Option<usize> {
    for (index, arg) in args.iter().enumerate().skip(1) {
        let arg = arg.to_string_lossy();

        if arg == "--" {
            return None;
        }

        if arg.starts_with('-') {
            continue;
        }

        return Some(index);
    }

    None
}

pub(crate) fn normalize_command_arg(args: &mut [OsString], index: usize) {
    let Some(command) = args[index].to_str() else {
        return;
    };
    let Some(canonical) = canonical_command_name(command) else {
        return;
    };

    args[index] = OsString::from(canonical);
}

pub(crate) fn canonical_command_name(command: &str) -> Option<&'static str> {
    match command.to_ascii_lowercase().as_str() {
        "dev" => Some("dev"),
        "build" => Some("build"),
        "check" => Some("check"),
        "start" => Some("start"),
        "preview" => Some("preview"),
        "routes" => Some("routes"),
        "analyze" => Some("analyze"),
        "doctor" => Some("doctor"),
        "clean" => Some("clean"),
        "trace" => Some("trace"),
        "bench" => Some("bench"),
        "test:parity" => Some("test:parity"),
        // The module doc has promised this spelling since it was written, and
        // it was the one entry of the sixteen that did not exist. A colon is
        // awkward to type in some shells and needs quoting in others, so the
        // hyphenated form is what a user reaches for.
        "test-parity" => Some("test:parity"),
        "parity" => Some("parity"),
        "adds" => Some("adds"),
        "help" => Some("help"),
        _ => None,
    }
}
