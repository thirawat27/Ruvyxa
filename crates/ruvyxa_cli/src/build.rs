//! The `build` command: the full production build pipeline.
//!
//! One build runs, in order: config load, route discovery, server bundle,
//! client bundles, static prerendering, asset optimization, and manifest
//! emission. Each stage is timed and reported, and every stage that can be
//! skipped by an unchanged input is.
//!
//! Output never lands in place incrementally. The whole build renders into a
//! staging directory and is committed at the end (see [`crate::build_output`]),
//! so a failed or interrupted build leaves the previous `dist/` intact rather
//! than a half-written one that `start` would serve.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use std::time::{Duration, Instant};

use anyhow::Context;
use ruvyxa_diagnostics::Diagnostic;
use ruvyxa_graph::{RenderStrategy, RouteManifest, validate_app, write_manifest};
use tracing::{info, warn};

use crate::*;

pub(crate) async fn build(args: BuildArgs) -> anyhow::Result<()> {
    build_with_output(args, true).await
}

/// Build targets that have an agreed server-only output contract.
///
/// `static` has no server to run at all, and `edge` adapters each stage their
/// own function payload from the full output; neither has been given a minimal
/// contract yet, so both are rejected rather than silently producing an
/// artifact that cannot be deployed.
pub(crate) const SERVER_ONLY_TARGETS: [&str; 3] = ["node", "bun", "deno"];

/// Reject a `--server-only` build whose target has no server-only contract.
///
/// Checked before any staging directory is created: a build that cannot
/// produce a deployable artifact must not write output at all.
pub(crate) fn server_only_target_diagnostic(target: BuildTarget) -> Option<Diagnostic> {
    let name = format!("{target:?}").to_lowercase();
    if SERVER_ONLY_TARGETS.contains(&name.as_str()) {
        return None;
    }
    Some(
        Diagnostic::new(
            "RUV1211",
            format!("--server-only does not support the `{name}` target"),
        )
        .explain(
            "A server-only artifact is a running Node, Bun, or Deno server. The static and edge targets \
             have no server-only output contract yet, so the build would emit an artifact that \
             cannot be deployed.",
        )
        .suggest(format!(
            "Build with `--target node`, `--target bun`, or `--target deno`, or drop --server-only to produce the \
             full {name} output."
        )),
    )
}

/// Reject a `--server-only` build that contains a page route.
///
/// Silently omitting pages would produce a deployment that starts successfully
/// and then serves 404 for every page — a defect that only appears in
/// production. Failing here keeps it a build-time error.
///
/// The offending route is chosen by sorted path so the same project always
/// reports the same first path, whatever order discovery walked the tree in.
pub(crate) fn server_only_page_route_diagnostic(manifest: &RouteManifest) -> Option<Diagnostic> {
    let mut pages = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .collect::<Vec<_>>();
    pages.sort_by(|left, right| left.path.cmp(&right.path));
    let first = pages.first()?;

    let remaining = pages.len() - 1;
    let diagnostic = Diagnostic::new(
        "RUV1210",
        format!("--server-only cannot build page route \"{}\"", first.path),
    )
    .explain(if remaining == 0 {
        "A server-only artifact has no client bundles, page CSS, or prerendered HTML, so this \
         page would return 404 in the deployed application."
            .to_string()
    } else {
        format!(
            "A server-only artifact has no client bundles, page CSS, or prerendered HTML, so this \
             page would return 404 in the deployed application. {remaining} more page route(s) \
             were discovered."
        )
    })
    .suggest(
        "Move the endpoint to app/api/<name>/route.ts, or run a normal Node/Bun build without \
         --server-only.",
    )
    .at_file(first.file.clone());
    Some(diagnostic)
}

