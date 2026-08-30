//! Project-owned PostCSS transformation for collected global stylesheets.
//!
//! Ruvyxa's CSS pipeline is Rust: [`crate::style::collect_styles`] walks
//! stylesheet imports, compiles Sass, scopes CSS modules, and concatenates the
//! result. PostCSS is the one stage that cannot be, because the plugins are the
//! application's own JavaScript resolved from the application's `node_modules`.
//!
//! This module is the seam. It finds the project's PostCSS configuration and
//! hands each collected stylesheet to `runtime/css-runner.mjs`, which runs the
//! declared plugin chain and reports the files those plugins read.
//!
//! The framework never names a plugin. A project that registers
//! `@tailwindcss/postcss` gets Tailwind; one that registers `autoprefixer` gets
//! autoprefixer; one with no PostCSS config gets the pipeline it had before this
//! stage existed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use ruvyxa_diagnostics::{Diagnostic, Result, RuvyxaError};
use serde::Deserialize;

/// Configuration filenames recognised at the project root, in priority order:
/// the first that exists is the project's PostCSS configuration.
///
/// This is the only such list. `packages/ruvyxa/runtime/css-runner.mjs` carried
/// a second copy, which its doc comment here described as a mirror the two had
/// to keep in agreement — but the runner never consulted it. Detection happens
/// entirely on this side, and the runner is handed the resolved `configFile`
/// path in its request, so the copy could not accept or reject anything. A
/// comment promising a contract that does not exist is worse than no comment,
/// because it tells the next reader a gate is watching this list. The copy is
/// gone; keep this list single.
const CONFIG_FILE_NAMES: &[&str] = &[
    "postcss.config.mjs",
    "postcss.config.js",
    "postcss.config.cjs",
    "postcss.config.ts",
    "postcss.config.mts",
    "postcss.config.cts",
    "postcss.config.json",
    ".postcssrc.mjs",
    ".postcssrc.js",
    ".postcssrc.cjs",
    ".postcssrc.json",
    ".postcssrc",
];

/// The project's PostCSS stage, resolved once per style collection.
#[derive(Debug, Clone)]
pub struct PostcssRunner {
    root: PathBuf,
    /// The discovered configuration file. Recorded as a watch input so a plugin
    /// change invalidates the stylesheet in development.
    config: PathBuf,
    runner: PathBuf,
    mode: &'static str,
    /// The runtime that executes the plugin chain. The chain is the project's
    /// own JavaScript and its plugins are the project's own dependencies, so it
    /// runs under the runtime the rest of the project's JavaScript does.
    runtime: crate::JavaScriptRuntime,
}

/// One stylesheet after the project's plugin chain has run.
pub struct PostcssOutput {
    pub css: String,
    /// Files and directories the plugins read. Tailwind reports the templates it
    /// scanned for class names this way, which is what makes a dev edit to a
    /// component regenerate the stylesheet.
    pub dependencies: Vec<PathBuf>,
}

#[derive(Deserialize)]
struct RunnerResponse {
    ok: bool,
    #[serde(default)]
    css: String,
    #[serde(default)]
    dependencies: Vec<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

impl PostcssRunner {
    /// Detect a project PostCSS configuration.
    ///
    /// Returns `Ok(None)` when the project has no configuration — the explicit
    /// signal that the CSS pipeline must behave exactly as it did before this
    /// stage existed. A project that *does* declare PostCSS but has no reachable
    /// runtime script is an error rather than a silent skip: emitting raw CSS a
    /// browser cannot resolve is how an unstyled page reaches production.
    pub fn detect(
        root: &Path,
        production: bool,
        runtime: crate::JavaScriptRuntime,
    ) -> Result<Option<Self>> {
        let Some(config) = CONFIG_FILE_NAMES
            .iter()
            .map(|name| root.join(name))
            .find(|candidate| candidate.is_file())
        else {
            return Ok(None);
        };

        let runner = crate::render_pipeline::find_runtime_script(root, "css-runner.mjs")
            .ok_or_else(|| {
                Diagnostic::new("RUV1405", "PostCSS support requires runtime/css-runner.mjs")
                    .explain(
                        "This project declares a PostCSS configuration, but the `ruvyxa` package \
                         runtime script that runs the plugin chain was not found.",
                    )
                    .at_file(&config)
                    .suggest("Reinstall the `ruvyxa` package.")
            })?;

        Ok(Some(Self {
            root: root.to_path_buf(),
            config,
            runner,
            mode: if production {
                "production"
            } else {
                "development"
            },
            runtime,
        }))
    }

    /// The configuration file, so callers can record it as a watch input.
    pub fn config_file(&self) -> &Path {
        &self.config
    }

