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

pub(crate) fn prepare_build_assets(
    root: &Path,
    app_dir: &Path,
    server_dir: &Path,
    assets_dir: &Path,
    // `None` skips style collection entirely: a server-only artifact renders no
    // HTML document, so collected CSS has nothing to be inlined into and no
    // stylesheet to be emitted as. The project's own source files are still
    // staged — an API route may import a module that sits next to a stylesheet.
    style_entries: Option<&[PathBuf]>,
    image_cache_dir: &Path,
    image_options: &ImageOptimizationOptions,
) -> anyhow::Result<PreparedBuildAssets> {
    let started = Instant::now();
    let (styles, images) = std::thread::scope(|scope| -> anyhow::Result<_> {
        let styles = scope.spawn(|| match style_entries {
            Some(entries) => ruvyxa_dev_server::collect_styles_for_build(root, app_dir, entries),
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
    copy_style_sources(root, server_dir, &styles.files)?;
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
    print_tui_header("Build");
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
fn run_adapter_stage(
    args: &BuildArgs,
    config: &ProjectConfig,
    out_dir: &Path,
    detected_adapter: &Option<(String, String)>,
    show_summary: bool,
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
    Ok(Some(serde_json::to_value(artifacts)?))
}

pub(crate) async fn build_with_output(args: BuildArgs, show_summary: bool) -> anyhow::Result<()> {
    build_with_cache_override(args, show_summary, None).await
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
    let target = config.build_target(args.target);
    let app_dir = args.root.join(config.app_dir());
    let out_dir = args.root.join(config.out_dir());
    let build_cache_directory = cache_directory
        .map(Path::to_path_buf)
        .unwrap_or_else(|| build_cache_dir(&args.root, &config.cache));

    if show_summary {
        print_build_header(&args, target, &app_dir, &out_dir);
    }

    let phase_started = Instant::now();
    let spinner = start_build_phase(show_summary, "routes discovered");
    let manifest = discover_project_routes(&args.root, &config)?;
    let route_discovery_duration = phase_started.elapsed();
    if show_summary {
        print_build_phase(
            spinner,
            "routes discovered",
            format!("{} routes", manifest.routes.len()),
            route_discovery_duration,
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
    }
    // Every server-only compatibility rule is enforced here: before the plugin
    // session starts and before any staging directory exists, so a rejected
    // build leaves the previous output and the project untouched.
    if args.server_only {
        ensure_server_only_supported(target, &manifest)?;
    }
    let plugin_session = TypeScriptPluginBuildSession::new(
        &args.root,
        &config.plugins,
        config.javascript_runtime(),
        config.markdown_enabled(),
        config.react_compiler.unwrap_or(false),
    )?;
    plugin_session.run_start(&out_dir)?;
    let staging_dir = create_build_staging_dir(&out_dir).with_context(|| {
        format!(
            "failed to create build staging dir in {}",
            out_dir.display()
        )
    })?;
    let _staging_cleanup = BuildStagingCleanup::new(staging_dir.clone());
    let server_dir = staging_dir.join("server");
    let client_dir = staging_dir.join("client");
    let assets_dir = staging_dir.join("assets");
    let image_cache_dir = build_cache_directory.join("images");
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
    let spinner = start_build_phase(show_summary, "bundling");
    let ((prepared_assets, client_manifest), client_bundle_duration) =
        std::thread::scope(|scope| -> anyhow::Result<_> {
            let preparation = scope.spawn(|| {
                prepare_build_assets(
                    &args.root,
                    &app_dir,
                    &server_dir,
                    &assets_dir,
                    style_entries.as_deref(),
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
                        dependency_hash: &config.config_dependency_hash,
                        directory: &build_cache_directory,
                    },
                    &plugin_session,
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

    // ─── SSG / ISR / PPR pre-rendering at build time ──────────────────────────
    // Every prerendered artifact is an HTML page, and a server-only build has
    // no page routes by the compatibility rule above, so this stage has no work
    // to do and its `prerender/` directory is never created.
    let prerender_dir = staging_dir.join("prerender");
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
            &style_collection.css,
            &config.build,
            RuvyxaBuildCache {
                dependency_hash: &config.config_dependency_hash,
                directory: &build_cache_directory,
            },
            config.javascript_runtime(),
            show_summary,
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

    let mut build_info = serde_json::json!({
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
                .as_ref()
                .map(|(name, _)| serde_json::json!(name))),
        "adapterOptions": config.adapter_options.clone(),
        "images": image_report,
        "runtime": {
            "middleware": config.middleware,
            "i18n": manifest.i18n,
            "image": {
                "onDemand": config.images.on_demand.enabled(),
                "maxWidth": config.images.on_demand.max_width(),
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
            "routeDiscoveryMs": duration_ms(route_discovery_duration),
            "validationMs": duration_ms(validation_duration),
            "preparationMs": duration_ms(preparation_duration),
            "clientBundleMs": duration_ms(client_bundle_duration),
            "prerenderMs": duration_ms(prerender_duration)
        }
    });
    fs::write(
        staging_dir.join("build.json"),
        serde_json::to_string_pretty(&build_info)?,
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
    if let Some(artifacts) =
        run_adapter_stage(&args, &config, &out_dir, &detected_adapter, show_summary)?
    {
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