/// Run every `--server-only` compatibility rule before output staging begins.
pub(crate) fn ensure_server_only_supported(
    target: BuildTarget,
    manifest: &RouteManifest,
) -> anyhow::Result<()> {
    let diagnostics = [
        server_only_target_diagnostic(target),
        server_only_page_route_diagnostic(manifest),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    fail_on_diagnostics(&diagnostics)
}

pub(crate) struct PreparedBuildAssets {
    pub(crate) styles: ruvyxa_dev_server::StyleCollection,
    pub(crate) images: ImageOptimizationReport,
    pub(crate) asset_files: usize,
    pub(crate) duration: Duration,
}

/// Owns an in-progress staging tree so every pre-commit error path cleans it.
///
/// A successful commit moves the named outputs and removes the now-empty tree;
/// the existence check keeps the guard a no-op on that path.
pub(crate) struct BuildStagingCleanup {
    pub(crate) path: PathBuf,
}

impl BuildStagingCleanup {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for BuildStagingCleanup {
    fn drop(&mut self) {
        if self.path.exists()
            && let Err(error) = fs::remove_dir_all(&self.path)
        {
            warn!(
                path = %self.path.display(),
                %error,
                "failed to clean incomplete build staging directory"
            );
        }
    }
}

/// What the style half of asset preparation needs.
///
/// The two travel together because neither answers anything on its own: the
/// entries say which stylesheets to collect, and the runtime is what executes
/// the project's PostCSS chain over them — that chain is the project's own
/// JavaScript, loaded from the project's own dependencies.
#[derive(Debug, Clone, Copy)]
pub(crate) struct StyleStage<'a> {
    /// `None` skips style collection entirely: a server-only artifact renders no
    /// HTML document, so collected CSS has nothing to be inlined into and no
    /// stylesheet to be emitted as. The project's own source files are still
    /// staged — an API route may import a module that sits next to a stylesheet.
    pub(crate) entries: Option<&'a [PathBuf]>,
    pub(crate) runtime: ruvyxa_dev_server::JavaScriptRuntime,
}

pub(crate) fn prepare_build_assets(
    root: &Path,
    app_dir: &Path,
    server_dir: &Path,
    assets_dir: &Path,
    styles_stage: StyleStage<'_>,
    image_cache_dir: &Path,
    image_options: &ImageOptimizationOptions,
) -> anyhow::Result<PreparedBuildAssets> {
    let started = Instant::now();
    let (styles, images) = std::thread::scope(|scope| -> anyhow::Result<_> {
        let styles = scope.spawn(|| match styles_stage.entries {
            Some(entries) => ruvyxa_dev_server::collect_styles_for_build(
                root,
                app_dir,
                entries,
                styles_stage.runtime,
            ),
            None => Ok(ruvyxa_dev_server::StyleCollection::default()),
        });
        let app_copy = scope.spawn(|| copy_dir_all(app_dir, &server_dir.join("app")));
        let components_copy = scope
            .spawn(|| copy_optional_dir(&root.join("components"), &server_dir.join("components")));
        let server_copy =
            scope.spawn(|| copy_optional_dir(&root.join("server"), &server_dir.join("server")));
        let images = scope.spawn(|| {
            optimize_public_images(
                &root.join("public"),
                assets_dir,
                image_cache_dir,
                image_options,
            )
        });

        // Join in the original phase order so simultaneous failures remain
        // deterministic even though the independent work runs concurrently.
        let styles = styles
            .join()
            .map_err(|_| anyhow::anyhow!("style collection worker panicked"))??;
        app_copy
            .join()
            .map_err(|_| anyhow::anyhow!("application copy worker panicked"))??;
        components_copy
            .join()
            .map_err(|_| anyhow::anyhow!("component copy worker panicked"))??;
        server_copy
            .join()
            .map_err(|_| anyhow::anyhow!("server copy worker panicked"))??;
        let images = images
            .join()
            .map_err(|_| anyhow::anyhow!("image optimization worker panicked"))??;
        Ok((styles, images))
    })?;

    // Style sources can overlap app/components destinations, so copy them only
    // after the directory workers finish instead of racing writes to one file.
    copy_project_sources(root, server_dir, &styles.files)?;
    let asset_files = count_files(assets_dir);

    Ok(PreparedBuildAssets {
        styles,
        images,
        asset_files,
        duration: started.elapsed(),
    })
}

/// Print the fixed banner a summarized build opens with.
fn print_build_header(args: &BuildArgs, target: BuildTarget, app_dir: &Path, out_dir: &Path) {
    print_header("Build");
    print_field("target", accent(format!("{:?}", target).to_lowercase()));
    print_field(
        "profile",
        accent(if args.server_only {
            "production · server-only"
        } else {
            "production"
        }),
    );
    print_field("root", path_text(&args.root));
    print_field("app dir", path_text(app_dir));
    print_field("out dir", path_text(out_dir));
    println!();
}

/// Report raw `<img>` references that bypass the image pipeline.
///
/// Capped at five so a project that never adopted `<Image>` reports a problem
/// rather than burying the rest of the build output under one per file.
fn warn_bypassed_images(bypassed: &[crate::image_usage::RawImageUsage], keep_original: bool) {
    for usage in bypassed.iter().take(5) {
        if keep_original {
            warn!(
                "{}:{} <img src=\"{}\"> ships {} instead of the generated WebP ({}). Use <Image> from @ruvyxa/react to serve the optimized file.",
                usage.file.display(),
                usage.line,
                usage.url,
                format_bytes(usage.source_bytes as usize),
                format_bytes(usage.webp_bytes as usize),
            );
        } else {
            warn!(
                "{}:{} <img src=\"{}\"> references an original image that is not published. Use <Image> from @ruvyxa/react or reference the generated WebP.",
                usage.file.display(),
                usage.line,
                usage.url,
            );
        }
    }
    if bypassed.len() > 5 {
        warn!(
            "{} more raw <img> references bypass the image pipeline.",
            bypassed.len() - 5
        );
    }
}

/// One-line summary of what asset preparation produced.
fn assets_phase_detail(asset_files: usize, optimized_images: usize) -> String {
    let mut detail = format!("{asset_files} files");
    if optimized_images > 0 {
        let plural = if optimized_images == 1 { "" } else { "s" };
        detail.push_str(&format!(" · {optimized_images} optimized image{plural}"));
    }
    detail
}

/// One-line summary naming the adapter that ran, and how it was chosen.
fn adapter_phase_detail(
    args: &BuildArgs,
    detected_adapter: &Option<(String, String)>,
    artifact_count: usize,
) -> String {
    match (&args.adapter, detected_adapter) {
        (Some(name), _) => name.clone(),
        (None, Some((name, source))) => format!("{name} (auto via {source})"),
        (None, None) => format!("{artifact_count} artifact(s)"),
    }
}

/// Write `robots.txt` and `sitemap.xml` for a build that serves pages.
///
/// A missing site URL is a warning rather than an error: a project can build
/// and deploy without one, it just cannot advertise absolute URLs.
fn write_discovery_stage(
    config: &ProjectConfig,
    manifest: &RouteManifest,
    prerendered_paths: &[String],
    assets_dir: &Path,
    site_url: Option<&str>,
) -> anyhow::Result<()> {
    let discovery = write_discovery_files(
        manifest,
        prerendered_paths,
        assets_dir,
        site_url,
        &config.site,
    )
    .with_context(|| {
        format!(
            "failed to write discovery files into {}",
            assets_dir.display()
        )
    })?;
    if discovery.sitemap_needs_site_url {
        warn!(
            "no production site URL is configured, so sitemap.xml was not generated. Set `site.url`, RUVYXA_SITE_URL, or a supported production host URL environment variable."
        );
    }
    Ok(())
}

/// Run the deploy adapter over the committed output, when one applies.
///
/// Returns the artifact list to record in `build.json`, or `None` when neither
/// the config, the CLI flag, nor platform detection selected an adapter.
/// Resolve the selected adapter before the build starts.
///
/// The adapter runs last, so an adapter that cannot be resolved used to be
/// reported after compiling, bundling, and pre-rendering the whole project —
/// `--adapter herkou` spent an entire build to answer a question its own
/// spelling had already settled.
///
/// The CLI cannot answer it from the name alone, and deliberately so: any npm
/// package may be an adapter, so `main.rs` accepts anything shaped like a
/// package name and leaves resolution to the runner. That is the right trade,
/// and it does not require waiting until the end — `inspect` mode loads the
/// adapter and reports `RUV2203` for one that is missing, and it does so
/// without reading the build output, which does not exist yet at this point.
///
/// Nothing is done with the inspection: this is a precondition, and the
/// adapter stage inspects again for the values it needs after the output is
/// committed.
/// Only a *named* adapter is checked. An auto-detected one comes from the
/// hosting platform's own environment and is always one of the built-in names,
/// so it cannot be a typo — and detection has not run this early anyway.
fn ensure_selected_adapter_resolves(
    args: &BuildArgs,
    config: &ProjectConfig,
    out_dir: &Path,
) -> anyhow::Result<()> {
    if args.adapter.is_none() && config.adapter.is_none() {
        return Ok(());
    }
    // The same pair `run_adapter_stage` passes: a `--adapter` name, or `None`
    // when the choice lives in `ruvyxa.config.ts` and the runner reads it
    // itself. `config.adapter` is a whole JSON value, not a name, because it
    // may carry adapter options alongside the name.
    inspect_adapter(
        &args.root,
        out_dir,
        config.javascript_runtime(),
        args.adapter.as_deref(),
    )
    .map(|_| ())
}

fn run_adapter_stage(
    args: &BuildArgs,
    config: &ProjectConfig,
    out_dir: &Path,
    detected_adapter: &Option<(String, String)>,
    show_summary: bool,
    revalidating_routes: bool,
) -> anyhow::Result<Option<serde_json::Value>> {
    if config.adapter.is_none() && args.adapter.is_none() && detected_adapter.is_none() {
        return Ok(None);
    }
    let phase_started = Instant::now();
    let spinner = start_build_phase(show_summary, "adapter");
    let named_adapter = args
        .adapter
        .as_deref()
        .or_else(|| detected_adapter.as_ref().map(|(name, _)| name.as_str()));
    // The runner resolves `config.adapter` itself, so it is not passed to it —
    // but a report about the adapter has to know the name the project declared,
    // and this is the chain the deploy manifest already resolves it with. An
    // object-form `adapter:` stays unnamed here, which is what keeps the report
    // below quiet rather than guessing.
    let reported_adapter =
        named_adapter.or_else(|| config.adapter.as_ref().and_then(serde_json::Value::as_str));
    let artifacts = run_adapter_runner(
        &args.root,
        out_dir,
        config.javascript_runtime(),
        named_adapter,
    )?;
    if show_summary {
        let detail = adapter_phase_detail(args, detected_adapter, artifacts.len());
        print_build_phase(spinner, "adapter", detail, phase_started.elapsed());
    }
    report_native_only_image_optimization(config, reported_adapter);
    report_stale_revalidation(revalidating_routes, reported_adapter);
    Ok(Some(serde_json::to_value(artifacts)?))
}

/// The adapters whose generated handler serves `/__ruvyxa/image`.
///
/// `tests/fixtures/adapter-contract.json` is the table, compiled in rather than
/// restated, because this was three restatements and every one of them was
/// wrong. `image.onDemand` was recorded as an unconditional two-host gap — the
/// Axum host resizes through the Rust image pipeline, no build artifact carries
/// it — and then `@ruvyxa/adapter-vercel` and `@ruvyxa/adapter-cloudflare` began
/// passing `createHandler` an `optimizeImage` that forwards to the platform's
/// own optimizer. The capability became adapter-conditional and nothing could
/// say so, so `ruvyxa build` told a Vercel project its working images answered
/// 404, `ruvyxa test:parity` agreed, and the guide said so in both languages
/// while the deployment matrix two chapters away said "adapter-dependent".
///
/// `records onDemandImages for exactly the adapters that pass an optimizer` in
/// `tests/packages/ruvyxa/serverless-shared-tables.test.mjs` holds the table to
/// the adapter sources that decide it.
///
/// `None` is "the table has no entry", which is a third-party adapter package —
/// deliberately not folded into `false`. Only an entry that says `false` is a
/// statement that the endpoint answers 404 there, and only that may be reported.
pub(crate) fn adapter_on_demand_images(adapter: &str) -> Option<bool> {
    const CONTRACT: &str = include_str!("../../../tests/fixtures/adapter-contract.json");
    let contract: serde_json::Value = serde_json::from_str(CONTRACT).ok()?;
    contract
        .get("adapters")?
        .as_array()?
        .iter()
        .find(|candidate| candidate["name"].as_str() == Some(adapter))?
        .get("onDemandImages")?
        .as_bool()
}

/// Whether `revalidatePath()` reaches what a visitor is served on this platform.
///
/// Read from `tests/fixtures/adapter-contract.json` for the same reason
/// [`adapter_on_demand_images`] is: the capability is a property of the platform
/// the adapter targets, not of anything this crate can inspect, and a restated
/// list is only correct until somebody forgets it.
///
/// `None` is "the table has no entry" — an object-form `adapter:` or a
/// third-party package — and stays distinct from `Some(false)`, which is the
/// only value that may be reported.
pub(crate) fn adapter_on_demand_revalidation(adapter: &str) -> Option<bool> {
    const CONTRACT: &str = include_str!("../../../tests/fixtures/adapter-contract.json");
    let contract: serde_json::Value = serde_json::from_str(CONTRACT).ok()?;
    contract
        .get("adapters")?
        .as_array()?
        .iter()
        .find(|candidate| candidate["name"].as_str() == Some(adapter))?
        .get("onDemandRevalidation")?
        .as_bool()
}

/// Say that on-demand image optimization does not survive into this artifact.
///
/// `image.onDemand` is served at `/__ruvyxa/image` by the Axum host, which
/// resizes through the Rust image pipeline. A build emits no such pipeline, so
/// unless the adapter hands `createHandler` an optimizer of its own — see
/// [`adapter_serves_on_demand_images`] — the endpoint answers 404 in the
/// deployment. A project that turns the option on then gets working responsive
/// images under `ruvyxa dev` and `ruvyxa start`, and on the deployed site every
/// `srcSet` entry 404s. The browser falls back to `src`, so the page still
/// renders and the only symptom is a phone downloading a full-size image, which
/// is why nothing surfaced it.
///
/// Silent when the adapter cannot be named — an inline `adapter:
/// vercelAdapter({…})` is an object, not a name, and a third-party package is
/// not in the table at all. A wrong answer errs in one direction here: telling a
/// working deployment it is broken sends the reader to disable a feature that
/// works, while saying nothing costs one full-size download.
///
/// Reported rather than refused, exactly like `RUV2205`: a deployment without
/// the endpoint is still a valid deployment, and a project may enable the
/// option for development on purpose.
/// Whether the report below has something true to say.
///
/// Separated from the printing so it can be asserted: the report is a warning
/// nobody's build fails on, so a condition that silently drifts back to "always"
/// is exactly the kind of thing that shipped here for two adapters.
pub(crate) fn should_report_native_only_images(
    config: &ProjectConfig,
    adapter: Option<&str>,
) -> bool {
    config.images.on_demand.enabled() && adapter.and_then(adapter_on_demand_images) == Some(false)
}

/// Whether the revalidation report below has something true to say.
///
/// Separated from the printing so it can be asserted, the same way
/// [`should_report_native_only_images`] is: the report is a warning nobody's
/// build fails on, which is exactly the kind of condition that drifts back to
/// "always" without anyone noticing.
pub(crate) fn should_report_stale_revalidation(
    revalidating_routes: bool,
    adapter: Option<&str>,
) -> bool {
    revalidating_routes && adapter.and_then(adapter_on_demand_revalidation) == Some(false)
}

/// Say that `revalidatePath()` will not reach a reader on this platform.
///
/// The forced write still lands in the function's own store. What it cannot do
/// is drop the copy the platform is serving, because the platform caches the
/// response on the `s-maxage` the handler sends and offers the code inside no
/// way to invalidate one path. The page therefore keeps answering with the
/// document it had until the window expires on its own — which is a correct
/// deployment, just not the one the call implies.
fn report_stale_revalidation(revalidating_routes: bool, adapter: Option<&str>) {
    if !should_report_stale_revalidation(revalidating_routes, adapter) {
        return;
    }
    let Some(adapter) = adapter else {
        return;
    };
    warn!(
        adapter,
        "revalidatePath() cannot drop a cached document on this platform"
    );
    println!(
        "  {} {}",
        warn_text("warn"),
        dim(format!(
            "the {adapter} platform caches this build's responses and exposes no per-path \
             purge, so revalidatePath() refreshes the function's own store while readers keep \
             the previous document until `revalidate` elapses. Shorten `revalidate` for a page \
             that must change on demand, or deploy it through an adapter that implements the \
             platform's purge (vercel, netlify, cloudflare)."
        ))
    );
}

fn report_native_only_image_optimization(config: &ProjectConfig, adapter: Option<&str>) {
    if !should_report_native_only_images(config, adapter) {
        return;
    }
    let Some(adapter) = adapter else {
        return;
    };
    warn!(
        adapter,
        "on-demand image optimization is not served by a build artifact"
    );
    println!(
        "  {} {}",
        warn_text("warn"),
        dim(format!(
            "image.onDemand is enabled, and the {adapter} artifact carries no image pipeline, \
             so /__ruvyxa/image answers 404 there. A page keeps rendering because the browser \
             falls back to `src`, so the cost is a full-size download rather than an error. \
             Pre-build the sizes with image.variantWidths, deploy through an adapter that \
             forwards to a platform optimizer (vercel, cloudflare), or serve the project with \
             `ruvyxa start`."
        ))
    );
}

pub(crate) async fn build_with_output(args: BuildArgs, show_summary: bool) -> anyhow::Result<()> {
    build_with_cache_override(args, show_summary, None).await
}

/// Ask the `react-server` graph for each server-components route's browser
/// entry, ahead of the bundling scope that needs the answer.
///
/// `None` for a server-only build, which emits no browser entry at all.
fn spawn_server_component_entry_collection(
    args: &BuildArgs,
    config: &ProjectConfig,
    app_dir: &Path,
    manifest: &RouteManifest,
    build_cache_directory: &Path,
) -> Option<tokio::task::JoinHandle<anyhow::Result<crate::client_bundle::ServerComponentEntries>>> {
    if args.server_only {
        return None;
    }
    let root = args.root.clone();
    let app_dir = app_dir.to_path_buf();
    let manifest = manifest.clone();
    let build = config.build.clone();
    let runtime = config.javascript_runtime();
    let worker_env = build_worker_env(&args.root, &config.build, runtime);
    let cache_directory = build_cache_directory.to_path_buf();
    let dependency_hash = config.build_dependency_hash.clone();
    Some(tokio::spawn(async move {
        // The context hash needs the same environment the worker will be
        // started with; a failure to assemble it is reported by the collection
        // itself, so caching is simply skipped here.
        let cache = worker_env.ok().map(|worker_env| ServerComponentEntryCache {
            directory: cache_directory,
            dependency_hash,
            context_hash: crate::artifact_cache::server_component_context_hash(
                &root,
                runtime,
                &worker_env,
            ),
            fingerprints: std::sync::Arc::new(ArtifactFingerprintCache::default()),
        });
        collect_server_component_entries(
            &root,
            &app_dir,
            &manifest,
            &build,
            runtime,
            cache.as_ref(),
        )
        .await
    }))
}

/// Start the pre-render worker pool ahead of the pre-render phase.
///
/// See [`start_static_params_worker_pool`] for why the phase cannot start it
/// any later than its own first statement, and why that made a fully cached
/// build pay for a Node process to do nothing but enumerate paths.
fn spawn_static_params_worker_pool(
    args: &BuildArgs,
    config: &ProjectConfig,
    manifest: &RouteManifest,
) -> Option<
    tokio::task::JoinHandle<
        anyhow::Result<Option<std::sync::Arc<ruvyxa_dev_server::NodeWorkerPool>>>,
    >,
> {
    if args.server_only {
        return None;
    }
    let root = args.root.clone();
    let manifest = manifest.clone();
    let build = config.build.clone();
    let runtime = config.javascript_runtime();
    Some(tokio::spawn(async move {
        start_static_params_worker_pool(&root, &manifest, &build, runtime).await
    }))
}

/// How long each reported build phase took.
pub(crate) struct BuildPhaseTiming {
    pub(crate) route_discovery: Duration,
    pub(crate) validation: Duration,
    pub(crate) preparation: Duration,
    pub(crate) client_bundle: Duration,
    pub(crate) prerender: Duration,
}

/// Everything `build.json` states about a finished build.
///
/// Assembling the document is pure data collection: no stage reads it and no
/// decision depends on it, so it is gathered apart from the pipeline that
/// produced the values. That keeps the pipeline function about the order work
/// happens in, which is the only thing its length is worth spending on.
pub(crate) struct BuildReport<'a> {
    pub(crate) target: BuildTarget,
    pub(crate) args: &'a BuildArgs,
    pub(crate) config: &'a ProjectConfig,
    pub(crate) manifest: &'a RouteManifest,
    pub(crate) client_manifest: &'a serde_json::Value,
    pub(crate) image_report: &'a ImageOptimizationReport,
    pub(crate) prerendered: &'a [PrerenderedRoute],
    pub(crate) detected_adapter: Option<&'a (String, String)>,
    pub(crate) timing: BuildPhaseTiming,
}

