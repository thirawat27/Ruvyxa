//! Plugin scaffolding and command-line argument normalization.
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
use std::fs;

use crate::*;

pub(crate) fn plugin(args: PluginArgs) -> anyhow::Result<()> {
    match args.command {
        PluginCommand::Create(args) => scaffold_plugin(args),
    }
}

pub(crate) const PLUGIN_TEMPLATE_FILES: &[(&str, &str)] = &[
    (
        "src/index.ts",
        include_str!("../../../templates/plugin/src/index.ts"),
    ),
    (
        "test/plugin.test.mjs",
        include_str!("../../../templates/plugin/test/plugin.test.mjs"),
    ),
    (
        "package.json",
        include_str!("../../../templates/plugin/package.json"),
    ),
    (
        "tsconfig.json",
        include_str!("../../../templates/plugin/tsconfig.json"),
    ),
    (
        "README.md",
        include_str!("../../../templates/plugin/README.md"),
    ),
    (
        ".gitignore",
        include_str!("../../../templates/plugin/.gitignore"),
    ),
];

pub(crate) fn scaffold_plugin(args: PluginCreateArgs) -> anyhow::Result<()> {
    let plugin_name = normalize_plugin_name(&args.name)?;
    let package_dir = match &args.dir {
        Some(dir) => {
            if dir.as_os_str().is_empty() {
                anyhow::bail!("--dir must not be empty");
            }
            if dir
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
            {
                anyhow::bail!("--dir must not contain `..` components: {}", dir.display());
            }
            if dir.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::Prefix(_) | std::path::Component::RootDir
                )
            }) {
                anyhow::bail!(
                    "--dir must be relative to --root without a drive or root prefix: {}",
                    dir.display()
                );
            }
            args.root.join(dir)
        }
        None => args.root.join(&plugin_name),
    };
    if package_dir.exists() {
        anyhow::bail!(
            "plugin package already exists: {}; choose a different name or remove it first",
            package_dir.display()
        );
    }

    let plugin_identifier = plugin_name.replace('-', "_");
    for (relative_path, template) in PLUGIN_TEMPLATE_FILES {
        let destination = package_dir.join(relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        let contents = template
            .replace("__PLUGIN_NAME__", &plugin_name)
            .replace("__PLUGIN_IDENTIFIER__", &plugin_identifier)
            .replace("__RUVYXA_VERSION__", env!("CARGO_PKG_VERSION"));
        fs::write(destination, contents)?;
    }

    print_header("Plugin");
    print_field("status", ok_text("created"));
    print_field("plugin", accent(&plugin_name));
    print_field("package", accent(format!("ruvyxa-plugin-{plugin_name}")));
    print_field("path", path_text(&package_dir));
    println!();
    println!("  {}", path_text(&package_dir));
    println!("  {} package.json", dim("├─"));
    println!("  {} README.md", dim("├─"));
    println!("  {} tsconfig.json", dim("├─"));
    println!("  {} test/plugin.test.mjs", dim("├─"));
    println!("  {} src", dim("└─"));
    println!("     {} {}", dim("└─"), accent("index.ts"));
    println!();
    println!("  {}", label("next steps"));
    println!(
        "  {} {}",
        dim("1."),
        accent(format!("cd {}", package_dir.display()))
    );
    println!(
        "  {} {}",
        dim("2."),
        accent("npm install  (or: pnpm install, bun install)")
    );
    println!(
        "  {} {}",
        dim("3."),
        accent("npm test  (or: pnpm test, bun test)")
    );
    println!(
        "  {} {}",
        dim("4."),
        dim("Start with headers; add direct sections only as the plugin grows.")
    );
    println!();
    println!(
        "  {} Plugin {} is ready to develop\n",
        success(),
        accent(&plugin_name)
    );
    Ok(())
}

pub(crate) fn normalize_plugin_name(value: &str) -> anyhow::Result<String> {
    let value = value.trim();
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        || value.starts_with('-')
        || value.ends_with('-')
        || value.contains("--")
    {
        anyhow::bail!(
            "plugin name must use lowercase letters and digits separated by single hyphens (for example `request-logger`)"
        );
    }
    Ok(value.to_string())
}

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

pub(crate) fn normalized_option_arg(arg: &OsString) -> Option<String> {
    let arg = arg.to_str()?;

    if arg.eq_ignore_ascii_case("-h") {
        return Some("-h".to_string());
    }

    let option = arg.strip_prefix("--")?;
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
        "parity" => Some("parity"),
        "plugin" => Some("plugin"),
        "adds" => Some("adds"),
        "help" => Some("help"),
        _ => None,
    }
}
