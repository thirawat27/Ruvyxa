//! Turning CLI arguments plus `ruvyxa.config.*` into a runnable configuration.
//!
//! Two server configurations are produced here — `dev` and `start` — and they
//! resolve the same settings from the same sources, with an explicit flag
//! always beating everything below it. Keeping both in one module is
//! deliberate: a setting added to one and forgotten in the other is the failure
//! mode, and here the omission is visible.
//!
//! The bind address is the one setting whose sources differ from the rest, and
//! `resolve_bind_address` owns that ordering for both.
//!
//! This module also owns adapter inspection and the JavaScript runtime choice
//! (Node, Bun, or Deno), including the process-wide override a `--runtime` flag sets
//! before any command runs.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use clap::ValueEnum;
use ruvyxa_dev_server::{JavaScriptRuntime, ServerConfig, find_runtime_script};

use crate::*;

/// Default bind host for `ruvyxa dev`: reachable from this machine only.
pub(crate) const DEV_DEFAULT_HOST: &str = "localhost";
/// Default bind host for `ruvyxa start` and `ruvyxa preview`.
///
/// Every container runtime routes to the container's address rather than to its
/// loopback, so a production server bound to `localhost` answers nothing from
/// outside — the health check fails and the platform reports a crash loop with
/// no error in the log. `0.0.0.0` is also what the standalone server this
/// repository generates has always used, so the two production hosts now agree.
pub(crate) const PRODUCTION_DEFAULT_HOST: &str = "0.0.0.0";
/// Port used when no flag, environment variable, or config value names one.
pub(crate) const DEFAULT_PORT: u16 = 3000;

/// Resolve the address a server binds: flag, then environment, then config.
///
/// An explicit flag is the operator speaking now, so it wins outright. `PORT`
/// and `HOST` come next, because they are set by whatever actually owns the
/// socket: every managed platform injects `PORT` and expects the process to use
/// it, and a `ruvyxa.config.ts` committed to the repository cannot know the
/// number. The config file is the project's own default below that, and
/// `default_host` is the last word.
///
/// Reading the environment through a closure rather than `std::env::var` keeps
/// this testable — the same shape `detect_platform_adapter` uses — since
/// mutating process environment from a test is both global and unsound.
///
/// An unparseable `PORT` is an error rather than a fallback: on a platform that
/// injected it, quietly binding 3000 instead produces a failing health check
/// and nothing that names the cause. Empty and whitespace-only values are
/// ignored, because a CI template that declares the variable without setting it
/// is saying nothing, not saying zero.
pub(crate) fn resolve_bind_address(
    args: &ServerArgs,
    config: &ProjectConfig,
    env: impl Fn(&str) -> Option<String>,
    default_host: &str,
) -> anyhow::Result<(String, u16)> {
    let from_env = |name: &str| {
        env(name)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    };

    let host = args
        .host
        .clone()
        .or_else(|| from_env("HOST"))
        .or_else(|| config.server.host.clone())
        .unwrap_or_else(|| default_host.to_string());

    let port = match args.port {
        Some(port) => port,
        None => match from_env("PORT") {
            Some(raw) => raw.parse::<u16>().with_context(|| {
                format!("PORT must be a number between 0 and 65535, got `{raw}`")
            })?,
            None => config.server.port.unwrap_or(DEFAULT_PORT),
        },
    };

    Ok((host, port))
}