impl BuildReport<'_> {
    fn document(&self) -> serde_json::Value {
        let Self {
            target,
            args,
            config,
            manifest,
            client_manifest,
            image_report,
            prerendered,
            detected_adapter,
            timing,
        } = self;
        serde_json::json!({
            "framework": "Ruvyxa",
            "version": env!("CARGO_PKG_VERSION"),
            "target": format!("{:?}", target).to_lowercase(),
            "profile": "production",
            "createdAtUnix": SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or_default(),
            "routes": manifest.routes.len(),
            "serverOnly": args.server_only,
            "serverDir": "server",
            // Null rather than "client": a server-only artifact has no client
            // directory, and an adapter or operator reading this file must not be
            // pointed at a path the build never wrote.
            "clientDir": if args.server_only { serde_json::Value::Null } else { serde_json::json!("client") },
            "assetsDir": "assets",
            "adapter": args
                .adapter
                .as_deref()
                .map(|name| serde_json::json!(name))
                .or_else(|| config.adapter.clone())
                .or_else(|| detected_adapter
                    .map(|(name, _)| serde_json::json!(name))),
            "adapterOptions": config.adapter_options.clone(),
            "images": image_report,
            "runtime": {
                "middleware": config.middleware,
                "i18n": manifest.i18n,
                "image": {
                    "onDemand": config.images.on_demand.enabled(),
                    "maxWidth": config.images.on_demand.max_width(),
                    // The quality an absent `q` resolves to. The native host has
                    // always answered that with `image.quality`; this block
                    // carried `onDemand` and `maxWidth` and not the value beside
                    // them, so an adapter that can optimize had nothing to read
                    // and fell back to the handler's own constant. Same URL, two
                    // different re-encodes, decided by where it was deployed.
                    "quality": config.images.quality,
                    "sizes": config.images.variant_widths
                }
            },
            "hashAlgorithm": ASSET_HASH_ALGORITHM,
            "security": {
                "actionLimit": config.security.action_body_limit_bytes.unwrap_or(1024 * 1024),
                "apiLimit": config.security.api_body_limit_bytes.unwrap_or(10 * 1024 * 1024),
                "pluginLimit": config.security.plugin_response_body_limit_bytes.unwrap_or(32 * 1024 * 1024),
                "actionRateLimit": {
                    "max": config.security.action_rate_limit.as_ref().and_then(|value| value.max).unwrap_or(600),
                    "window": config.security.action_rate_limit.as_ref().and_then(|value| value.window).unwrap_or(60)
                },
                "sameOrigin": config.security.same_origin_actions.unwrap_or(true),
                "fetchMeta": config.security.fetch_metadata_actions.unwrap_or(true),
                "trustedProxyIps": config.security.trusted_proxy_ips,
                "headers": config.security.security_headers.unwrap_or(true)
            },
            "build": {
                "minify": config.build.minify.unwrap_or(true),
                "map": config.build.sourcemap.unwrap_or(false),
                "treeShake": config.build.tree_shaking.unwrap_or(true),
                "split": config.build.split_strategy.as_deref().unwrap_or("route"),
                "jsx": config.build.jsx_runtime.as_deref().unwrap_or("automatic"),
                "manifest": config.build.emit_chunk_manifest.unwrap_or(false),
                "warm": config.build.prebundle_dependencies.unwrap_or(true),
                "prerenderCache": config.build.prerender_cache.unwrap_or(true),
                "workers": client_manifest.get("parallelism").cloned().unwrap_or(serde_json::Value::Null)
            },
            "render": {
                "prerendered": prerendered.len(),
                "routes": prerendered.iter().map(|p| serde_json::json!({
                    "path": p.path,
                    "strategy": format!("{:?}", p.strategy).to_lowercase(),
                    "revalidate": p.revalidate,
                    "cacheHit": p.artifact_cache_hit,
                })).collect::<Vec<_>>()
            },
            "timing": {
                "routeDiscoveryMs": duration_ms(timing.route_discovery),
                "validationMs": duration_ms(timing.validation),
                "preparationMs": duration_ms(timing.preparation),
                "clientBundleMs": duration_ms(timing.client_bundle),
                "prerenderMs": duration_ms(timing.prerender)
            }
        })
    }
}