    /// The process that runs the plugin chain.
    ///
    /// Separated from [`Self::run`] so a test can read the program and its
    /// arguments without a runtime installed. What it answers is which
    /// JavaScript runtime the project's own PostCSS plugins execute under, and
    /// that used to be `node` no matter what the project had selected — so a
    /// Bun- or Deno-only machine built everything except its stylesheet.
    fn plugin_command(&self, request_file: &Path) -> Command {
        let mut command = Command::new(self.runtime.executable());
        command
            .args(self.runtime.script_args())
            .current_dir(&self.root)
            .arg(&self.runner)
            .arg(request_file);
        command
    }

    /// Run the plugin chain over one collected stylesheet.
    ///
    /// `from` is the real path of the stylesheet entry, not the temporary file
    /// the CSS travels in. Plugins resolve relative paths and content globs
    /// against it — Tailwind's `@source` in particular — so passing the scratch
    /// path would silently change which templates are scanned.
    pub fn run(&self, css: &str, from: &Path) -> Result<PostcssOutput> {
        let scratch = ScratchDir::new()?;
        let css_file = scratch.path().join("input.css");
        let request_file = scratch.path().join("request.json");
        fs::write(&css_file, css)?;
        fs::write(
            &request_file,
            serde_json::json!({
                "root": self.root,
                "config": self.config,
                "from": from,
                "cssFile": css_file,
                "mode": self.mode,
            })
            .to_string(),
        )?;

        let mut command = self.plugin_command(&request_file);

        let output =
            crate::process::output_with_timeout(&mut command, crate::process::STYLE_TOOL_TIMEOUT)
                .map_err(|error| match error {
                crate::process::ProcessError::Io(source) => RuvyxaError::Io {
                    message: "Failed to run PostCSS".to_string(),
                    source,
                },
                timed_out => RuvyxaError::Message(format!("PostCSS {timed_out}")),
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        // The runner answers with exactly one JSON line. Anything else means it
        // died before reporting — a plugin calling `process.exit`, an OOM — and
        // the raw streams are the only evidence there is.
        let Some(response) = stdout
            .lines()
            .rev()
            .find_map(|line| serde_json::from_str::<RunnerResponse>(line).ok())
        else {
            return Err(
                Diagnostic::new("RUV1406", "PostCSS did not report a result")
                    .explain(format!(
                        "{}\n{}",
                        stdout.trim(),
                        String::from_utf8_lossy(&output.stderr).trim()
                    ))
                    .at_file(from)
                    .suggest("Check the plugins registered in the PostCSS configuration.")
                    .into(),
            );
        };

        if !response.ok {
            // The runner reports which half failed: loading the project's
            // configuration and plugins, or running them. Anything else is
            // treated as a transform failure rather than trusted verbatim — the
            // diagnostic code is part of Ruvyxa's contract, not the runner's.
            let (code, title, fix) = match response.code.as_deref() {
                Some("RUV1405") => (
                    "RUV1405",
                    "PostCSS configuration could not be loaded",
                    "Install the packages the configuration names, or remove them from it.",
                ),
                _ => (
                    "RUV1406",
                    "PostCSS failed for a global stylesheet",
                    "Fix the reported plugin error. Ruvyxa does not fall back to untransformed \
                     CSS, because that ships a stylesheet the browser cannot resolve.",
                ),
            };
            return Err(Diagnostic::new(code, title)
                .explain(response.message.unwrap_or_default())
                .at_file(from)
                .suggest(fix)
                .into());
        }

        Ok(PostcssOutput {
            css: response.css,
            dependencies: response
                .dependencies
                .into_iter()
                .map(PathBuf::from)
                .collect(),
        })
    }
}

/// A temporary directory removed when the run ends, however it ends.
///
/// The collected CSS travels by file because the caller closes the child's
/// stdin and a stylesheet is far larger than an argument list may be. What
/// travels with it is the project's whole compiled global stylesheet and a
/// `request.json` naming the absolute project root.
///
/// Created by `tempfile`, not by hand. The hand-rolled version built
/// `$TMPDIR/ruvyxa-postcss/{pid}-{nanos}` with `create_dir_all`, which succeeds
/// on a directory that already exists -- so the path was *claimed* rather than
/// created, and both name components are guessable by anyone who can watch the
/// process table. On a shared build host that is a readable copy of the
/// project's stylesheet and root path, and a directory an attacker can put
/// content into for the PostCSS runner to read. `tempfile` creates exclusively
/// and retries on collision, so the directory is this process's or the call
/// fails.
///
/// The mode is asked for explicitly. `tempfile` creates a *file* with 0600, but
/// a *directory* with the platform default -- 0755 under a normal umask -- which
/// its own documentation says and which CI proved: the first version of this
/// asserted 0700 and failed on Linux with `mode was 755`. Since the whole point
/// here is that another user on a shared host cannot read the project's compiled
/// stylesheet or learn its absolute root, the permission has to be requested
/// rather than assumed. Requested rather than applied afterwards, too:
/// `tempfile` passes it to `DirBuilderExt::mode`, so the directory exists with
/// that mode from the moment it exists and there is no window to race.
struct ScratchDir {
    /// Held for its `Drop`, which removes the directory however the run ends.
    dir: tempfile::TempDir,
}

impl ScratchDir {
    fn new() -> Result<Self> {
        let mut builder = tempfile::Builder::new();
        builder.prefix("ruvyxa-postcss-");
        // Windows has no mode; the per-user temp directory is what answers there.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        Ok(Self {
            dir: builder.tempdir()?,
        })
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The scratch directory is this process's, or the call fails.
    ///
    /// The hand-rolled version keyed the name on the process id and a
    /// nanosecond clock and created it with `create_dir_all`, which succeeds on
    /// a directory that is already there. Two runs inside the same process --
    /// `ruvyxa dev` recompiling on a save while a build runs -- could therefore
    /// land on one directory and overwrite each other's `input.css`, and
    /// anything that guessed the name first was simply adopted.
    #[test]
    fn two_scratch_directories_never_collide() {
        let first = ScratchDir::new().expect("a scratch directory");
        let second = ScratchDir::new().expect("a second scratch directory");
        assert_ne!(first.path(), second.path());
        assert!(first.path().is_dir() && second.path().is_dir());

        // Distinct directories mean distinct files, which is the property the
        // caller actually depends on.
        std::fs::write(first.path().join("input.css"), b"a{}").expect("write");
        std::fs::write(second.path().join("input.css"), b"b{}").expect("write");
        assert_eq!(
            std::fs::read_to_string(first.path().join("input.css")).expect("read"),
            "a{}",
        );

        let path = first.path().to_path_buf();
        drop(first);
        assert!(!path.exists(), "the directory is removed when the run ends");
    }

    /// Not readable by anyone else on the host.
    ///
    /// The project's whole compiled stylesheet and the absolute project root go
    /// into this directory. This assertion earned its place immediately: the
    /// first version of `ScratchDir` relied on `tempfile` for the mode, and CI
    /// answered `mode was 755` -- `tempfile` restricts a temporary *file* to the
    /// owner and creates a temporary *directory* with the platform default.
    ///
    /// On Windows the equivalent guarantee comes from the per-user temp
    /// directory rather than from a mode, so there is nothing to assert there --
    /// stated rather than silently skipped.
    #[cfg(unix)]
    #[test]
    fn the_scratch_directory_is_private_to_this_user() {
        use std::os::unix::fs::PermissionsExt;

        let scratch = ScratchDir::new().expect("a scratch directory");
        let mode = std::fs::metadata(scratch.path())
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700, "mode was {:o}", mode & 0o777);
    }

