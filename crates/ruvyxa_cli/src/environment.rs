//! Toolchain and dependency probing for `ruvyxa doctor`.
//!
//! Reads the project's package manager, installed tool versions, and React
//! compatibility off the filesystem. Every probe degrades to a printable status
//! rather than failing: for `doctor`, a missing tool is a finding to report, not
//! an error to abort on.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;

use anyhow::Context;
use ruvyxa_dev_server::JavaScriptRuntime;

pub(crate) fn detect_package_manager(root: &Path) -> String {
    if find_upwards(root, "pnpm-lock.yaml").is_some() {
        "pnpm".to_string()
    } else if find_upwards(root, "package-lock.json").is_some() {
        "npm".to_string()
    } else if find_upwards(root, "yarn.lock").is_some() {
        "yarn".to_string()
    } else if find_upwards(root, "bun.lock").is_some() || find_upwards(root, "bun.lockb").is_some()
    {
        "bun".to_string()
    } else if find_upwards(root, "deno.lock").is_some()
        || find_upwards(root, "deno.json").is_some()
        || find_upwards(root, "deno.jsonc").is_some()
    {
        "deno".to_string()
    } else {
        "unknown".to_string()
    }
}

pub(crate) fn find_upwards(root: &Path, file_name: &str) -> Option<PathBuf> {
    let mut current = ruvyxa_diagnostics::normalized_canonical_path(root);

    loop {
        let candidate = current.join(file_name);
        if candidate.exists() {
            return Some(candidate);
        }

        if !current.pop() {
            return None;
        }
    }
}

pub(crate) fn tool_version(command: &str, args: &[&str]) -> String {
    // Bounded: `doctor` probes several tools in a row, and one that never
    // answers would stall the whole report instead of being listed as missing.
    let mut probe = ProcessCommand::new(command);
    probe.args(args);
    match ruvyxa_dev_server::process::output_with_timeout(
        &mut probe,
        ruvyxa_dev_server::process::PROBE_TIMEOUT,
    ) {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => "missing".to_string(),
    }
}

/// The oldest Bun release the generated Bun server is written against.
///
/// `Bun.serve` gained `idleTimeout` in 1.1.26, and that is the newest API the
/// emitted program can reach for; everything else it uses is older
/// (`import.meta.dirname` since 1.0.23, `Bun.file` and its `slice`,
/// `server.stop`). Recorded so `doctor` can say so rather than leaving a user to
/// discover it from a deployed server that will not start.
pub(crate) const MINIMUM_BUN_VERSION: (u32, u32, u32) = (1, 1, 26);

/// The oldest Deno release the generated Deno server is written against.
///
/// 2.0 is where Node built-in compatibility — `node:process`, `node:fs`,
/// `node:path` — became the supported path rather than a flag, and the emitted
/// program imports all three by specifier. The two Deno APIs it uses are older
/// than that (`import.meta.dirname` since 1.40, `HttpServer.shutdown`), so this
/// is the binding constraint.
pub(crate) const MINIMUM_DENO_VERSION: (u32, u32, u32) = (2, 0, 0);

/// The leading `major.minor.patch` of a `--version` line, or `None`.
///
/// `bun --version` prints the number alone and `deno --version` prints it after
/// the runtime's name, so the first three dot-separated integers anywhere in the
/// line are the version. A line this cannot read is reported as-is rather than
/// as too old: refusing to run over an unparsed string would turn a cosmetic
/// change in someone else's output into a broken toolchain.
pub(crate) fn parse_runtime_version(text: &str) -> Option<(u32, u32, u32)> {
    let start = text.find(|character: char| character.is_ascii_digit())?;
    let digits = &text[start..];
    let end = digits
        .find(|character: char| !character.is_ascii_digit() && character != '.')
        .unwrap_or(digits.len());
    let mut parts = digits[..end].split('.');
    Some((
        parts.next()?.parse().ok()?,
        parts.next().unwrap_or("0").parse().ok()?,
        parts.next().unwrap_or("0").parse().ok()?,
    ))
}

/// A `doctor` row for a JavaScript runtime, flagged when it is below the floor.
pub(crate) fn runtime_status(version: String, minimum: (u32, u32, u32)) -> String {
    let (major, minor, patch) = minimum;
    if version == "missing" {
        return crate::ui::tool_status(version);
    }
    match parse_runtime_version(&version) {
        Some(found) if found < minimum => {
            crate::ui::warn_text(format!("{version} — Ruvyxa needs {major}.{minor}.{patch}"))
        }
        _ => crate::ui::ok_text(version),
    }
}

/// Reports Bun's version using the same executable resolution as the build
/// and dev-server runtimes. Windows exposes Bun as a `bun.cmd` shim, which a
/// plain `Command::new("bun")` cannot launch, so a naive check reports "missing"
/// even when `bun --version` succeeds in a shell.
pub(crate) fn bun_version() -> String {
    let mut probe = ProcessCommand::new(JavaScriptRuntime::Bun.executable());
    probe.arg("--version");
    match ruvyxa_dev_server::process::output_with_timeout(
        &mut probe,
        ruvyxa_dev_server::process::PROBE_TIMEOUT,
    ) {
        Ok(output) if output.status.success() => {
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        }
        _ => "missing".to_string(),
    }
}