/// Collect the deferred pool start, naming the task if it panicked.
///
/// `Ok(None)` covers both "no task was started" and "no route needed a pool",
/// which pre-rendering treats the same way: it starts one itself if a render
/// turns out to miss the artifact cache.
async fn awaited_static_params_pool(
    task: Option<
        tokio::task::JoinHandle<
            anyhow::Result<Option<std::sync::Arc<ruvyxa_dev_server::NodeWorkerPool>>>,
        >,
    >,
) -> anyhow::Result<Option<std::sync::Arc<ruvyxa_dev_server::NodeWorkerPool>>> {
    let Some(task) = task else {
        return Ok(None);
    };
    task.await
        .map_err(|error| anyhow::anyhow!("static-parameter worker start failed: {error}"))?
}

/// Run a production build with an optional isolated artifact-cache directory.
///
/// Normal CLI builds pass `None` and retain the configured cache contract. The
/// benchmark harness supplies a private directory so a cold sample never
/// deletes, warms, or otherwise changes the application's real build cache.
pub(crate) async fn build_with_cache_override(
    args: BuildArgs,
    show_summary: bool,
    cache_directory: Option<&Path>,
) -> anyhow::Result<()> {
    let started = Instant::now();
    let config = load_project_config(&args.root)?;
    // The project's `.env` values, recorded for the compilers that substitute
    // `import.meta.env`. Read here rather than from this process's own
    // environment: a build hands env values to the workers it spawns and never
    // adopts them itself, so a client bundle asking `std::env` saw none of the
    // project's public values.
    ruvyxa_bundler::compiler::set_public_env(ruvyxa_dev_server::project_env(&args.root)?);
    let target = config.build_target(args.target);
    let app_dir = args.root.join(config.app_dir());
    let out_dir = args.root.join(config.out_dir());
    let build_cache_directory = cache_directory
        .map(Path::to_path_buf)
        .unwrap_or_else(|| build_cache_dir(&args.root, &config.cache));
    // Before any phase runs: the adapter is the last thing a build uses and the
    // first thing it can check.
    ensure_selected_adapter_resolves(&args, &config, &out_dir)?;

    if show_summary {
        print_build_header(&args, target, &app_dir, &out_dir);
    }

    let ValidatedRoutes {
        manifest,
        discovery_duration: route_discovery_duration,
        validation_duration,
    } = discover_and_validate_routes(&args, &config, target, show_summary)?;
    // Both started here and awaited at the phase that consumes them. Each is
    // dominated by a JavaScript runtime coming up and neither needs anything
    // the phases in between produce, so run in sequence they were three of the
    // largest steps of a warm build and overlapped they cost the slowest one.
    // Spawned rather than joined, so the plugin host's blocking start below
    // does not hold them.
    let rsc_entries_task = spawn_server_component_entry_collection(
        &args,
        &config,
        &app_dir,
        &manifest,
        &build_cache_directory,
    );
    let static_params_pool_task = spawn_static_params_worker_pool(&args, &config, &manifest);
    let plugin_session = TypeScriptPluginBuildSession::new(
        &args.root,
        &config.plugins,
        config.javascript_runtime(),
        config.markdown_enabled(),
        config.react_compiler.unwrap_or(false),
    )?;
    plugin_session.run_start(&out_dir)?;
    let staging = BuildStagingLayout::create(&out_dir, &build_cache_directory)?;
    let _staging_cleanup = BuildStagingCleanup::new(staging.root.clone());
    let BuildStagingLayout {
        root: staging_dir,
        server_dir,
        client_dir,
        assets_dir,
        image_cache_dir,
    } = staging;
    let style_entries = (!args.server_only).then(|| config.style_entries(&args.root));
    // `public/` is a URL contract for an API-only service too, so its files are
    // still staged. What is skipped is the browser-facing part: WebP conversion
    // output exists for `<Image>`, which a server-only artifact never renders.
    let image_options = if args.server_only {
        ImageOptimizationOptions {
            optimize: false,
            ..config.images.clone()
        }
    } else {
        config.images.clone()
    };
    if !args.server_only {
        fs::create_dir_all(&client_dir)?;
    }
    write_manifest(&manifest, &staging_dir.join("manifest.json"))?;

    // Asset preparation and client bundling both read the immutable project
    // snapshot but write disjoint staging trees. Overlap them, then reduce
    // results in the historical phase order so errors and output stay stable.
    // One live line covers both workers: they overlap, so two spinners would
    // fight over the same terminal row.
    // Collected before the bundling scope below, which is synchronous: asking
    // the `react-server` graph what a server-components route's browser entry
    // contains needs a worker, and the answer has to be in hand before the
    // shared-chunk analysis runs. An app with no such route starts no worker.
    let rsc_entries = match rsc_entries_task {
        Some(task) => task.await.map_err(|error| {
            anyhow::anyhow!("server-components entry collection failed: {error}")
        })??,
        None => crate::client_bundle::ServerComponentEntries::default(),
    };

    let spinner = start_build_phase(show_summary, "bundling");
    let ((prepared_assets, client_manifest), client_bundle_duration) =
        std::thread::scope(|scope| -> anyhow::Result<_> {
            let preparation = scope.spawn(|| {
                prepare_build_assets(
                    &args.root,
                    &app_dir,
                    &server_dir,
                    &assets_dir,
                    StyleStage {
                        entries: style_entries.as_deref(),
                        runtime: config.javascript_runtime(),
                    },
                    &image_cache_dir,
                    &image_options,
                )
            });
            let bundle_started = Instant::now();
            let client_manifest = if args.server_only {
                Ok(serde_json::json!({ "routes": [] }))
            } else {
                emit_client_bundles_with_session(
                    &args.root,
                    &app_dir,
                    &manifest,
                    &client_dir,
                    &config.build,
                    &config.plugins,
                    RuvyxaBuildCache {
                        dependency_hash: &config.build_dependency_hash,
                        directory: &build_cache_directory,
                    },
                    &plugin_session,
                    &rsc_entries,
                )
            };
            let client_bundle_duration = bundle_started.elapsed();
            let prepared_assets = preparation
                .join()
                .map_err(|_| anyhow::anyhow!("asset preparation worker panicked"))??;
            Ok(((prepared_assets, client_manifest?), client_bundle_duration))
        })?;
    // The two phases below report separately, so the shared live line ends here
    // without a result of its own.
    if let Some(spinner) = spinner {
        spinner.cancel();
    }
    let PreparedBuildAssets {
        styles: style_collection,
        images: image_report,
        asset_files,
        duration: preparation_duration,
    } = prepared_assets;

    // Stage every module the routes reach from outside `app/`.
    //
    // `ruvyxa start` compiles pages out of this copy, so a module it does not
    // contain cannot be resolved at request time. Only `app/` and two
    // hard-coded directory names were staged, and the ordinary `app/` + `lib/`
    // layout therefore answered a request-time render with
    // `RUV1801 cannot resolve '../../lib/x'` — naming a path under `.ruvyxa`
    // that nobody wrote, on a build that reported success. The set comes from
    // the route graph rather than a directory list, so it follows whatever the
    // application actually imports.
    let reachable_modules = ruvyxa_graph::reachable_project_modules(&args.root, &manifest)
        .into_iter()
        .collect::<Vec<_>>();
    copy_project_sources(&args.root, &server_dir, &reachable_modules)?;

    // The optimizer converted these images; a raw `<img>` still references the
    // source extension. Depending on `keepOriginal`, that either 404s on a
    // static host or bypasses the smaller WebP. A server-only build converts
    // nothing and renders no markup, so there is nothing to warn about.
    let bypassed_images = if args.server_only {
        Vec::new()
    } else {
        scan_raw_image_usage(&app_dir, &image_report.entries)
    };
    warn_bypassed_images(&bypassed_images, image_options.keep_original);

    if show_summary {
        let detail = assets_phase_detail(asset_files, image_report.optimized_images);
        print_build_phase(None, "assets prepared", detail, preparation_duration);
    }

    // The client manifest is machine-read (the server resolves per-route scripts
    // and preloads from it) and never hand-edited, so emit compact JSON: it is
    // part of the deployed artifact and is parsed on the render path.
    if !args.server_only {
        fs::write(
            client_dir.join("manifest.json"),
            serde_json::to_string(&client_manifest)?,
        )?;
        attach_client_artifacts(&staging_dir.join("manifest.json"), &client_manifest)?;
        write_render_manifests(&staging_dir, &manifest, &client_manifest)?;
    }

    // A plugin rewrote a module the server is about to render unchanged.
    //
    // Checked here because this is the first moment both halves are known: the
    // browser compile has run and recorded what it rewrote, and the route graph
    // says which routes render on the server and hydrate. Neither half can see
    // the problem alone — each is internally consistent — and the deployed
    // symptom is a flicker, not an error.
    report_plugin_transform_divergence(
        &plugin_session,
        &manifest,
        &build_cache_directory,
        &config.build_dependency_hash,
    );

    // A package the browser graph asked for and could not find.
    //
    // Reported here rather than where it is decided, because the linker is five
    // levels below any signature this holds and the answer is the same for
    // every route: the build is over, and this is what it could not resolve.
    report_unresolved_client_imports();

    // The project's compiled CSS, written as a client asset rather than inlined
    // into each document.
    //
    // A deployed function has no `app/` to compile from and no style collector
    // to run, so a route rendered at request time on a deployed build reached
    // the browser with **no stylesheet at all** — the pre-rendered pages carried
    // theirs inline and everything else was unstyled. Emitting it here puts the
    // stylesheet where every adapter already copies from and where `start`
    // already serves from, so all three hosts reference one file.
    let style_asset = if args.server_only {
        None
    } else {
        crate::client_bundle::write_style_asset(&client_dir, &style_collection.css)?
    };
    let client_bundles = client_manifest
        .get("routes")
        .and_then(|routes| routes.as_array())
        .map(Vec::len)
        .unwrap_or_default();
    if show_summary && !args.server_only {
        print_build_phase(
            None,
            "client bundles",
            format!("{client_bundles} bundles"),
            client_bundle_duration,
        );
    }

    // Composed once and reused by everything that writes a document: the
    // pre-rendered pages, the not-found page, and the deployment manifest the
    // function bundle bakes its own copy from. Three readers of one value, so a
    // head contribution cannot reach some documents and not others.
    let document_head = prerender_head(
        &config,
        &assets_dir,
        style_asset.as_deref(),
        &style_collection.css,
        CsrShell::from_site(&config.site),
    );

    // ─── SSG / ISR / PPR pre-rendering at build time ──────────────────────────
    // Every prerendered artifact is an HTML page, and a server-only build has
    // no page routes by the compatibility rule above, so this stage has no work
    // to do and its `prerender/` directory is never created.
    let prerender_dir = staging_dir.join("prerender");
    let static_params_pool = awaited_static_params_pool(static_params_pool_task).await?;
    let phase_started = Instant::now();
    let prerendered = if args.server_only {
        Vec::new()
    } else {
        prerender_static_routes(
            &args.root,
            &app_dir,
            &manifest,
            &prerender_dir,
            &client_dir,
            document_head.clone(),
            &config.build,
            RuvyxaBuildCache {
                dependency_hash: &config.build_dependency_hash,
                directory: &build_cache_directory,
            },
            config.javascript_runtime(),
            show_summary,
            static_params_pool,
        )
        .await?
    };
    let prerender_duration = phase_started.elapsed();
    if show_summary && !prerendered.is_empty() {
        print_build_phase(
            None,
            "prerendered",
            format!(
                "{} page{}",
                prerendered.len(),
                if prerendered.len() == 1 { "" } else { "s" }
            ),
            prerender_duration,
        );
    }

    // The application's own answer for a URL no route owns.
    //
    // Written beside the pre-rendered pages as `404.html`, which is the name a
    // static host already serves without configuration and the file a function
    // build carries so `createHandler` can return the same document. Before
    // this, a deployed application answered an unmatched URL with the bare
    // string `Not Found` while the same code under `ruvyxa dev` rendered the
    // project's page.
    let not_found_document = crate::prerender::prerender_not_found_document(
        &args.root,
        &app_dir,
        &prerender_dir,
        &PrerenderHead {
            // Unused on this path: the not-found document is rendered, so it
            // composes its own head.
            shell: CsrShell::default(),
            ..document_head.clone()
        },
        NotFoundPrerender {
            build: &config.build,
            cache: RuvyxaBuildCache {
                dependency_hash: &config.build_dependency_hash,
                directory: &build_cache_directory,
            },
            runtime: config.javascript_runtime(),
            i18n: manifest.i18n.as_ref(),
            documents: !args.server_only,
        },
    )
    .await?;

    // What was actually written, read back. Every other check in this build
    // asks about the inputs; this one asks whether the output can load. A
    // client chunk carrying a specifier the linker never rewrote, or a document
    // referencing a stylesheet no directory holds, both look like a working
    // build and fail in the browser — one silently stops hydration, the other
    // renders the site unstyled.
    if !args.server_only {
        let dangling =
            crate::output_audit::audit_emitted_output(&client_dir, &prerender_dir, &assets_dir);
        if let Some(diagnostic) = crate::output_audit::dangling_reference_diagnostic(&dangling) {
            fail_on_diagnostics(&[diagnostic])?;
        }
    }

    // Discovery files are written after prerendering: a dynamic route has no
    // URL until the build produces one, and `public/` has already been staged,
    // so a file the project ships still wins.
    let prerendered_paths: Vec<String> =
        prerendered.iter().map(|route| route.path.clone()).collect();
    let site_url = resolve_site_url(config.site.url.as_deref(), |name| std::env::var(name).ok())
        .map_err(anyhow::Error::msg)?;
    // robots.txt and sitemap.xml describe crawlable pages. A server-only
    // artifact has none, so writing them would advertise URLs that do not exist
    // and would create an `assets/` directory for a project with no `public/`.
    if !args.server_only {
        write_discovery_stage(
            &config,
            &manifest,
            &prerendered_paths,
            &assets_dir,
            site_url.as_deref(),
        )?;
    }

    // Zero-config deploys: pick the adapter from the hosting platform's build
    // environment when neither ruvyxa.config nor --adapter selected one.
    let detected_adapter = if args.adapter.is_none() && config.adapter.is_none() {
        detect_platform_adapter(|key| std::env::var(key).ok())
    } else {
        None
    };
    if let Some((name, source)) = &detected_adapter {
        info!(adapter = %name, source = %source, "auto-detected deploy adapter");
    }

    let mut build_info = BuildReport {
        target,
        args: &args,
        config: &config,
        manifest: &manifest,
        client_manifest: &client_manifest,
        image_report: &image_report,
        prerendered: &prerendered,
        detected_adapter: detected_adapter.as_ref(),
        timing: BuildPhaseTiming {
            route_discovery: route_discovery_duration,
            validation: validation_duration,
            preparation: preparation_duration,
            client_bundle: client_bundle_duration,
            prerender: prerender_duration,
        },
    }
    .document();
    fs::write(
        staging_dir.join("build.json"),
        serde_json::to_string_pretty(&build_info)?,
    )?;

    // The deployment description, added to the route manifest for whoever has
    // to serve this build.
    //
    // `build.json` reports what the build did; this says how the result must be
    // served — which URLs a CDN may answer from a file, which must reach the
    // function, and what cache-control each class of emitted file carries.
    // Adapters used to re-derive all of that from route metadata, one copy per
    // adapter, and the copies disagreed.
    attach_deploy_description(
        &staging_dir.join("manifest.json"),
        &DeployManifestInput {
            manifest: &manifest,
            prerendered: &prerendered,
            prerender_dir: &prerender_dir,
            not_found_document: not_found_document.as_deref(),
            client_assets: emitted_client_assets(&client_dir),
            base_path: String::new(),
            document_head: crate::deploy_manifest::DocumentHeadDefaults {
                asset_links: &document_head.asset_links,
                plugin_head: &document_head.plugin_head,
            },
            adapter: args
                .adapter
                .as_deref()
                .or_else(|| config.adapter.as_ref().and_then(serde_json::Value::as_str))
                .or_else(|| detected_adapter.as_ref().map(|(name, _)| name.as_str())),
        },
    )?;

    // The commit path retries renames with blocking backoff sleeps on
    // Windows; run it off the async runtime so a locked file can't stall
    // other tasks sharing the worker thread.
    let commit_staging = staging_dir.clone();
    let commit_out = out_dir.clone();
    tokio::task::spawn_blocking(move || commit_staged_build_outputs(&commit_staging, &commit_out))
        .await
        .context("build output commit task panicked")?
        .with_context(|| format!("failed to commit build output into {}", out_dir.display()))?;
    plugin_session.run_complete(&out_dir, &build_info)?;
    // Adapters must snapshot the committed output after build-complete hooks:
    // first-party and application plugins can add public artifacts such as a
    // sitemap or service worker that must be present in static deploy output.
    // Only a route the platform is asked to hold can go stale behind a
    // `revalidatePath()` it cannot deliver.
    let revalidating_routes = manifest.routes.iter().any(|route| {
        matches!(
            route.render.strategy,
            ruvyxa_graph::RenderStrategy::Isr | ruvyxa_graph::RenderStrategy::Ppr
        )
    });
    if let Some(artifacts) = run_adapter_stage(
        &args,
        &config,
        &out_dir,
        &detected_adapter,
        show_summary,
        revalidating_routes,
    )? {
        build_info["adapterArtifacts"] = artifacts;
    }
    build_info["timing"]["totalMs"] = serde_json::json!(duration_ms(started.elapsed()));
    fs::write(
        out_dir.join("build.json"),
        serde_json::to_string_pretty(&build_info)?,
    )?;

    info!(
        target = ?target,
        routes = manifest.routes.len(),
        output = %out_dir.display(),
        "build complete"
    );
    if show_summary {
        println!();
        print_route_size_table(&manifest, &client_manifest);
        print_success_banner_at("Built into", Some(&out_dir), started.elapsed());
    }
    Ok(())
}