pub(crate) fn dev_server_config(
    args: &ServerArgs,
    config: &ProjectConfig,
) -> anyhow::Result<ServerConfig> {
    let (host, port) = resolve_bind_address(
        args,
        config,
        |key| std::env::var(key).ok(),
        DEV_DEFAULT_HOST,
    )?;
    let mut server = ServerConfig::dev(&args.root, host, port);
    let out_dir = args.root.join(config.out_dir());
    server.app_dir = args.root.join(config.app_dir());
    server.public_dir = args.root.join("public");
    server.client_dir = out_dir.join("client");
    server.prerender_dir = out_dir.join("prerender");
    server.cache_route_manifest = config.cache.route_manifest.unwrap_or(true);
    server.cache_css = config.cache.css.unwrap_or(true);
    server.style_entries = config.style_entries(&args.root);
    server.prebundle_dependencies = config.build.prebundle_dependencies.unwrap_or(true);
    server.runtime = config.javascript_runtime();
    server.jsx_runtime = parse_jsx_runtime(config.build.jsx_runtime.as_deref())?;
    server.es_target = parse_es_target(config.build.es_target.as_ref())?;
    server.error_overlay = config.debug.overlay.unwrap_or(true);
    server.debug_traces = config.debug.traces.unwrap_or(false);
    server.action_body_limit_bytes = config
        .security
        .action_body_limit_bytes
        .unwrap_or(server.action_body_limit_bytes);
    server.api_body_limit_bytes = config
        .security
        .api_body_limit_bytes
        .unwrap_or(server.api_body_limit_bytes);
    server.plugin_response_body_limit_bytes = config
        .security
        .plugin_response_body_limit_bytes
        .unwrap_or(server.plugin_response_body_limit_bytes);
    if let Some(rate_limit) = &config.security.action_rate_limit {
        server.action_rate_limit_max = rate_limit.max.unwrap_or(server.action_rate_limit_max);
        server.action_rate_limit_window = Duration::from_secs(
            rate_limit
                .window
                .unwrap_or(server.action_rate_limit_window.as_secs()),
        );
    }
    server.same_origin_actions = config
        .security
        .same_origin_actions
        .unwrap_or(server.same_origin_actions);
    server.fetch_metadata_actions = config
        .security
        .fetch_metadata_actions
        .unwrap_or(server.fetch_metadata_actions);
    server.trusted_proxies = parse_trusted_proxies(&config.security.trusted_proxy_ips)?;
    server.security_headers = config
        .security
        .security_headers
        .unwrap_or(server.security_headers);
    server.middleware = config.middleware.clone();
    server.plugins_enabled = !config.plugins.is_empty();
    server.plugin_head = collect_plugin_head(&config.plugins);
    server.default_render_strategy = config.rendering.default_strategy;
    server.default_revalidate = config.rendering.default_revalidate;
    server.i18n = config.i18n.as_ref().map(I18nConfigOptions::routing);
    server.dynamic_images.enabled = config.images.on_demand.enabled();
    server.dynamic_images.max_width = config.images.on_demand.max_width();
    server.dynamic_images.default_quality = config.images.quality.clamp(1, 100);
    if config.typed_routes() {
        server.route_manifest_observer = Some(route_types_observer(args.root.clone()));
    }
    Ok(server)
}

/// Keep `.ruvyxa/types/routes.d.ts` in step with the dev server's route set.
///
/// Only `dev` installs this. A production server serves a build whose routes
/// cannot change while it runs, and rewriting a source-tree file from a running
/// production process would be a surprise, not a convenience.
///
/// The last emitted route set is remembered here so an invalidation that did
/// not change the routes — the overwhelmingly common case, since any edit to
/// any file under `app/` invalidates the manifest — costs a comparison rather
/// than a filesystem read.
fn route_types_observer(root: PathBuf) -> ruvyxa_dev_server::RouteManifestObserver {
    let last_written: Mutex<Option<String>> = Mutex::new(None);
    ruvyxa_dev_server::RouteManifestObserver::new(move |manifest| {
        let source = route_types_source(manifest);
        let mut last = match last_written.lock() {
            Ok(last) => last,
            // A poisoned lock means a previous write panicked. Regenerating is
            // idempotent, so recovering is strictly better than giving up on
            // route types for the rest of the session.
            Err(poisoned) => poisoned.into_inner(),
        };
        if last.as_deref() == Some(source.as_str()) {
            return;
        }
        match write_route_types(&root, manifest) {
            Ok(_) => *last = Some(source),
            Err(error) => {
                // A page render must not fail because a convenience file could
                // not be written; report and let the next discovery retry.
                eprintln!(
                    "{}",
                    warn_text(format!("typed routes not written: {error:#}"))
                );
            }
        }
    })
}