    #[test]
    fn a_project_without_a_config_has_no_postcss_stage() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            PostcssRunner::detect(dir.path(), false, crate::JavaScriptRuntime::Node)
                .unwrap()
                .is_none(),
            "a project with no PostCSS config must keep the previous CSS behavior"
        );
    }

    /// The plugin chain is the project's own JavaScript, loaded from the
    /// project's own `node_modules`, so it runs under the runtime the project
    /// selected. Every other JavaScript stage of a build already did; this one
    /// launched `node` regardless, which a machine that has only Bun or Deno
    /// does not have.
    #[test]
    fn the_plugin_chain_runs_under_the_projects_runtime() {
        for runtime in [
            crate::JavaScriptRuntime::Node,
            crate::JavaScriptRuntime::Bun,
            crate::JavaScriptRuntime::Deno,
        ] {
            let dir = tempfile::tempdir().unwrap();
            fs::write(
                dir.path().join("postcss.config.js"),
                "module.exports = { plugins: {} }",
            )
            .unwrap();
            let runner = PostcssRunner::detect(dir.path(), false, runtime)
                .unwrap()
                .expect("a project with a PostCSS config has a stage");
            let command = runner.plugin_command(Path::new("request.json"));
            assert_eq!(
                command.get_program(),
                runtime.executable().as_os_str(),
                "{runtime:?} must run its own executable"
            );
            let leading = command
                .get_args()
                .take(runtime.script_args().len())
                .collect::<Vec<_>>();
            assert_eq!(
                leading,
                runtime
                    .script_args()
                    .iter()
                    .map(std::ffi::OsStr::new)
                    .collect::<Vec<_>>(),
                "{runtime:?} must keep the arguments its runtime needs before a script"
            );
        }
    }

    #[test]
    fn every_recognised_config_name_is_detected() {
        for name in CONFIG_FILE_NAMES {
            let dir = tempfile::tempdir().unwrap();
            fs::write(dir.path().join(name), "export default { plugins: {} }").unwrap();
            let detected = PostcssRunner::detect(dir.path(), false, crate::JavaScriptRuntime::Node);
            // The runtime script lives in this working tree, so detection
            // resolves; the point under test is that the name was recognised.
            match detected {
                Ok(Some(runner)) => assert_eq!(runner.config_file().file_name().unwrap(), *name),
                Ok(None) => panic!("{name} was not recognised as a PostCSS config"),
                Err(error) => panic!("{name}: {error}"),
            }
        }
    }
}