/// What route discovery and validation produced, with the two phase timings the
/// build report quotes.
struct ValidatedRoutes {
    manifest: RouteManifest,
    discovery_duration: Duration,
    validation_duration: Duration,
}

/// The `<head>` every page this build bakes carries beyond its own metadata.
///
/// One composer for both documents a build writes -- the pre-rendered pages and
/// the not-found page -- because the two were assembled separately and a head
/// contribution added to one was simply absent from the other. What goes in is
/// fixed by what the live pipeline injects: a pre-rendered page is served from
/// disk with no renderer left to add anything, so whatever `dev` and `start`
/// compose has to be baked in here or the two documents differ in production
/// only. `prerender_html_includes_the_document_head_the_live_renderer_composes`
/// holds that correspondence.
pub(crate) fn prerender_head(
    config: &ProjectConfig,
    assets_dir: &Path,
    style_asset: Option<&str>,
    css: &str,
    shell: CsrShell,
) -> PrerenderHead {
    PrerenderHead {
        // Read from the staged assets directory, which is what a deployed
        // server publishes — not from the project's `public/`, which still
        // holds an original the build converted away.
        asset_links: ruvyxa_dev_server::public_asset_links(assets_dir).into(),
        plugin_head: ruvyxa_dev_server::render_plugin_head(
            &crate::client_bundle::collect_plugin_head(&config.plugins),
        )
        .into(),
        styles: ruvyxa_dev_server::style_head_tag(style_asset, css).into(),
        shell,
    }
}