pub(crate) fn production_server_config(
    args: &ServerArgs,
    config: &ProjectConfig,
) -> anyhow::Result<ServerConfig> {
    let (host, port) = resolve_bind_address(
        args,
        config,
        |key| std::env::var(key).ok(),
        PRODUCTION_DEFAULT_HOST,
    )?;
    let mut server = ServerConfig::production(&args.root, host, port);
    let out_dir = args.root.join(config.out_dir());
    server.app_dir = out_dir.join("server").join(config.app_dir());
    server.public_dir = out_dir.join("assets");
    server.client_dir = out_dir.join("client");
    server.prerender_dir = out_dir.join("prerender");
    server.cache_route_manifest = config.cache.route_manifest.unwrap_or(true);
    server.cache_css = config.cache.css.unwrap_or(true);
    server.style_entries = config.style_entries(&out_dir.join("server"));
    server.runtime = config.javascript_runtime();
    server.jsx_runtime = parse_jsx_runtime(config.build.jsx_runtime.as_deref())?;
    server.es_target = parse_es_target(config.build.es_target.as_ref())?;
    server.action_body_limit_bytes = config
        .security
        .action_body_limit_bytes
        .unwrap_or(server.action_body_limit_bytes);
    server.api_body_limit_bytes = config
        .security
        .api_body_limit_bytes
        .unwrap_or(server.api_body_limit_bytes);
    server.plugin_response_body_limit_bytes = config
        .security
        .plugin_response_body_limit_bytes
        .unwrap_or(server.plugin_response_body_limit_bytes);
    if let Some(rate_limit) = &config.security.action_rate_limit {
        server.action_rate_limit_max = rate_limit.max.unwrap_or(server.action_rate_limit_max);
        server.action_rate_limit_window = Duration::from_secs(
            rate_limit
                .window
                .unwrap_or(server.action_rate_limit_window.as_secs()),
        );
    }
    server.same_origin_actions = config
        .security
        .same_origin_actions
        .unwrap_or(server.same_origin_actions);
    server.fetch_metadata_actions = config
        .security
        .fetch_metadata_actions
        .unwrap_or(server.fetch_metadata_actions);
    server.trusted_proxies = parse_trusted_proxies(&config.security.trusted_proxy_ips)?;
    server.security_headers = config
        .security
        .security_headers
        .unwrap_or(server.security_headers);
    server.middleware = config.middleware.clone();
    server.plugins_enabled = !config.plugins.is_empty();
    server.plugin_head = collect_plugin_head(&config.plugins);
    server.default_render_strategy = config.rendering.default_strategy;
    server.default_revalidate = config.rendering.default_revalidate;
    server.i18n = config.i18n.as_ref().map(I18nConfigOptions::routing);
    server.dynamic_images.enabled = config.images.on_demand.enabled();
    server.dynamic_images.max_width = config.images.on_demand.max_width();
    server.dynamic_images.default_quality = config.images.quality.clamp(1, 100);
    Ok(server)
}

pub(crate) fn load_project_config(root: &Path) -> anyhow::Result<ProjectConfig> {
    let runtime_override = runtime_override()?;
    let invoker_runtime = invoker_runtime();
    let bootstrap_runtime = runtime_override
        .or(invoker_runtime)
        .unwrap_or_else(default_javascript_runtime);
    let Some(renderer) = find_runtime_script(root, "config-renderer.mjs") else {
        let mut config = ProjectConfig {
            config_dependency_hash: "no-config".to_string(),
            ..ProjectConfig::default()
        };
        config.javascript_runtime_override = Some(bootstrap_runtime);
        config.validate_paths()?;
        return Ok(config);
    };

    let mut result = run_config_renderer(root, &renderer, bootstrap_runtime)?;
    if !result.ok {
        anyhow::bail!(
            "config load failed: {} {}",
            result.code.unwrap_or_else(|| "RUV1600".to_string()),
            result
                .message
                .or(result.stack)
                .unwrap_or_else(|| "unknown config error".to_string())
        )
    }

    let mut config = result.config.take().unwrap_or_default();
    let selected_runtime = runtime_override.unwrap_or_else(|| {
        if config.runtime.is_some() {
            config.javascript_runtime()
        } else {
            invoker_runtime.unwrap_or_else(|| config.javascript_runtime())
        }
    });
    if selected_runtime != bootstrap_runtime {
        result = run_config_renderer(root, &renderer, selected_runtime)?;
        if !result.ok {
            anyhow::bail!(
                "config load failed: {} {}",
                result.code.unwrap_or_else(|| "RUV1600".to_string()),
                result
                    .message
                    .or(result.stack)
                    .unwrap_or_else(|| "unknown config error".to_string())
            )
        }
        config = result.config.take().unwrap_or_default();
    }
    config.javascript_runtime_override = runtime_override.or_else(|| {
        if config.runtime.is_none() {
            invoker_runtime
        } else {
            None
        }
    });
    let dependency_hash = required_config_dependency_hash(&result)?;
    config.config_dependency_hash = dependency_hash;
    config.validate_paths()?;
    Ok(config)
}