/// Reports Deno's version using the runtime resolver, including npm/nvm shims
/// on Windows. Only the first line is shown because `deno --version` reports
/// its V8 and TypeScript versions on following lines.
pub(crate) fn deno_version() -> String {
    let mut probe = ProcessCommand::new(JavaScriptRuntime::Deno.executable());
    probe.arg("--version");
    match ruvyxa_dev_server::process::output_with_timeout(
        &mut probe,
        ruvyxa_dev_server::process::PROBE_TIMEOUT,
    ) {
        Ok(output) if output.status.success() => String::from_utf8_lossy(&output.stdout)
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string(),
        _ => "missing".to_string(),
    }
}

pub(crate) fn local_binary_upwards(root: &Path, binary: &str) -> Option<PathBuf> {
    let binary = if cfg!(windows) {
        format!("{binary}.cmd")
    } else {
        binary.to_string()
    };
    let mut current = ruvyxa_diagnostics::normalized_canonical_path(root);

    loop {
        let candidate = current.join("node_modules").join(".bin").join(&binary);
        if candidate.is_file() {
            return Some(candidate);
        }

        if !current.pop() {
            return None;
        }
    }
}

pub(crate) fn read_package_json(path: &Path) -> anyhow::Result<serde_json::Value> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    serde_json::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

pub(crate) fn dependency_version(package: &serde_json::Value, name: &str) -> Option<String> {
    ["dependencies", "devDependencies", "peerDependencies"]
        .into_iter()
        .find_map(|section| {
            package
                .get(section)
                .and_then(|deps| deps.get(name))
                .and_then(|version| version.as_str())
                .map(str::to_string)
        })
}

/// Every Ruvyxa package the project depends on, sorted by name.
///
/// A project pulls in `ruvyxa` plus any number of `@ruvyxa/*` packages, and
/// each is versioned independently, so listing them is the only way to see the
/// set a project is actually running.
pub(crate) fn ruvyxa_dependencies(package: &serde_json::Value) -> Vec<(String, String)> {
    let mut found = BTreeMap::<String, String>::new();

    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        let Some(deps) = package.get(section).and_then(|value| value.as_object()) else {
            continue;
        };

        for (name, version) in deps {
            if name == "ruvyxa" || name.starts_with("@ruvyxa/") {
                found.insert(
                    name.clone(),
                    version.as_str().unwrap_or("unknown").to_string(),
                );
            }
        }
    }

    found.into_iter().collect()
}

/// Compare the npm `ruvyxa` dependency against the CLI binary running the check.
///
/// The native CLI and the npm package are released together and read each
/// other's contracts — a manifest written by one version and served by another
/// is a class of failure that only appears at runtime, so the drift is worth
/// naming here rather than leaving it to be discovered in production.
pub(crate) fn cli_version_match(package_version: Option<&str>, cli_version: &str) -> String {
    let Some(package_version) = package_version else {
        return "missing".to_string();
    };

    // A workspace or link protocol resolves to the checkout itself, so there is
    // no published version to compare and nothing to warn about. Reporting
    // these as drift is what made the framework's own repository fail its own
    // doctor.
    if package_version.starts_with("workspace:")
        || package_version.starts_with("link:")
        || package_version.starts_with("file:")
    {
        return format!("ok ({package_version})");
    }

    let declared = package_version.trim_start_matches(['^', '~', '=', 'v', ' ']);
    if declared == "*" || declared.is_empty() || declared.eq_ignore_ascii_case("latest") {
        return format!("ok (unpinned, cli {cli_version})");
    }
    if declared == cli_version {
        return format!("ok ({cli_version})");
    }

    match (major_version(declared), major_version(cli_version)) {
        (Some(left), Some(right)) if left == right => {
            format!("ok (package {declared}, cli {cli_version})")
        }
        _ => format!("drift: package {declared}, cli {cli_version}"),
    }
}

/// The version of `name` as it is actually installed under `node_modules`,
/// searching upward so a package hoisted to a workspace root still counts.
///
/// A declared range and an installed copy are different facts, and only the
/// second one decides whether a page can render. npm installs a dependency's
/// peer dependencies without writing them into the project manifest, so a
/// project that declares neither `react` nor `react-dom` and gets both through
/// `ruvyxa` is correct, not broken — reading the manifest alone would report it
/// as missing React.
pub(crate) fn installed_dependency_version(root: &Path, name: &str) -> Option<String> {
    let mut current = ruvyxa_diagnostics::normalized_canonical_path(root);

    loop {
        let mut manifest = current.join("node_modules");
        for segment in name.split('/') {
            manifest.push(segment);
        }
        manifest.push("package.json");

        if let Ok(package) = read_package_json(&manifest)
            && let Some(version) = package.get("version").and_then(|value| value.as_str())
        {
            return Some(version.to_string());
        }

        if !current.pop() {
            return None;
        }
    }
}