/// Discover the project's routes and refuse the build if they do not validate.
///
/// Everything here happens before the plugin session starts and before any
/// staging directory exists, which is the property that matters: a build
/// rejected on a route error leaves the previous output and the project
/// untouched.
fn discover_and_validate_routes(
    args: &BuildArgs,
    config: &ProjectConfig,
    target: BuildTarget,
    show_summary: bool,
) -> anyhow::Result<ValidatedRoutes> {
    let phase_started = Instant::now();
    let spinner = start_build_phase(show_summary, "routes discovered");
    let manifest = discover_project_routes(&args.root, config)?;
    let discovery_duration = phase_started.elapsed();
    if show_summary {
        print_build_phase(
            spinner,
            "routes discovered",
            format!("{} routes", manifest.routes.len()),
            discovery_duration,
        );
    }
    // Written before validation so a build that fails on a route error still
    // leaves the editor's route types describing what is on disk.
    if config.typed_routes() {
        write_route_types(&args.root, &manifest)?;
    }
    let phase_started = Instant::now();
    let spinner = start_build_phase(show_summary, "validated");
    let validation = validate_app(&args.root, &manifest)?;
    fail_on_diagnostics(&validation.diagnostics)?;
    let validation_duration = phase_started.elapsed();
    if show_summary {
        print_build_phase(spinner, "validated", "ok".to_string(), validation_duration);
        report_inert_hydration(&validation.inert_hydration_routes);
        report_server_only_error_pages(&args.root, &manifest.app_dir.clone(), &manifest);
    }
    if args.server_only {
        ensure_server_only_supported(target, &manifest)?;
    }
    Ok(ValidatedRoutes {
        manifest,
        discovery_duration,
        validation_duration,
    })
}

/// The directories one build writes into.
///
/// Derived together because every phase below takes some subset of them, and
/// they are only correct relative to each other: `root` is the staging tree the
/// build commits atomically, while `image_cache_dir` deliberately sits outside
/// it so a discarded staging tree does not throw away converted images.
struct BuildStagingLayout {
    root: PathBuf,
    server_dir: PathBuf,
    client_dir: PathBuf,
    assets_dir: PathBuf,
    image_cache_dir: PathBuf,
}

impl BuildStagingLayout {
    fn create(out_dir: &Path, build_cache_directory: &Path) -> anyhow::Result<Self> {
        let root = create_build_staging_dir(out_dir).with_context(|| {
            format!(
                "failed to create build staging dir in {}",
                out_dir.display()
            )
        })?;
        Ok(Self {
            server_dir: root.join("server"),
            client_dir: root.join("client"),
            assets_dir: root.join("assets"),
            image_cache_dir: build_cache_directory.join("images"),
            root,
        })
    }
}

/// Copy the browser artifact identity into the deployment route manifest.
///
/// Adapters read this manifest when building their function registry. Keeping
/// the identity beside the route makes every server-side protocol compare the
/// exact client artifact the browser can navigate to, rather than a mutable
/// build counter or a path-derived guess.
fn attach_client_artifacts(
    manifest_path: &Path,
    client_manifest: &serde_json::Value,
) -> anyhow::Result<()> {
    let artifacts = client_manifest
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|route| Some((route.get("path")?.as_str()?, route)))
        .collect::<BTreeMap<_, _>>();
    if artifacts.is_empty() {
        return Ok(());
    }

    let mut manifest: serde_json::Value = serde_json::from_slice(&fs::read(manifest_path)?)?;
    let Some(routes) = manifest
        .get_mut("routes")
        .and_then(serde_json::Value::as_array_mut)
    else {
        anyhow::bail!("route manifest has no routes array")
    };
    for route in routes {
        let Some(path) = route.get("path").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if let Some(client) = artifacts.get(path) {
            if let Some(artifact_version) = client
                .get("artifactVersion")
                .and_then(serde_json::Value::as_str)
            {
                route["artifactVersion"] = serde_json::Value::String(artifact_version.to_string());
            }
            route["flight"] = serde_json::Value::Bool(
                client
                    .get("flight")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            );
            route["cache"] = serde_json::Value::Bool(
                client
                    .get("cache")
                    .and_then(serde_json::Value::as_bool)
                    .unwrap_or(false),
            );
        }
    }
    fs::write(manifest_path, serde_json::to_vec(&manifest)?)?;
    Ok(())
}