/// Identity of the toolchain that wrote a [`ConfigLoadCache`] entry.
///
/// This cache lives at a fixed path, so nothing else distinguishes an entry
/// written by an older Ruvyxa. It used to be a `CONFIG_CACHE_VERSION: u32`
/// documented as "bump when the meaning of a field changes" — a stamp that is
/// silent when forgotten, and the failure is a config result replayed from a
/// renderer that no longer exists. The crate version answers the same question
/// without anyone maintaining it.
pub(crate) const CONFIG_CACHE_TOOLCHAIN: &str = env!("CARGO_PKG_VERSION");

/// A previous config render, with everything needed to decide if it still holds.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigLoadCache {
    /// The Ruvyxa that wrote this entry; see [`CONFIG_CACHE_TOOLCHAIN`].
    pub(crate) toolchain: String,
    /// The runtime that produced this result. A config may branch on
    /// `process.versions`, so a result rendered by Node cannot answer for Bun.
    pub(crate) runtime: String,
    /// Content hash of `config-renderer.mjs`, so upgrading the ruvyxa package
    /// invalidates results produced by the previous renderer.
    pub(crate) renderer_fingerprint: String,
    /// Project-relative input path to its content hash at render time.
    pub(crate) inputs: BTreeMap<String, String>,
    /// Environment variables the config read, and what they were.
    pub(crate) env: BTreeMap<String, Option<String>>,
    /// The renderer's stdout verbatim, replayed on a hit.
    pub(crate) stdout: String,
}

fn config_cache_path(root: &Path) -> PathBuf {
    // Alongside the compiled config bundle the renderer already writes, which is
    // fixed at `.ruvyxa/cache/` — `outDir` cannot be honoured here, because
    // reading it is what this cache exists to avoid.
    root.join(".ruvyxa").join("cache").join("config-load.json")
}

fn file_fingerprint(path: &Path) -> Option<String> {
    let bytes = fs::read(path).ok()?;
    Some(blake3::hash(&bytes).to_hex().to_string())
}

/// Whether `cache` still describes the project as it is on disk right now.
pub(crate) fn config_cache_is_current(
    cache: &ConfigLoadCache,
    root: &Path,
    runtime: JavaScriptRuntime,
    renderer_fingerprint: &str,
) -> bool {
    if cache.toolchain != CONFIG_CACHE_TOOLCHAIN
        || cache.runtime != runtime.command()
        || cache.renderer_fingerprint != renderer_fingerprint
    {
        return false;
    }

    // A config with no inputs was never rendered from a file, so there is
    // nothing that could prove it still current.
    if cache.inputs.is_empty() {
        return false;
    }

    for (relative, expected) in &cache.inputs {
        if file_fingerprint(&root.join(relative)).as_deref() != Some(expected.as_str()) {
            return false;
        }
    }

    cache
        .env
        .iter()
        .all(|(key, value)| std::env::var(key).ok().as_deref() == value.as_deref())
}

fn read_config_cache(
    root: &Path,
    runtime: JavaScriptRuntime,
    renderer_fingerprint: &str,
) -> Option<ConfigRendererOutput> {
    let raw = fs::read_to_string(config_cache_path(root)).ok()?;
    let cache: ConfigLoadCache = serde_json::from_str(&raw).ok()?;
    if !config_cache_is_current(&cache, root, runtime, renderer_fingerprint) {
        return None;
    }
    // Re-parsed rather than stored structurally so the cached result travels
    // through exactly the same validation a fresh render does.
    let result = parse_config_renderer_output(root, cache.stdout.as_bytes(), b"", "cached").ok()?;
    if result
        .config
        .as_ref()
        .is_some_and(ProjectConfig::markdown_enabled)
        && !root
            .join(".ruvyxa")
            .join("cache")
            .join("config")
            .join("runtime-config.mjs")
            .is_file()
    {
        return None;
    }
    Some(result)
}