/// What the project will actually load: the installed copy when there is one,
/// and the declared range only as a fallback for an uninstalled project.
pub(crate) fn resolved_dependency_version(
    root: &Path,
    package: &serde_json::Value,
    name: &str,
) -> Option<String> {
    installed_dependency_version(root, name).or_else(|| dependency_version(package, name))
}

pub(crate) fn react_compatibility(root: &Path, package: &serde_json::Value) -> String {
    let Some(react) = resolved_dependency_version(root, package, "react") else {
        return "missing react".to_string();
    };
    let Some(react_dom) = resolved_dependency_version(root, package, "react-dom") else {
        return "missing react-dom".to_string();
    };

    match (major_version(&react), major_version(&react_dom)) {
        (Some(left), Some(right)) if left == right => format!("ok (major {left})"),
        (Some(left), Some(right)) => format!("mismatch react {left} vs react-dom {right}"),
        _ => "unknown version format".to_string(),
    }
}

/// Whether the installed React server-components runtime agrees with React.
///
/// `react-server-dom-webpack` reaches into React internals rather than a public
/// API, so it has to be the same version rather than a compatible major. It is
/// an optional peer of `ruvyxa`: a project that never writes
/// `export const serverComponents = true` should not carry it, so `None` here
/// means "not an RSC project", not "broken" — the row is left off entirely
/// rather than reported as missing on every app that does not use it.
pub(crate) fn server_components_compatibility(root: &Path) -> Option<String> {
    let flight = installed_dependency_version(root, "react-server-dom-webpack")?;
    let Some(react) = installed_dependency_version(root, "react") else {
        return Some(format!("{flight} installed without react"));
    };

    if flight == react {
        Some(format!("ok ({flight})"))
    } else {
        Some(format!(
            "mismatch react {react} vs react-server-dom-webpack {flight}"
        ))
    }
}

pub(crate) fn major_version(version: &str) -> Option<u64> {
    let digits = version
        .trim_start_matches(|character: char| !character.is_ascii_digit())
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}

pub(crate) fn duplicate_dependencies(package: &serde_json::Value) -> Vec<String> {
    let mut seen = BTreeMap::<String, String>::new();
    let mut duplicates = Vec::new();

    for section in ["dependencies", "devDependencies", "peerDependencies"] {
        let Some(deps) = package.get(section).and_then(|value| value.as_object()) else {
            continue;
        };

        for (name, version) in deps {
            let version = version.as_str().unwrap_or("unknown").to_string();
            if let Some(previous) = seen.insert(name.clone(), version.clone())
                && previous != version
            {
                duplicates.push(format!("{name} ({previous}, {version})"));
            }
        }
    }

    duplicates.sort();
    duplicates
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two runtimes print their version differently, and both have to be
    /// read: `bun --version` prints the number alone, `deno --version` prints it
    /// after the runtime's name and follows it with V8 and TypeScript lines.
    #[test]
    fn reads_the_version_out_of_either_runtimes_greeting() {
        assert_eq!(parse_runtime_version("1.4.0"), Some((1, 4, 0)));
        assert_eq!(parse_runtime_version("deno 2.9.5"), Some((2, 9, 5)));
        assert_eq!(parse_runtime_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_runtime_version("2"), Some((2, 0, 0)));
        // Pre-releases are common on both, and the tag after the number is not
        // part of the ordering this compares.
        assert_eq!(parse_runtime_version("1.4.0-canary.3"), Some((1, 4, 0)));
        assert_eq!(parse_runtime_version("missing"), None);
        assert_eq!(parse_runtime_version(""), None);
    }

    /// A runtime below the floor is flagged, one at or above it is not, and one
    /// whose output cannot be read is left alone.
    ///
    /// The last case is the one worth stating: reporting an unparsed string as
    /// too old would turn a cosmetic change in someone else's `--version` output
    /// into a toolchain this tool declares broken.
    #[test]
    fn flags_a_runtime_below_the_floor_and_nothing_else() {
        let needle = format!("needs {}.{}.{}", 1, 1, 26);
        assert!(runtime_status("1.1.25".to_string(), MINIMUM_BUN_VERSION).contains(&needle));
        assert!(runtime_status("1.0.36".to_string(), MINIMUM_BUN_VERSION).contains(&needle));
        assert!(!runtime_status("1.1.26".to_string(), MINIMUM_BUN_VERSION).contains(&needle));
        assert!(!runtime_status("1.4.0".to_string(), MINIMUM_BUN_VERSION).contains(&needle));
        assert!(!runtime_status("deno 2.9.5".to_string(), MINIMUM_DENO_VERSION).contains("needs"));
        assert!(runtime_status("deno 1.46.3".to_string(), MINIMUM_DENO_VERSION).contains("needs"));
        assert!(!runtime_status("bun-next".to_string(), MINIMUM_BUN_VERSION).contains("needs"));
        assert!(runtime_status("missing".to_string(), MINIMUM_BUN_VERSION).contains("missing"));
    }
}