/// Emit the concise server-side contracts consumed by tooling and adapters.
///
/// File names stay short and domain-specific; schema versions live inside the
/// payload so a future schema does not force every deployment path to rename.
fn write_render_manifests(
    out_dir: &Path,
    routes: &RouteManifest,
    client: &serde_json::Value,
) -> anyhow::Result<()> {
    let client_routes = client
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .cloned()
        .unwrap_or_default();

    let mut modules = BTreeMap::<String, serde_json::Value>::new();
    for route in &client_routes {
        for module in route
            .pointer("/chunkManifest/referenceManifest/modules")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(id) = module.get("id").and_then(serde_json::Value::as_str) {
                modules
                    .entry(id.to_string())
                    .or_insert_with(|| module.clone());
            }
        }
    }
    let module_values = modules.into_values().collect::<Vec<_>>();
    let reference_version = &blake3::hash(&serde_json::to_vec(&module_values)?).to_hex()[..16];
    write_contract(
        &out_dir.join("references.json"),
        serde_json::json!({
            "contract": "ruvyxa.references",
            "schemaVersion": 1,
            "artifactVersion": reference_version,
            "modules": module_values,
        }),
    )?;

    let mut actions = Vec::new();
    for route in routes
        .routes
        .iter()
        .filter(|route| route.kind != ruvyxa_graph::RouteKind::Api)
    {
        let Some(parent) = route.file.parent() else {
            continue;
        };
        let Some(file) = [parent.join("action.ts"), parent.join("action.js")]
            .into_iter()
            .find(|file| file.is_file())
        else {
            continue;
        };
        let source = fs::read_to_string(&file)?;
        actions.push(serde_json::json!({
            "route": route.path,
            "routeId": route.id,
            "referenceId": ruvyxa_dev_server::action_reference_id(&route.id, &source),
        }));
    }
    write_contract(
        &out_dir.join("actions.json"),
        serde_json::json!({
            "contract": "ruvyxa.actions",
            "schemaVersion": 1,
            "routes": actions,
        }),
    )?;

    let flights = client_routes
        .iter()
        .filter(|route| route.get("flight").and_then(serde_json::Value::as_bool) == Some(true))
        .map(|route| {
            serde_json::json!({
                "route": route.get("path").and_then(serde_json::Value::as_str).unwrap_or("/"),
                "artifactVersion": route.get("artifactVersion").and_then(serde_json::Value::as_str),
                "cache": route.get("cache").and_then(serde_json::Value::as_bool).unwrap_or(false),
            })
        })
        .collect::<Vec<_>>();
    write_contract(
        &out_dir.join("flight.json"),
        serde_json::json!({
            "contract": "ruvyxa.flight",
            "schemaVersion": 1,
            "routes": flights,
        }),
    )?;
    Ok(())
}

fn write_contract(path: &Path, value: serde_json::Value) -> anyhow::Result<()> {
    fs::write(path, serde_json::to_vec(&value)?)?;
    Ok(())
}

/// Starts the live line for a phase that is about to block, when the build is
/// reporting to a human. `None` — a quiet build, or a phase that draws its own
/// progress track — leaves the terminal untouched until the phase line prints.
pub(crate) fn start_build_phase(show_summary: bool, name: &str) -> Option<Spinner> {
    show_summary.then(|| Spinner::start(name))
}

/// Replaces the live line with the phase result. Passing the spinner in rather
/// than dropping it separately is what keeps the animation and the line it
/// resolves to from disagreeing about the phase name.
pub(crate) fn print_build_phase(
    spinner: Option<Spinner>,
    name: &str,
    detail: String,
    duration: Duration,
) {
    match spinner {
        Some(spinner) => spinner.finish_with(detail, duration),
        None => print_phase(name, detail, duration),
    }
}

/// Per-route bundle size table shown after a successful build.
pub(crate) fn print_route_size_table(
    manifest: &RouteManifest,
    client_manifest: &serde_json::Value,
) {
    let page_routes = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .collect::<Vec<_>>();
    if page_routes.is_empty() {
        return;
    }
    let client_routes = client_manifest
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let shared_chunks = client_manifest
        .get("sharedRouteChunks")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();

    // Measured in characters, not bytes: a route directory named in Thai or any
    // other non-ASCII script is several bytes per column, and a byte count pads
    // those rows two or three times too little.
    let route_width = page_routes
        .iter()
        .map(|route| display_width(&route.path))
        .max()
        .unwrap_or(0)
        .max(display_width("shared by all"))
        .max(24);
    println!(
        "      {}{} {}{} {}",
        label("route"),
        spaces(route_width, display_width("route")),
        label("size"),
        spaces(9, display_width("size")),
        label("first load")
    );
    for (index, route) in page_routes.iter().enumerate() {
        let client_route = client_routes.iter().find(|entry| {
            entry.get("path").and_then(serde_json::Value::as_str) == Some(route.path.as_str())
        });
        let route_bytes = client_route.map(manifest_entry_bytes).unwrap_or_default();
        let first_load = client_route.map(first_load_bytes).unwrap_or_default();
        let branch = if index + 1 == page_routes.len() && shared_chunks.is_empty() {
            "└"
        } else {
            "├"
        };
        let size = format_bytes(route_bytes);
        println!(
            "  {} {} {}{} {}{} {}",
            dim(branch),
            styled_render_symbol(route.render.strategy),
            route.path,
            spaces(route_width, display_width(&route.path)),
            dim(&size),
            spaces(9, display_width(&size)),
            styled_first_load(first_load)
        );
    }
    let shared_bytes = shared_chunks
        .iter()
        .map(manifest_entry_bytes)
        .sum::<usize>();
    if shared_bytes > 0 {
        println!(
            "  {}   shared by all{}{}",
            dim("└"),
            spaces(route_width + 11, display_width("shared by all")),
            styled_first_load(shared_bytes)
        );
    }
    println!("  {}", dim("○ csr · ● static · ◐ isr/ppr · ƒ dynamic"));
    println!();
}

/// Colour a first-load size by how close it is to the shipping budget.
///
/// The table used to paint every size the same accent colour, which made the
/// column decoration rather than information: a 40 kB route and a 400 kB one
/// looked identical. Green/yellow/red is the whole point of having a budget.
pub(crate) fn styled_first_load(bytes: usize) -> String {
    let text = format_bytes(bytes);
    let budget = crate::client_bundle::DEFAULT_FIRST_LOAD_BUDGET_BYTES;
    if bytes > budget {
        alert_text(text)
    } else if bytes * 5 > budget * 4 {
        // Within 20% of the budget — worth seeing before it crosses.
        warn_text(text)
    } else {
        ok_text(text)
    }
}

pub(crate) fn styled_render_symbol(strategy: RenderStrategy) -> String {
    match strategy {
        RenderStrategy::Csr => dim("○"),
        RenderStrategy::Ssg => ok_text("●"),
        RenderStrategy::Isr | RenderStrategy::Ppr => warn_text("◐"),
        RenderStrategy::Ssr => accent("ƒ"),
    }
}

/// Warn when a plugin transform makes the two documents disagree.
///
/// Both compilers run `build.onTransform`, so an ordinary hook rewrites the
/// browser bundle and the server render alike and there is nothing to report.
/// A hook that branches on `environment` can still rewrite one lane only — a
/// legitimate thing to do, and the demo plugin does exactly that — but on a
/// route that renders on the server *and* hydrates it means the document is
/// built from one text and hydrated against another. React throws the server
/// markup away and re-renders (#418): the page ends up correct, nothing fails,
/// and the only evidence is a flash of the wrong content.
///
/// Both halves of the condition are checked rather than assumed. The plugin is
/// asked whether it really produces different text for `client` and `server`,
/// and the route graph is asked whether anything that both renders and hydrates
/// reaches the module. A client-only transform behind a `'use client'` route,
/// which is what the demo pairs it with, satisfies neither.
/// Name the packages a browser bundle could not resolve.
///
/// The linker replaces such an import with a binding that throws `RUV1611` when
/// it is touched, which keeps the rest of the page alive and names the package
/// in the browser console. That is the right *runtime* answer and it was the
/// only one: the build emitted the stub and said nothing, so a deployment
/// shipped a document whose first interaction threw, from a build that was
/// green.
///
/// A warning rather than a failure. A package reached only by a code path the
/// browser never runs is a working arrangement, and refusing it would break
/// projects that are correct today — but nothing about that arrangement should
/// be silent.
///
/// **Reported on a build that linked, which is every cold one.** A warm build
/// reuses the emitted bundle without running the linker, so the collector has
/// nothing to drain and this says nothing — the stub is still in the bytes it
/// reused. That covers the case this exists for, because an install on a
/// hosting platform is cold and is exactly where a package that resolves on the
/// developer's machine stops resolving. Making a warm build repeat it means
/// carrying the answer in the artifact cache, which is a larger change than a
/// warning is worth; `ruvyxa clean` or any change to the route's graph brings
/// it back.
fn report_unresolved_client_imports() {
    let unresolved = ruvyxa_bundler::linker::take_unresolved_client_imports();
    for (specifier, importers) in &unresolved {
        let importers = importers
            .iter()
            .map(|path| {
                std::path::Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone())
            })
            .collect::<Vec<_>>()
            .join(", ");
        warn!(package = %specifier, "a browser bundle could not resolve a package");
        println!(
            "  {} {}",
            warn_text("warn"),
            dim(format!(
                "{specifier} could not be resolved from node_modules, so the browser bundle carries \
                 a binding that throws RUV1611 the first time it is used ({importers} imports it). \
                 Add {specifier} to this project's dependencies: a package reached through another \
                 package's node_modules resolves on one machine and not on the next."
            ))
        );
    }
}