/// Persist a successful render. A cache that cannot be written is not an error:
/// the next run simply pays for the renderer again.
fn write_config_cache(
    root: &Path,
    runtime: JavaScriptRuntime,
    renderer_fingerprint: &str,
    result: &ConfigRendererOutput,
    stdout: &str,
) {
    let Some(key) = result.cache_key.as_ref() else {
        return;
    };
    if key.inputs.is_empty() {
        return;
    }

    let mut inputs = BTreeMap::new();
    for relative in &key.inputs {
        // A file that vanished between render and write would make the entry
        // permanently stale; skip writing rather than store a lie.
        let Some(hash) = file_fingerprint(&root.join(relative)) else {
            return;
        };
        inputs.insert(relative.clone(), hash);
    }

    let cache = ConfigLoadCache {
        toolchain: CONFIG_CACHE_TOOLCHAIN.to_string(),
        runtime: runtime.command().to_string(),
        renderer_fingerprint: renderer_fingerprint.to_string(),
        inputs,
        env: key.env.clone(),
        stdout: stdout.to_string(),
    };

    let path = config_cache_path(root);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(encoded) = serde_json::to_string(&cache) {
        let _ = fs::write(path, encoded);
    }
}

/// Render a project's config, reusing the previous result while it still holds.
///
/// Rendering costs a full JavaScript runtime start plus a recompile of the
/// config bundle — around 300–400ms — and it ran on every single CLI command,
/// including ones that never look at most of the config. Caching it is the
/// single largest saving available on a warm build.
///
/// The cache is keyed on what the renderer reports it depended on rather than on
/// the config file alone: transitive project imports, the package manifests, the
/// runtime, the renderer itself, and every environment variable the config read.
pub(crate) fn run_config_renderer(
    root: &Path,
    renderer: &Path,
    runtime: JavaScriptRuntime,
) -> anyhow::Result<ConfigRendererOutput> {
    let renderer_fingerprint = file_fingerprint(renderer).unwrap_or_default();
    if !renderer_fingerprint.is_empty()
        && let Some(cached) = read_config_cache(root, runtime, &renderer_fingerprint)
    {
        return Ok(cached);
    }

    let mut command = ProcessCommand::new(runtime.executable());
    command
        .args(runtime.script_args())
        .arg(renderer)
        .arg(root)
        .env("RUVYXA_RUNTIME", runtime.command());
    // Bounded: a `ruvyxa.config.ts` that imports a module which opens a handle
    // — a database pool, a watcher, a server — keeps the runtime alive after
    // the config has already been printed, which used to hang every command
    // before it produced any output at all.
    let output = ruvyxa_dev_server::process::output_with_timeout(
        &mut command,
        ruvyxa_dev_server::process::CONFIG_LOAD_TIMEOUT,
    )
    .with_context(|| {
        format!(
            "failed to load config with {} for {}",
            runtime.command(),
            root.display()
        )
    })?;
    let result = parse_config_renderer_output(
        root,
        &output.stdout,
        &output.stderr,
        &output.status.to_string(),
    )?;

    // Only a successful render is worth replaying; a failure must be re-reported
    // by re-running, so the user sees it again after fixing nothing.
    if result.ok && !renderer_fingerprint.is_empty() {
        write_config_cache(
            root,
            runtime,
            &renderer_fingerprint,
            &result,
            &String::from_utf8_lossy(&output.stdout),
        );
    }

    Ok(result)
}

