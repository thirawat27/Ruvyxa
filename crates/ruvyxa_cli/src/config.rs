//! `ruvyxa.config.*` loading, validation, and the JavaScript config renderer.
//!
//! A Ruvyxa config may be TypeScript, so loading it means running a JavaScript
//! renderer and reading the result back as JSON. Every field is then validated
//! *here*, before anything downstream sees it: a limit that is merely wrong — a
//! body cap of zero, a rate-limit window of a billion seconds, a project path
//! that escapes the root — should fail as a config error the user can fix, not
//! as surprising behavior at request time.
//!
//! `deny_unknown_fields` is deliberate. A misspelled key is reported rather
//! than ignored, which is the difference between a security setting that is off
//! and one the user believes is on.

use std::path::{Path, PathBuf};

use ruvyxa_dev_server::{
    JavaScriptRuntime, MAX_ACTION_BODY_LIMIT_BYTES, MAX_ACTION_RATE_LIMIT_REQUESTS,
    MAX_ACTION_RATE_LIMIT_WINDOW_SECS, MAX_API_BODY_LIMIT_BYTES,
    MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES, TrustedProxies,
};
use ruvyxa_graph::{DiscoverOptions, I18nRouting, RenderStrategy, RouteManifest, discover_routes};

use crate::*;

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ProjectConfig {
    pub(crate) app_dir: Option<String>,
    pub(crate) out_dir: Option<String>,
    pub(crate) runtime: Option<BuildTarget>,
    /// Accepted so `deny_unknown_fields` does not reject a config that sets it,
    /// and read by nothing. Ruvyxa always renders React, so the flag selects no
    /// behaviour. Deprecated in `RuvyxaConfig`; do not wire it to anything
    /// without deciding what it should mean first.
    #[serde(rename = "react")]
    pub(crate) _react: Option<serde_json::Value>,
    /// Opt-in stable React Compiler transform for production module builds.
    pub(crate) react_compiler: Option<bool>,
    /// Accepted and read by nothing, for the same reason as `_react`. Note the
    /// config renderer does not even forward `typescript`, so this is always
    /// `None` in practice — type checking is `tsc`'s job against the project's
    /// own `tsconfig.json`.
    #[serde(rename = "typescript")]
    pub(crate) _typescript: Option<serde_json::Value>,
    /// Generate `.ruvyxa/types/routes.d.ts` so `<Link href>` is checked against
    /// the real route set. Off by default: the file is inert until the project
    /// `tsconfig.json` includes it, and turning it on for existing projects
    /// would only produce a hint they did not ask for.
    pub(crate) typed_routes: Option<bool>,
    #[serde(default, rename = "render")]
    pub(crate) rendering: RenderingConfigOptions,
    #[serde(default)]
    pub(crate) server: ServerConfigOptions,
    #[serde(default)]
    pub(crate) css: CssConfigOptions,
    /// Executable unified plugins stay in the JavaScript config module. This
    /// marker tells native bundling to activate the persistent MDX bridge.
    #[serde(rename = "markdown")]
    pub(crate) markdown_enabled: Option<bool>,
    #[serde(default)]
    pub(crate) build: BuildConfigOptions,
    #[serde(default)]
    pub(crate) debug: DebugConfigOptions,
    #[serde(default, rename = "image")]
    pub(crate) images: ImageOptimizationOptions,
    pub(crate) i18n: Option<I18nConfigOptions>,
    #[serde(default)]
    pub(crate) security: SecurityConfigOptions,
    #[serde(default)]
    pub(crate) cache: CacheConfigOptions,
    #[serde(default)]
    pub(crate) site: SiteConfigOptions,
    #[serde(rename = "content")]
    pub(crate) _content: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) middleware: ruvyxa_middleware::MiddlewareConfig,
    #[serde(default)]
    pub(crate) plugins: Vec<BuildPluginConfig>,
    #[serde(rename = "adapter")]
    pub(crate) adapter: Option<serde_json::Value>,
    #[serde(rename = "adapterOptions")]
    pub(crate) adapter_options: Option<serde_json::Value>,
    /// Identity of every build input that is not a project source file.
    ///
    /// The config's own dependency hash — its file, its transitive imports,
    /// the package manifests — folded together with the project environment,
    /// and it is named for the build rather than for the config because the
    /// second half is the half that was missing.
    ///
    /// `.env` decides emitted bytes exactly as directly as the config does:
    /// `import.meta.env` is substituted into every compiled module as a frozen
    /// literal, so a `RUVYXA_PUBLIC_*` value is *in* the browser bundle. Keyed
    /// on the config alone, editing one and rebuilding produced a build whose
    /// pre-rendered HTML carried the new value — `prerender_context_hash` has
    /// always keyed on the environment — and whose browser bundle carried the
    /// old one, from the compile cache. One build, two answers for the same
    /// variable, and the browser's is the one that survives hydration.
    #[serde(skip)]
    pub(crate) build_dependency_hash: String,
    #[serde(skip)]
    pub(crate) javascript_runtime_override: Option<JavaScriptRuntime>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ServerConfigOptions {
    pub(crate) host: Option<String>,
    pub(crate) port: Option<u16>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct I18nConfigOptions {
    pub(crate) locales: Vec<String>,
    pub(crate) default_locale: String,
    pub(crate) locale_param: Option<String>,
    pub(crate) detect_locale: Option<bool>,
    pub(crate) cookie: Option<String>,
}

impl I18nConfigOptions {
    pub(crate) fn routing(&self) -> I18nRouting {
        I18nRouting {
            locales: self.locales.clone(),
            default_locale: self.default_locale.clone(),
            locale_param: self
                .locale_param
                .clone()
                .unwrap_or_else(|| "lang".to_string()),
            detect_locale: self.detect_locale.unwrap_or(true),
            cookie: self
                .cookie
                .clone()
                .unwrap_or_else(|| "RUVYXA_LOCALE".to_string()),
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CssConfigOptions {
    #[serde(default)]
    pub(crate) entries: Vec<String>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct BuildConfigOptions {
    pub(crate) minify: Option<bool>,
    #[serde(rename = "map")]
    pub(crate) sourcemap: Option<bool>,
    #[serde(rename = "treeShake")]
    pub(crate) tree_shaking: Option<bool>,
    #[serde(rename = "split")]
    pub(crate) split_strategy: Option<String>,
    #[serde(rename = "workers")]
    pub(crate) parallelism: Option<usize>,
    #[serde(rename = "jsx")]
    pub(crate) jsx_runtime: Option<String>,
    /// JavaScript language level the emitted modules are written down to.
    ///
    /// Held as raw JSON rather than `Option<String>` so a non-string value is
    /// reported by `parse_es_target` with the accepted list beside it, instead
    /// of as a serde type error naming a Rust field.
    ///
    /// This key was inert for several releases: it was validated and carried
    /// into `BundleOptions`, and then neither transform consumed it — the Rust
    /// bundler built `TransformOptions::default()` and `runtime/compiler.mjs`
    /// hardcoded `target: 'esnext'`, so a project that set `target: "es2018"`
    /// got byte-identical esnext output and found out in the browser. Both
    /// compilers apply it now, and `tests/fixtures/es-target-conformance.json`
    /// holds the two to one accepted list.
    ///
    /// Downlevelling is not free of runtime support: oxc emits
    /// `@oxc-project/runtime/helpers/*` imports for transforms that need them
    /// and Ruvyxa ships no helper runtime. Which targets need one depends on
    /// the source rather than on the number — ordinary application code is
    /// helper-free at es2022 and above, a private class field pulls helpers in
    /// from es2021 down, and one `using` declaration needs one at every target
    /// below es2026 — so the refusal is on the emitted code
    /// (`compiler::reject_runtime_helpers`), not on the configured value.
    #[serde(rename = "target")]
    pub(crate) es_target: Option<serde_json::Value>,
    #[serde(rename = "manifest")]
    pub(crate) emit_chunk_manifest: Option<bool>,
    #[serde(rename = "warm")]
    pub(crate) prebundle_dependencies: Option<bool>,
    #[serde(rename = "prerenderCache")]
    pub(crate) prerender_cache: Option<bool>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RenderingConfigOptions {
    #[serde(rename = "strategy")]
    pub(crate) default_strategy: Option<RenderStrategy>,
    #[serde(rename = "revalidate")]
    pub(crate) default_revalidate: Option<u64>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct DebugConfigOptions {
    pub(crate) overlay: Option<bool>,
    pub(crate) traces: Option<bool>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SecurityConfigOptions {
    #[serde(rename = "actionLimit")]
    pub(crate) action_body_limit_bytes: Option<usize>,
    #[serde(rename = "apiLimit")]
    pub(crate) api_body_limit_bytes: Option<usize>,
    #[serde(rename = "pluginLimit")]
    pub(crate) plugin_response_body_limit_bytes: Option<usize>,
    #[serde(rename = "actionRateLimit")]
    pub(crate) action_rate_limit: Option<ActionRateLimitOptions>,
    #[serde(rename = "sameOrigin")]
    pub(crate) same_origin_actions: Option<bool>,
    #[serde(rename = "fetchMeta")]
    pub(crate) fetch_metadata_actions: Option<bool>,
    #[serde(default, rename = "trustedProxyIps")]
    pub(crate) trusted_proxy_ips: Vec<String>,
    #[serde(rename = "headers")]
    pub(crate) security_headers: Option<bool>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ActionRateLimitOptions {
    pub(crate) max: Option<usize>,
    pub(crate) window: Option<u64>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CacheConfigOptions {
    #[serde(rename = "routes")]
    pub(crate) route_manifest: Option<bool>,
    pub(crate) css: Option<bool>,
    #[serde(rename = "dir")]
    pub(crate) build_dir: Option<String>,
    /// A project module that answers "what is the cached document for this
    /// path", replacing whatever store the deploy target would otherwise use.
    ///
    /// The store a deployed build writes ISR documents to is a decision the
    /// platform usually makes: a Worker gets KV, a serverless function gets the
    /// only writable directory it has, and that directory is per-instance and
    /// per-deployment. Neither is wrong, and neither is something the framework
    /// can choose correctly for an application running several instances behind
    /// one domain — which needs one store all of them read.
    ///
    /// Read here only to be carried into the deployed bundle; the CLI does not
    /// load it. `documentCacheHandlerPrelude` in
    /// `packages/ruvyxa/runtime/adapter-runner.mjs` imports it into the route
    /// registry, which is the one module every adapter's handler already
    /// imports.
    pub(crate) handler: Option<String>,
    /// Entries the in-memory `cache()` tier holds before LRU eviction.
    ///
    /// `0` turns the tier off, which is what a deployment running several
    /// instances behind one shared store wants: a per-instance copy in front of
    /// a shared one is the thing that makes two instances disagree. Read into
    /// the deployed bundle by `documentCacheHandlerPrelude`, never by the CLI.
    pub(crate) max_entries: Option<u32>,
}

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BuildPluginConfig {
    pub(crate) name: String,
    /// Elements this plugin contributes to every rendered document's `<head>`.
    #[serde(default)]
    pub(crate) head: Vec<ruvyxa_dev_server::PluginHeadEntry>,
}

pub(crate) struct RuvyxaBuildCache<'a> {
    pub(crate) dependency_hash: &'a str,
    pub(crate) directory: &'a Path,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigRendererOutput {
    pub(crate) ok: bool,
    pub(crate) config: Option<ProjectConfig>,
    pub(crate) code: Option<String>,
    pub(crate) message: Option<String>,
    pub(crate) stack: Option<String>,
    pub(crate) dependency_hash: Option<String>,
    /// What the rendered result depends on, so the next run can decide whether
    /// it may reuse this one instead of starting a JavaScript runtime again.
    pub(crate) cache_key: Option<ConfigCacheKey>,
    /// What the renderer wrote to stderr, kept for the failure path.
    ///
    /// The process's output is captured rather than inherited, so on
    /// `ok: false` with no `message` this is the only remaining description of
    /// what went wrong. It used to be dropped and replaced with the literal
    /// `unknown config error`. Not part of the renderer's JSON — set by
    /// `parse_config_renderer_output` after parsing.
    #[serde(skip)]
    pub(crate) stderr: String,
}

/// The inputs one config render observed.
///
/// Reported by `config-renderer.mjs` rather than guessed here: the renderer is
/// the only side that knows which modules the config actually imported and
/// which environment variables it actually read.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConfigCacheKey {
    /// Project-relative paths of every file whose contents fed the dependency
    /// hash — the config, its transitive project imports, and the manifests.
    #[serde(default)]
    pub(crate) inputs: Vec<String>,
    /// Environment variables the config read while it was evaluated, with the
    /// value seen. `None` records a variable that was read while unset, which
    /// invalidates the cache just as surely when it later appears.
    #[serde(default)]
    pub(crate) env: std::collections::BTreeMap<String, Option<String>>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdapterRunnerOutput {
    pub(crate) ok: bool,
    pub(crate) result: Option<serde_json::Value>,
    pub(crate) code: Option<String>,
    pub(crate) message: Option<String>,
    pub(crate) stack: Option<String>,
}

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdapterInspection {
    pub(crate) name: String,
    pub(crate) target: String,
    pub(crate) runtime: String,
    pub(crate) platform: Option<String>,
    pub(crate) supports: Vec<String>,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AdapterArtifactReport {
    pub(crate) kind: String,
    pub(crate) path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) skipped: Option<bool>,
}

impl ProjectConfig {
    pub(crate) fn build_target(&self, cli_target: Option<BuildTarget>) -> BuildTarget {
        cli_target.or(self.runtime).unwrap_or(BuildTarget::Node)
    }

    pub(crate) fn javascript_runtime(&self) -> JavaScriptRuntime {
        self.javascript_runtime_override
            .unwrap_or_else(|| match self.runtime {
                Some(BuildTarget::Bun) => JavaScriptRuntime::Bun,
                Some(BuildTarget::Deno) => JavaScriptRuntime::Deno,
                Some(BuildTarget::Node | BuildTarget::Edge | BuildTarget::Static) => {
                    JavaScriptRuntime::Node
                }
                None => JavaScriptRuntime::detect(),
            })
    }

    pub(crate) fn app_dir(&self) -> &str {
        self.app_dir.as_deref().unwrap_or("app")
    }

    pub(crate) fn out_dir(&self) -> &str {
        self.out_dir.as_deref().unwrap_or(".ruvyxa")
    }

    /// Whether to emit `.ruvyxa/types/routes.d.ts` for this project.
    pub(crate) fn typed_routes(&self) -> bool {
        self.typed_routes.unwrap_or(false)
    }

    pub(crate) fn markdown_enabled(&self) -> bool {
        self.markdown_enabled.unwrap_or(false)
    }

    pub(crate) fn validate_paths(&self) -> anyhow::Result<()> {
        validate_project_relative_path("appDir", self.app_dir())?;
        validate_project_relative_path("outDir", self.out_dir())?;
        for entry in &self.css.entries {
            validate_project_relative_path("css.entries", entry)?;
        }
        validate_bounded_limit(
            "security.actionLimit",
            self.security.action_body_limit_bytes,
            MAX_ACTION_BODY_LIMIT_BYTES,
        )?;
        validate_bounded_limit(
            "security.apiLimit",
            self.security.api_body_limit_bytes,
            MAX_API_BODY_LIMIT_BYTES,
        )?;
        validate_plugin_response_limit(self.security.plugin_response_body_limit_bytes)?;
        if let Some(rate_limit) = &self.security.action_rate_limit {
            validate_bounded_limit(
                "security.actionRateLimit.max",
                rate_limit.max,
                MAX_ACTION_RATE_LIMIT_REQUESTS,
            )?;
            validate_bounded_limit(
                "security.actionRateLimit.window",
                rate_limit.window,
                MAX_ACTION_RATE_LIMIT_WINDOW_SECS,
            )?;
        }
        validate_trusted_proxy_ips(&self.security.trusted_proxy_ips)?;
        if let Some(i18n) = &self.i18n {
            validate_i18n(i18n)?;
        }
        if self.images.on_demand.enabled()
            && !(16..=8192).contains(&self.images.on_demand.max_width())
        {
            anyhow::bail!(
                "RUV1602 config field `image.onDemand.maxWidth` must be between 16 and 8192"
            );
        }
        validate_image_settings(&self.images)?;
        parse_jsx_runtime(self.build.jsx_runtime.as_deref())?;
        Ok(())
    }

    pub(crate) fn style_entries(&self, root: &Path) -> Vec<PathBuf> {
        let root = ruvyxa_diagnostics::normalized_canonical_path(root);
        self.css
            .entries
            .iter()
            .map(|entry| root.join(entry))
            .collect()
    }

    pub(crate) fn discover_options(&self, root: &Path) -> DiscoverOptions {
        DiscoverOptions::new(root.join(self.app_dir()))
            .with_rendering_defaults(
                self.rendering.default_strategy,
                self.rendering.default_revalidate,
            )
            .with_i18n(self.i18n.as_ref().map(I18nConfigOptions::routing))
    }
}

/// Reject image settings the optimizer would otherwise quietly reinterpret.
///
/// `quality` and `effort` are clamped at each of the three places they are
/// read, which is right as defence in depth and wrong as validation: a project
/// writing `quality: 150` got a quality-100 build and no diagnostic, while
/// every other out-of-range number in the file fails the build by name.
/// `workers` had neither — it reached `rayon::ThreadPoolBuilder` as written.
///
/// The clamps stay. This is the layer that gives the user the field name.
pub(crate) fn validate_image_settings(config: &ImageOptimizationOptions) -> anyhow::Result<()> {
    validate_bounded_limit(
        "image.quality",
        Some(config.quality),
        crate::image_optimizer::MAX_IMAGE_QUALITY,
    )?;
    // `effort: 0` is libwebp's fastest encode, and `workers: 0` documents "let
    // Rayon decide", so neither can go through `validate_bounded_limit` — it
    // rejects zero.
    if config.effort > crate::image_optimizer::MAX_IMAGE_EFFORT {
        anyhow::bail!(
            "RUV1602 config field `image.effort` must be between 0 and {}",
            crate::image_optimizer::MAX_IMAGE_EFFORT
        );
    }
    if config.parallelism > crate::image_optimizer::MAX_CONFIGURED_IMAGE_WORKERS {
        anyhow::bail!(
            "RUV1602 config field `image.workers` must be between 0 and {}",
            crate::image_optimizer::MAX_CONFIGURED_IMAGE_WORKERS
        );
    }
    Ok(())
}

pub(crate) fn validate_i18n(config: &I18nConfigOptions) -> anyhow::Result<()> {
    if config.locales.is_empty() || config.locales.len() > 32 {
        anyhow::bail!("RUV1602 config field `i18n.locales` must contain between 1 and 32 locales");
    }
    let mut normalized = std::collections::BTreeSet::new();
    for locale in &config.locales {
        if !valid_locale(locale) {
            anyhow::bail!("RUV1602 config field `i18n.locales` contains invalid locale `{locale}`");
        }
        if !normalized.insert(locale.to_ascii_lowercase()) {
            anyhow::bail!(
                "RUV1602 config field `i18n.locales` contains duplicate locale `{locale}`"
            );
        }
    }
    if !config
        .locales
        .iter()
        .any(|locale| locale.eq_ignore_ascii_case(&config.default_locale))
    {
        anyhow::bail!(
            "RUV1602 config field `i18n.defaultLocale` must be included in `i18n.locales`"
        );
    }
    let locale_param = config.locale_param.as_deref().unwrap_or("lang");
    if !valid_identifier(locale_param) {
        anyhow::bail!(
            "RUV1602 config field `i18n.localeParam` must be a JavaScript-style identifier"
        );
    }
    let cookie = config.cookie.as_deref().unwrap_or("RUVYXA_LOCALE");
    if cookie.is_empty()
        || cookie.len() > 128
        || !cookie.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
    {
        anyhow::bail!("RUV1602 config field `i18n.cookie` must be a valid HTTP cookie name");
    }
    Ok(())
}

fn valid_locale(locale: &str) -> bool {
    !locale.is_empty()
        && locale.len() <= 35
        && locale.split('-').all(|part| {
            !part.is_empty()
                && part.len() <= 8
                && part.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
}

fn valid_identifier(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphabetic() || matches!(byte, b'_' | b'$'))
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$'))
}

pub(crate) fn validate_positive_limit<T>(field: &str, value: Option<T>) -> anyhow::Result<()>
where
    T: PartialEq + From<u8>,
{
    if value.is_some_and(|value| value == T::from(0)) {
        anyhow::bail!("RUV1601 config field `{field}` must be greater than zero");
    }
    Ok(())
}

pub(crate) fn validate_bounded_limit<T>(
    field: &str,
    value: Option<T>,
    maximum: T,
) -> anyhow::Result<()>
where
    T: PartialOrd + PartialEq + From<u8> + std::fmt::Display + Copy,
{
    if let Some(value) = value {
        if value == T::from(0) {
            anyhow::bail!("RUV1601 config field `{field}` must be greater than zero");
        }
        if value > maximum {
            anyhow::bail!("RUV1602 config field `{field}` must not exceed {maximum}");
        }
    }
    Ok(())
}

pub(crate) fn validate_plugin_response_limit(value: Option<usize>) -> anyhow::Result<()> {
    validate_positive_limit("security.pluginLimit", value)?;
    if value.is_some_and(|value| value > MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES) {
        anyhow::bail!(
            "RUV1602 config field `security.pluginLimit` must not exceed {MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES} bytes"
        );
    }
    Ok(())
}

pub(crate) fn validate_trusted_proxy_ips(values: &[String]) -> anyhow::Result<()> {
    parse_trusted_proxies(values).map(|_| ())
}

/// Parse `security.trustedProxyIps` into matchable prefixes.
///
/// Accepts a CIDR range or a bare address, which is what the field has always
/// been documented to take. Parsing only exact `IpAddr` values rejected every
/// documented example (`10.0.0.0/8`) at startup with `RUV1602`, and left users
/// on container networks and managed platform edges — where the proxy address
/// is not stable enough to enumerate — with no way to declare their proxy at
/// all. Both server builders share this function so validation and the value
/// the server actually uses can never disagree.
pub(crate) fn parse_trusted_proxies(values: &[String]) -> anyhow::Result<TrustedProxies> {
    TrustedProxies::parse_all(values.iter().map(String::as_str)).map_err(|error| {
        anyhow::anyhow!("RUV1602 config field `security.trustedProxyIps` contains {error}")
    })
}

pub(crate) fn discover_project_routes(
    root: &Path,
    config: &ProjectConfig,
) -> anyhow::Result<RouteManifest> {
    discover_routes(config.discover_options(root)).map_err(Into::into)
}

pub(crate) fn validate_project_relative_path(field: &str, value: &str) -> anyhow::Result<()> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        anyhow::bail!("RUV1601 config field `{field}` must not be empty");
    }

    let path = Path::new(trimmed);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
                    | std::path::Component::ParentDir
            )
        })
    {
        anyhow::bail!(
            "RUV1601 config field `{field}` must be a project-relative path inside the project root"
        );
    }

    Ok(())
}