fn report_plugin_transform_divergence(
    plugin_session: &TypeScriptPluginBuildSession,
    manifest: &RouteManifest,
    cache_dir: &Path,
    dependency_hash: &str,
) {
    let divergent = plugin_session.lane_divergent_modules(cache_dir, dependency_hash);
    let at_risk = ruvyxa_graph::hydrated_routes_reaching(manifest, &divergent);
    let Some(((route, module), rest)) = at_risk.split_first() else {
        return;
    };
    let module = module
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| module.display().to_string());
    let more = match rest.len() {
        0 => String::new(),
        count => format!(" and {count} more"),
    };
    warn!(
        route = %route,
        module = %module,
        "a plugin transform differs between the browser and the server"
    );
    println!(
        "  {} {}",
        warn_text("warn"),
        dim(format!(
            "a build.onTransform hook rewrites {module} differently for the browser than for the server, and \
             {route}{more} both renders it on the server and hydrates it. The two documents will differ and \
             React will discard the server markup (#418). Drop the `environment` branch so both compiles \
             agree, or move the value behind a `'use client'` route, which has no server document to \
             disagree with."
        ))
    );
}

/// Name the server-components routes whose `error.tsx` only runs on the server.
///
/// On such a route the browser bundle contains client components and nothing
/// else, so a special file without `'use client'` cannot be in it. The server
/// still renders it, which is what makes the gap quiet: the author sees their
/// error page whenever the failure happens during the server render, and the
/// framework's built-in message whenever it happens in the browser — reading
/// the payload, or during a soft navigation, where no server render is involved
/// at all. Two different error pages for one route, decided by where the error
/// occurred.
///
/// A hint, not a failure. Rendering it on the server alone is a working
/// arrangement, and a project may prefer it.
fn report_server_only_error_pages(root: &Path, app_dir: &Path, manifest: &RouteManifest) {
    let mut routes = Vec::new();
    for route in &manifest.routes {
        if !route.render.server_components {
            continue;
        }
        let Some(directory) = route.file.parent() else {
            continue;
        };
        let Some(error_page) = resolve_route_specials(app_dir, directory).error else {
            continue;
        };
        let Ok(source) = fs::read_to_string(&error_page) else {
            continue;
        };
        if ruvyxa_bundler::reference_manifest::has_module_directive(&source, "use client") {
            continue;
        }
        routes.push((route.path.clone(), error_page));
    }
    let Some(((path, error_page), rest)) = routes.split_first() else {
        return;
    };
    let name = error_page
        .strip_prefix(root)
        .unwrap_or(error_page)
        .display()
        .to_string()
        .replace('\\', "/");
    let more = match rest.len() {
        0 => String::new(),
        count => format!(" and {count} more"),
    };
    println!(
        "  {} {}",
        warn_text("hint"),
        dim(format!(
            "{name} has no `'use client'`, so {path}{more} renders it on the server and the \
             framework's built-in message in the browser. Add the directive to use one error \
             page for both."
        ))
    );
}

/// Name the routes that ship a browser bundle with nothing in it to hydrate.
///
/// A server-components route whose graph never crosses a `'use client'`
/// boundary has no browser code of its own, yet it still downloads the shared
/// React runtime — a few hundred kilobytes on a page that uses none of it. That
/// cost appears in no other report: the bundle is real, referenced, and
/// correct, so nothing flags it and nobody looks.
///
/// A hint rather than an automatic opt-out. `export const hydrate = false`
/// removes the route from the client router's registry too, so navigating to it
/// stops being a soft navigation and becomes a full page load. Which cost
/// matters is the project's decision, and it cannot be read from the source.
fn report_inert_hydration(routes: &[String]) {
    let Some((first, rest)) = routes.split_first() else {
        return;
    };
    let (more, verb) = match rest.len() {
        0 => (String::new(), "ships"),
        count => (format!(" and {count} more"), "ship"),
    };
    println!(
        "  {} {} {}",
        warn_text("hint"),
        dim(format!(
            "{first}{more} {verb} a client bundle with nothing to hydrate."
        )),
        dim(
            "Add `export const hydrate = false` to serve them without JavaScript (navigation to them becomes a full page load)."
        )
    );
}

/// Add the deployment description to the route manifest.
///
/// Written into `manifest.json` under one key rather than beside it as a second
/// file. Two top-level manifests that both describe routes gave a reader no way
/// to tell which one was authoritative, and the route manifest is already the
/// file everything downstream opens.
fn attach_deploy_description(
    manifest_path: &Path,
    input: &DeployManifestInput<'_>,
) -> anyhow::Result<()> {
    let mut manifest: serde_json::Value = serde_json::from_slice(&fs::read(manifest_path)?)?;
    let Some(object) = manifest.as_object_mut() else {
        anyhow::bail!("route manifest is not an object")
    };
    object.insert(
        DEPLOY_MANIFEST_KEY.to_string(),
        crate::deploy_manifest::deploy_manifest(input),
    );
    fs::write(manifest_path, serde_json::to_string_pretty(&manifest)?)?;
    Ok(())
}

pub(crate) fn styled_strategy_word(strategy: RenderStrategy) -> String {
    match strategy {
        RenderStrategy::Csr => dim("csr"),
        RenderStrategy::Ssg => ok_text("ssg"),
        RenderStrategy::Isr => warn_text("isr"),
        RenderStrategy::Ppr => warn_text("ppr"),
        RenderStrategy::Ssr => accent("ssr"),
    }
}

pub(crate) fn manifest_entry_bytes(entry: &serde_json::Value) -> usize {
    entry
        .get("bytes")
        .and_then(serde_json::Value::as_u64)
        .and_then(|bytes| usize::try_from(bytes).ok())
        .unwrap_or_default()
}

pub(crate) fn first_load_bytes(entry: &serde_json::Value) -> usize {
    let mut files = BTreeSet::new();
    let mut total = 0;
    add_manifest_entry_bytes(entry, &mut files, &mut total);
    for section in ["chunks", "sharedChunks"] {
        for chunk in entry
            .get(section)
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            add_manifest_entry_bytes(chunk, &mut files, &mut total);
        }
    }
    total
}

pub(crate) fn add_manifest_entry_bytes(
    entry: &serde_json::Value,
    files: &mut BTreeSet<String>,
    total: &mut usize,
) {
    let should_count = entry
        .get("file")
        .and_then(serde_json::Value::as_str)
        .map(|file| files.insert(file.to_string()))
        .unwrap_or(true);
    if should_count {
        *total += manifest_entry_bytes(entry);
    }
}

// `deploy` and `static` hold adapter artifacts (see artifactDestination in
// adapter-runner.mjs); omitting them here would silently drop adapter output
// when the staged build is committed.
pub(crate) const BUILD_OUTPUT_DIRS: [&str; 6] = [
    "server",
    "client",
    "assets",
    "prerender",
    "deploy",
    "static",
];
pub(crate) const BUILD_OUTPUT_FILES: [&str; 2] = ["manifest.json", "build.json"];
// Default cap balances Node process memory against prerender throughput; an
// explicit `build.parallelism` config value may raise it up to the pool limit.

#[cfg(test)]
mod build_table_tests {
    use super::*;

    /// Colour in the build table has to mean something. `paint` strips escapes
    /// when stdout is not a terminal — as in tests — so assert on the mapping
    /// through the styling helpers rather than on escape codes.
    #[test]
    fn first_load_colour_tracks_the_shipping_budget() {
        let budget = crate::client_bundle::DEFAULT_FIRST_LOAD_BUDGET_BYTES;

        assert_eq!(
            styled_first_load(budget / 4),
            ok_text(format_bytes(budget / 4))
        );
        // 90% of budget: close enough to warn before it crosses.
        let near = budget * 9 / 10;
        assert_eq!(styled_first_load(near), warn_text(format_bytes(near)));
        let over = budget + 1;
        assert_eq!(styled_first_load(over), alert_text(format_bytes(over)));
    }
}