pub(crate) fn run_adapter_runner(
    root: &Path,
    staging_dir: &Path,
    runtime: JavaScriptRuntime,
    adapter_name: Option<&str>,
) -> anyhow::Result<Vec<AdapterArtifactReport>> {
    let runner = find_runtime_script(root, "adapter-runner.mjs").ok_or_else(|| {
        anyhow::anyhow!(
            "adapter build hook requires runtime/adapter-runner.mjs; reinstall the ruvyxa package"
        )
    })?;
    let mut command = ProcessCommand::new(runtime.executable());
    command
        .args(runtime.script_args())
        .arg(runner)
        .arg(root)
        .arg(staging_dir)
        .env("RUVYXA_RUNTIME", runtime.command());
    if let Some(adapter_name) = adapter_name {
        command.arg(adapter_name);
    }
    let output = ruvyxa_dev_server::process::output_with_timeout(
        &mut command,
        ruvyxa_dev_server::process::ADAPTER_HOOK_TIMEOUT,
    )
    .with_context(|| {
        format!(
            "failed to run adapter build hook with {} for {}",
            runtime.command(),
            root.display()
        )
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let result: AdapterRunnerOutput = serde_json::from_str(&stdout).with_context(|| {
        format!(
            "adapter runner returned invalid output for {}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
            root.display(),
            output.status,
            diagnostic_stream(&stdout),
            diagnostic_stream(&stderr),
        )
    })?;
    if !result.ok {
        anyhow::bail!(
            "adapter build hook failed: {} {}",
            result.code.unwrap_or_else(|| "RUV2200".to_string()),
            result
                .message
                .or(result.stack)
                .unwrap_or_else(|| "unknown adapter error".to_string())
        );
    }
    result
        .result
        .map(serde_json::from_value)
        .transpose()
        .context("adapter runner returned an invalid artifact report")
        .map(Option::unwrap_or_default)
}

pub(crate) fn inspect_adapter(
    root: &Path,
    out_dir: &Path,
    runtime: JavaScriptRuntime,
    adapter_name: Option<&str>,
) -> anyhow::Result<Option<AdapterInspection>> {
    let runner = find_runtime_script(root, "adapter-runner.mjs").ok_or_else(|| {
        anyhow::anyhow!(
            "adapter inspection requires runtime/adapter-runner.mjs; reinstall the ruvyxa package"
        )
    })?;
    let mut command = ProcessCommand::new(runtime.executable());
    command
        .args(runtime.script_args())
        .arg(runner)
        .arg(root)
        .arg(out_dir)
        .env("RUVYXA_RUNTIME", runtime.command())
        .env("RUVYXA_ADAPTER_RUNNER_MODE", "inspect");
    if let Some(adapter_name) = adapter_name {
        command.arg(adapter_name);
    }
    let output = ruvyxa_dev_server::process::output_with_timeout(
        &mut command,
        ruvyxa_dev_server::process::ADAPTER_HOOK_TIMEOUT,
    )
    .with_context(|| {
        format!(
            "failed to inspect adapter with {} for {}",
            runtime.command(),
            root.display()
        )
    })?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let result: AdapterRunnerOutput = serde_json::from_str(&stdout).with_context(|| {
        format!(
            "adapter inspector returned invalid output for {}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
            root.display(),
            output.status,
            diagnostic_stream(&stdout),
            diagnostic_stream(&stderr),
        )
    })?;
    if !result.ok {
        anyhow::bail!(
            "adapter inspection failed: {} {}",
            result.code.unwrap_or_else(|| "RUV2200".to_string()),
            result
                .message
                .or(result.stack)
                .unwrap_or_else(|| "unknown adapter error".to_string())
        );
    }
    result
        .result
        .map(serde_json::from_value)
        .transpose()
        .context("adapter inspector returned an invalid capability report")
}

/// Process-wide runtime override set by the `--runtime` CLI flag. Takes
/// precedence over `RUVYXA_RUNTIME` and `config.runtime`.
pub(crate) static CLI_RUNTIME_OVERRIDE: std::sync::OnceLock<JavaScriptRuntime> =
    std::sync::OnceLock::new();

pub(crate) fn command_runtime(command: &Command) -> Option<CliRuntime> {
    match command {
        Command::Dev(args) | Command::Start(args) | Command::Preview(args) => args.runtime,
        Command::Build(args) => args.runtime,
        Command::Check(args) | Command::Clean(args) | Command::TestParity(args) => args.runtime,
        Command::Routes(args) => args.runtime,
        Command::Analyze(args) => args.runtime,
        Command::Adds(args) => args.runtime,
        Command::Doctor(args) => args.runtime,
        Command::Bench(args) => args.runtime,
        Command::Trace(_) | Command::Plugin(_) => None,
    }
}

pub(crate) fn set_cli_runtime_override(runtime: Option<CliRuntime>) {
    if let Some(runtime) = runtime {
        let _ = CLI_RUNTIME_OVERRIDE.set(runtime.into());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub(crate) enum CliRuntime {
    Node,
    Bun,
    Deno,
}

impl From<CliRuntime> for JavaScriptRuntime {
    fn from(value: CliRuntime) -> Self {
        match value {
            CliRuntime::Node => Self::Node,
            CliRuntime::Bun => Self::Bun,
            CliRuntime::Deno => Self::Deno,
        }
    }
}

pub(crate) fn runtime_override() -> anyhow::Result<Option<JavaScriptRuntime>> {
    if let Some(runtime) = CLI_RUNTIME_OVERRIDE.get() {
        return Ok(Some(*runtime));
    }
    let Ok(value) = std::env::var("RUVYXA_RUNTIME") else {
        return Ok(None);
    };
    match value.trim().to_ascii_lowercase().as_str() {
        "node" => Ok(Some(JavaScriptRuntime::Node)),
        "bun" => Ok(Some(JavaScriptRuntime::Bun)),
        "deno" => Ok(Some(JavaScriptRuntime::Deno)),
        _ => anyhow::bail!("RUVYXA_RUNTIME must be 'node', 'bun', or 'deno'"),
    }
}

/// Runtime that executed the JavaScript package launcher. This is a hint below
/// explicit CLI/environment/config choices, so `bun run dev` and
/// `deno task dev` feel native without overriding project policy.
pub(crate) fn invoker_runtime() -> Option<JavaScriptRuntime> {
    match std::env::var("RUVYXA_INVOKER_RUNTIME")
        .ok()?
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "node" => Some(JavaScriptRuntime::Node),
        "bun" => Some(JavaScriptRuntime::Bun),
        "deno" => Some(JavaScriptRuntime::Deno),
        _ => None,
    }
}

pub(crate) fn default_javascript_runtime() -> JavaScriptRuntime {
    JavaScriptRuntime::detect()
}

pub(crate) fn required_config_dependency_hash(
    result: &ConfigRendererOutput,
) -> anyhow::Result<String> {
    result
        .dependency_hash
        .as_ref()
        .filter(|hash| !hash.is_empty())
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("config renderer returned success without dependencyHash"))
}

pub(crate) fn parse_config_renderer_output(
    root: &Path,
    stdout: &[u8],
    stderr: &[u8],
    status: &str,
) -> anyhow::Result<ConfigRendererOutput> {
    let stdout = String::from_utf8_lossy(stdout);
    let stderr = String::from_utf8_lossy(stderr);
    serde_json::from_str(&stdout).with_context(|| {
        format!(
            "config renderer returned invalid output for {}\nstatus: {}\nstdout:\n{}\nstderr:\n{}",
            root.display(),
            status,
            diagnostic_stream(&stdout),
            diagnostic_stream(&stderr),
        )
    })
}

pub(crate) fn build_cache_dir(root: &Path, cache: &CacheConfigOptions) -> PathBuf {
    resolve_build_cache_dir(
        root,
        cache.build_dir.as_deref(),
        std::env::var_os("RUVYXA_BUILD_CACHE_DIR"),
    )
}

pub(crate) fn resolve_build_cache_dir(
    root: &Path,
    configured: Option<&str>,
    environment: Option<OsString>,
) -> PathBuf {
    let selected = environment
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            configured
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
        });

    match selected {
        Some(path) if path.is_absolute() => path,
        Some(path) => root.join(path),
        None => root.join(".ruvyxa").join("cache").join("bundler"),
    }
}

pub(crate) fn diagnostic_stream(value: &str) -> String {
    if value.trim().is_empty() {
        "(empty)".to_string()
    } else {
        value.to_string()
    }
}
