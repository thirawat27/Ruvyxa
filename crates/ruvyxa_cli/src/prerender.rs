//! Static prerendering: turning routes into HTML files at build time.
//!
//! A prerender job is one route path with one set of resolved parameters. Jobs
//! are produced from the route manifest (expanding `getStaticParams` where a
//! route has dynamic segments), rendered through a pool of JavaScript workers,
//! and written under the prerender output directory.
//!
//! Two properties matter more than speed here and constrain the code:
//!
//! - **Path safety.** A parameter value becomes a filesystem path, so every
//!   segment is validated before it is joined ([`is_unsafe_prerender_segment`]).
//!   A route that would escape the output directory fails the build.
//! - **Cache correctness.** A cached artifact is keyed by everything that can
//!   change its HTML — route inputs, client assets, and the stable subset of
//!   `process.env` — so a hit cannot serve HTML the current inputs would not
//!   produce.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ruvyxa_dev_server::JavaScriptRuntime;
use ruvyxa_graph::{HydrationMode, RenderStrategy, RouteEntry, RouteManifest, RouteParams};
use tracing::info;

use crate::*;

pub(crate) const MAX_PRERENDER_PARALLELISM: usize = 4;
pub(crate) const MAX_CONFIGURED_PRERENDER_PARALLELISM: usize = 8;
pub(crate) const WINDOWS_RENAME_RETRY_COUNT: usize = 5;

/// A route that was pre-rendered at build time.
#[derive(Debug)]
pub(crate) struct PrerenderedRoute {
    pub(crate) path: String,
    pub(crate) strategy: RenderStrategy,
    pub(crate) revalidate: Option<u64>,
    pub(crate) html_file: PathBuf,
    pub(crate) artifact_cache_hit: bool,
}

#[derive(Clone)]
pub(crate) struct PrerenderArtifactCache {
    pub(crate) directory: PathBuf,
    pub(crate) dependency_hash: String,
    pub(crate) render_context_hash: String,
    pub(crate) fingerprints: Arc<ArtifactFingerprintCache>,
    pub(crate) enabled: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum PrerenderJobKind {
    Csr,
    Render {
        route_file: PathBuf,
        mode: &'static str,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct PrerenderJob {
    pub(crate) route_path: String,
    pub(crate) render_path: String,
    pub(crate) params: RouteParams,
    pub(crate) strategy: RenderStrategy,
    pub(crate) revalidate: Option<u64>,
    pub(crate) kind: PrerenderJobKind,
}

/// Pre-render all SSG, ISR, and PPR routes at build time.
///
/// For each qualifying route:
/// - SSG static routes: rendered once, saved as `.html`
/// - SSG dynamic routes (with `getStaticParams`): calls the export to discover params, renders each
/// - ISR routes: same as SSG but metadata records the revalidation interval
/// - PPR routes: renders the static shell (Suspense fallbacks, not dynamic content)
/// - CSR routes: emits a minimal shell HTML (no server rendering)
///
/// Returns a list of all pre-rendered routes with their metadata.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn prerender_static_routes(
    root: &Path,
    app_dir: &Path,
    manifest: &RouteManifest,
    prerender_dir: &Path,
    client_dir: &Path,
    styles: &str,
    build: &BuildConfigOptions,
    cache: RuvyxaBuildCache<'_>,
    runtime: JavaScriptRuntime,
    show_progress: bool,
) -> anyhow::Result<Vec<PrerenderedRoute>> {
    use ruvyxa_graph::RouteKind;

    let routes_to_prerender: Vec<&RouteEntry> = manifest
        .routes
        .iter()
        .filter(|route| {
            route.kind == RouteKind::Page
                && matches!(
                    route.render.strategy,
                    RenderStrategy::Ssg
                        | RenderStrategy::Isr
                        | RenderStrategy::Ppr
                        | RenderStrategy::Csr
                )
        })
        .collect();

    if routes_to_prerender.is_empty() {
        return Ok(Vec::new());
    }

    fs::create_dir_all(prerender_dir)?;
    let client_assets = Arc::new(load_prerender_client_assets(client_dir));
    let shared_styles = Arc::<str>::from(styles);
    let i18n = manifest.i18n.clone();

    let parallelism = prerender_parallelism(build.parallelism, routes_to_prerender.len());
    let jsx_runtime = parse_jsx_runtime(build.jsx_runtime.as_deref())?;
    let mut worker_env = ruvyxa_dev_server::project_env(root)?;
    worker_env.insert(
        "RUVYXA_JSX_RUNTIME".to_string(),
        match jsx_runtime {
            ruvyxa_bundler::JsxRuntime::Classic => "classic".to_string(),
            ruvyxa_bundler::JsxRuntime::Automatic => "automatic".to_string(),
        },
    );
    worker_env.insert("RUVYXA_RUNTIME".to_string(), runtime.command().to_string());
    let render_context_hash =
        prerender_context_hash(root, styles, &client_assets, build, &worker_env);
    let artifact_cache = PrerenderArtifactCache {
        directory: cache.directory.to_path_buf(),
        dependency_hash: cache.dependency_hash.to_string(),
        render_context_hash,
        fingerprints: Arc::new(ArtifactFingerprintCache::default()),
        enabled: build.prerender_cache.unwrap_or(true),
    };
    // Do not pay Node process startup on a warm build whose static routes are
    // all served from the validated artifact cache. Dynamic static-parameter
    // discovery still needs a worker before jobs can be enumerated; ordinary
    // static/CSR routes start one only when a render cache miss remains.
    let needs_static_params_worker = routes_to_prerender
        .iter()
        .any(|route| route.render.has_static_params && route_has_dynamic_segments(&route.path));
    let mut worker_pool = if needs_static_params_worker {
        Some(start_prerender_worker_pool(root, &worker_env, parallelism, runtime).await?)
    } else {
        None
    };

    let prerendered = async {
        let mut jobs = Vec::new();
        let static_params_routes = Arc::new(
            manifest
                .routes
                .iter()
                .map(|entry| ruvyxa_dev_server::StaticParamsRoute {
                    path: entry.path.clone(),
                    id: entry.id.clone(),
                })
                .collect::<Vec<_>>(),
        );
        let mut static_param_tasks = (0..routes_to_prerender.len())
            .map(|_| None)
            .collect::<Vec<Option<StaticParamsTask>>>();

        // Static-parameter discovery is independent per dynamic route and the
        // worker pool already bounds execution. Start all requests together,
        // then await them in manifest order below to preserve deterministic
        // errors and job ordering.
        if needs_static_params_worker {
            let permits = Arc::new(tokio::sync::Semaphore::new(parallelism));
            let worker_pool = worker_pool.as_ref().cloned().ok_or_else(|| {
                anyhow::anyhow!("Static-parameter worker pool was not initialized")
            })?;
            for (index, route) in routes_to_prerender.iter().enumerate() {
                if !route.render.has_static_params || !route_has_dynamic_segments(&route.path) {
                    continue;
                }
                let worker_pool = worker_pool.clone();
                let root = root.to_path_buf();
                let route = (*route).clone();
                let routes = static_params_routes.clone();
                let permits = permits.clone();
                static_param_tasks[index] = Some(tokio::spawn(async move {
                    let _permit = permits.acquire_owned().await.map_err(|_| {
                        anyhow::anyhow!("Static-parameter concurrency limiter closed")
                    })?;
                    resolve_static_params(&worker_pool, &root, &route, routes.as_slice()).await
                }));
            }
        }

        for (route_index, route) in routes_to_prerender.into_iter().enumerate() {
            match route.render.strategy {
                RenderStrategy::Csr => {
                    jobs.push(PrerenderJob {
                        route_path: route.path.clone(),
                        render_path: route.path.clone(),
                        params: RouteParams::new(),
                        strategy: RenderStrategy::Csr,
                        revalidate: None,
                        kind: PrerenderJobKind::Csr,
                    });
                }
                RenderStrategy::Ssg | RenderStrategy::Isr | RenderStrategy::Ppr => {
                    // For dynamic routes with getStaticParams, resolve static paths first
                    let paths_to_render = if route.render.has_static_params
                        && route_has_dynamic_segments(&route.path)
                    {
                        let task = static_param_tasks[route_index].take().ok_or_else(|| {
                            anyhow::anyhow!(
                                "Static-parameter task was not initialized for {}",
                                route.path
                            )
                        })?;
                        match task.await {
                            Ok(Ok(paths)) => paths,
                            Ok(Err(error)) => {
                                abort_static_param_tasks(&mut static_param_tasks).await;
                                return Err(error);
                            }
                            Err(error) => {
                                abort_static_param_tasks(&mut static_param_tasks).await;
                                return Err(anyhow::anyhow!(
                                    "getStaticParams worker task panicked for {}: {error}",
                                    route.path
                                ));
                            }
                        }
                    } else if !route_has_dynamic_segments(&route.path) {
                        // Pure static route — render the single path
                        vec![StaticRouteParams {
                            path: route.path.clone(),
                            params: RouteParams::new(),
                        }]
                    } else if let Some(paths) =
                        locale_static_paths(manifest.i18n.as_ref(), &route.path)
                    {
                        // A locale-only dynamic route has a finite path set in
                        // config, so users do not need boilerplate
                        // getStaticParams() just to enumerate locales.
                        paths
                    } else {
                        // Dynamic route without getStaticParams — skip (will be rendered at request time)
                        continue;
                    };

                    let mode = match route.render.strategy {
                        RenderStrategy::Ppr => "ppr",
                        _ => "full",
                    };
                    for static_route in paths_to_render {
                        jobs.push(PrerenderJob {
                            route_path: route.path.clone(),
                            render_path: static_route.path,
                            params: static_route.params,
                            strategy: route.render.strategy,
                            revalidate: route.render.revalidate,
                            kind: PrerenderJobKind::Render {
                                route_file: route.file.clone(),
                                mode,
                            },
                        });
                    }
                }
                _ => {}
            }
        }

        let needs_render_worker = jobs.iter().any(|job| match &job.kind {
            PrerenderJobKind::Csr => false,
            PrerenderJobKind::Render { .. } => {
                !artifact_cache.enabled || load_prerender_artifact(&artifact_cache, job).is_none()
            }
        });
        if needs_render_worker && worker_pool.is_none() {
            worker_pool =
                Some(start_prerender_worker_pool(root, &worker_env, parallelism, runtime).await?);
        }

        let parallelism = prerender_parallelism(build.parallelism, jobs.len());
        let total_jobs = jobs.len();
        let mut completed_jobs = 0usize;
        let mut pending = tokio::task::JoinSet::new();
        let mut jobs = jobs.into_iter().enumerate();
        let mut prerendered = Vec::new();

        draw_progress_bar(show_progress, "prerender", completed_jobs, total_jobs);
        loop {
            while pending.len() < parallelism {
                let Some((index, job)) = jobs.next() else {
                    break;
                };
                let worker_pool = worker_pool.clone();
                let root = root.to_path_buf();
                let app_dir = app_dir.to_path_buf();
                let prerender_dir = prerender_dir.to_path_buf();
                let client_assets = client_assets.clone();
                let styles = shared_styles.clone();
                let artifact_cache = artifact_cache.clone();
                let i18n = i18n.clone();
                pending.spawn(async move {
                    render_prerender_job(
                        worker_pool.as_deref(),
                        &root,
                        &app_dir,
                        &prerender_dir,
                        &client_assets,
                        &styles,
                        &job,
                        &artifact_cache,
                        i18n.as_ref(),
                    )
                    .await
                    .map(|route| (index, route))
                });
            }

            let Some(result) = pending.join_next().await else {
                break;
            };
            prerendered.push(
                result
                    .map_err(|error| anyhow::anyhow!("pre-render worker panicked: {error}"))??,
            );
            completed_jobs += 1;
            draw_progress_bar(show_progress, "prerender", completed_jobs, total_jobs);
        }
        clear_progress_bar(show_progress);

        prerendered.sort_by_key(|(index, _)| *index);
        let prerendered = prerendered
            .into_iter()
            .map(|(_, route)| route)
            .collect::<Vec<_>>();

        // Write pre-render manifest for the production server
        let prerender_manifest = serde_json::json!({
            "routes": prerendered.iter().map(|p| serde_json::json!({
                "path": p.path,
                "strategy": format!("{:?}", p.strategy).to_lowercase(),
                "revalidate": p.revalidate,
                "htmlFile": p.html_file.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
                "cacheHit": p.artifact_cache_hit,
            })).collect::<Vec<_>>()
        });
        fs::write(
            prerender_dir.join("manifest.json"),
            serde_json::to_string_pretty(&prerender_manifest)?,
        )?;

        info!(
            prerendered = prerendered.len(),
            "pre-rendered static routes"
        );

        Ok(prerendered)
    }
    .await;
    if let Some(worker_pool) = worker_pool {
        worker_pool.shutdown().await;
    }
    prerendered
}

pub(crate) async fn start_prerender_worker_pool(
    root: &Path,
    worker_env: &BTreeMap<String, String>,
    parallelism: usize,
    runtime: JavaScriptRuntime,
) -> anyhow::Result<std::sync::Arc<ruvyxa_dev_server::NodeWorkerPool>> {
    Ok(std::sync::Arc::new(
        ruvyxa_dev_server::NodeWorkerPool::start_with_size_and_runtime(
            root,
            worker_env.clone(),
            Some(parallelism),
            runtime,
        )
        .await
        .map_err(|error| anyhow::anyhow!(error.to_string()))?,
    ))
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn render_prerender_job(
    worker_pool: Option<&ruvyxa_dev_server::NodeWorkerPool>,
    root: &Path,
    app_dir: &Path,
    prerender_dir: &Path,
    client_assets: &BTreeMap<String, PrerenderClientAssets>,
    styles: &str,
    job: &PrerenderJob,
    artifact_cache: &PrerenderArtifactCache,
    i18n: Option<&ruvyxa_graph::I18nRouting>,
) -> anyhow::Result<PrerenderedRoute> {
    let Some(html_path) = prerender_html_path(prerender_dir, &job.render_path) else {
        anyhow::bail!(
            "RUV1205 Prerender path `{}` for route `{}` cannot be written inside the build output. \
             Return static params that map to plain URL segments.",
            job.render_path,
            job.route_path
        );
    };
    if let Some(parent) = html_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut artifact_cache_hit = false;
    let html = match &job.kind {
        PrerenderJobKind::Csr => csr_shell_html(&job.route_path, client_assets, styles),
        PrerenderJobKind::Render { route_file, mode } => {
            if artifact_cache.enabled
                && let Some(html) = load_prerender_artifact(artifact_cache, job)
            {
                artifact_cache_hit = true;
                html
            } else {
                let worker_pool = worker_pool.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Pre-rendering worker pool was not initialized for cache miss {}",
                        job.render_path
                    )
                })?;
                let result = worker_pool
                    .render_ssg_isolated(
                        root,
                        app_dir,
                        Path::new(route_file),
                        &job.render_path,
                        &job.route_path,
                        &job.params,
                        mode,
                    )
                    .await
                    .map_err(|error| {
                        anyhow::anyhow!("Pre-rendering failed for {}: {error}", job.render_path)
                    })?;
                if !result.ok {
                    let message = result
                        .message
                        .unwrap_or_else(|| "unknown error".to_string());
                    let code = result.code.unwrap_or_default();
                    anyhow::bail!(
                        "Pre-rendering failed for {}: {code} {message}",
                        job.render_path
                    );
                }
                let dependency_hash = result
                    .dependency_hash
                    .unwrap_or_else(|| "worker-legacy-renderer".to_string());
                let inputs = result.inputs.unwrap_or_default();
                let html = result.html.ok_or_else(|| {
                    anyhow::anyhow!(
                        "Pre-rendering failed for {}: worker completed without HTML",
                        job.render_path
                    )
                })?;
                let html = inject_prerender_styles(&html, styles);
                let html = inject_prerender_client_assets(
                    &html,
                    client_assets,
                    &job.route_path,
                    &job.render_path,
                    &job.params,
                );
                if artifact_cache.enabled {
                    let mut stable_inputs = stable_prerender_inputs(root, app_dir, &inputs);
                    stable_inputs.extend(stable_prerender_inputs(
                        root,
                        app_dir,
                        std::slice::from_ref(route_file),
                    ));
                    store_prerender_artifact(
                        artifact_cache,
                        job,
                        &dependency_hash,
                        &stable_inputs,
                        &html,
                    );
                }
                html
            }
        }
    };

    let html = ruvyxa_dev_server::localize_document(
        &html,
        i18n,
        &job.route_path,
        &job.render_path,
        &job.params,
    );
    fs::write(&html_path, html)?;
    Ok(PrerenderedRoute {
        path: job.render_path.clone(),
        strategy: job.strategy,
        revalidate: job.revalidate,
        html_file: html_path,
        artifact_cache_hit,
    })
}

pub(crate) fn stable_prerender_inputs(
    root: &Path,
    app_dir: &Path,
    inputs: &[PathBuf],
) -> Vec<PathBuf> {
    let project_root = ruvyxa_diagnostics::normalized_canonical_path(root);
    let staging_root = app_dir.parent().and_then(Path::parent);
    inputs
        .iter()
        .map(|input| {
            // The Node worker reports project-relative paths so metadata stays
            // portable across staging directories. Resolve those paths against
            // the project root before touching the process CWD; otherwise a
            // build launched outside the project silently fingerprints nothing
            // and can never reuse prerender artifacts.
            let input = if input.is_absolute() {
                input.clone()
            } else {
                root.join(input)
            };
            let input = ruvyxa_diagnostics::normalized_canonical_path(&input);
            if input.strip_prefix(&project_root).is_ok() {
                return input;
            }
            staging_root
                .and_then(|staging_root| {
                    input.strip_prefix(staging_root).ok().map(|relative| {
                        let relative = relative.strip_prefix("server").unwrap_or(relative);
                        root.join(relative)
                    })
                })
                .unwrap_or(input)
        })
        .collect()
}

/// Resolve static params for a dynamic SSG route by calling getStaticParams
/// via the SSG renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StaticRouteParams {
    pub(crate) path: String,
    pub(crate) params: RouteParams,
}

pub(crate) type StaticParamsTask = tokio::task::JoinHandle<anyhow::Result<Vec<StaticRouteParams>>>;

pub(crate) async fn abort_static_param_tasks(tasks: &mut [Option<StaticParamsTask>]) {
    for task in tasks.iter().flatten() {
        task.abort();
    }
    for task in tasks.iter_mut().filter_map(Option::take) {
        let _ = task.await;
    }
}

pub(crate) async fn resolve_static_params(
    worker_pool: &ruvyxa_dev_server::NodeWorkerPool,
    root: &Path,
    route: &RouteEntry,
    routes: &[ruvyxa_dev_server::StaticParamsRoute],
) -> anyhow::Result<Vec<StaticRouteParams>> {
    let segments = static_param_segments(&route.path);
    let result = worker_pool
        .resolve_static_params(root, &route.file, &route.path, &segments, routes)
        .await
        .map_err(|error| anyhow::anyhow!("getStaticParams failed for {}: {error}", route.path))?;
    if !result.ok {
        anyhow::bail!(
            "getStaticParams failed for {}: {} {}",
            route.path,
            result.code.unwrap_or_default(),
            result
                .message
                .unwrap_or_else(|| "unknown error".to_string())
        );
    }
    let params_list = result.params.unwrap_or_default();

    params_list
        .iter()
        .map(|value| {
            let params = value.clone();
            Ok(StaticRouteParams {
                path: static_route_path(&route.path, &params)?,
                params,
            })
        })
        .collect()
}

pub(crate) fn static_param_segments(
    route_path: &str,
) -> Vec<ruvyxa_dev_server::StaticParamSegment> {
    route_path
        .split('/')
        .filter_map(|segment| {
            if segment.starts_with("[[...") && segment.ends_with("]]") {
                Some(ruvyxa_dev_server::StaticParamSegment {
                    name: segment[5..segment.len() - 2].to_string(),
                    catch_all: true,
                    optional: true,
                })
            } else if segment.starts_with("[...") && segment.ends_with(']') {
                Some(ruvyxa_dev_server::StaticParamSegment {
                    name: segment[4..segment.len() - 1].to_string(),
                    catch_all: true,
                    optional: false,
                })
            } else if segment.starts_with('[') && segment.ends_with(']') {
                Some(ruvyxa_dev_server::StaticParamSegment {
                    name: segment[1..segment.len() - 1].to_string(),
                    catch_all: false,
                    optional: false,
                })
            } else {
                None
            }
        })
        .collect()
}

pub(crate) fn static_route_path(route_path: &str, params: &RouteParams) -> anyhow::Result<String> {
    let mut segments = Vec::new();
    for segment in route_path
        .trim_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
    {
        if segment.starts_with('[')
            && segment.ends_with(']')
            && !segment.starts_with("[...")
            && !segment.starts_with("[[...")
        {
            let name = &segment[1..segment.len() - 1];
            let value = params
                .get(name)
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    anyhow::anyhow!("getStaticParams is missing '{name}' for route {route_path}")
                })?;
            validate_static_path_segment(value, name, route_path)?;
            segments.push(value.to_string());
        } else if segment.starts_with("[...") && segment.ends_with(']') {
            let name = &segment[4..segment.len() - 1];
            let Some(value) = params.get(name) else {
                anyhow::bail!("getStaticParams is missing '{name}' for route {route_path}");
            };
            let values = value.as_array().ok_or_else(|| {
                anyhow::anyhow!(
                    "getStaticParams for {route_path} must return a string array for catch-all '{name}'"
                )
            })?;
            if values.is_empty() {
                anyhow::bail!(
                    "getStaticParams returned an empty catch-all '{name}' for route {route_path}"
                );
            }
            for value_segment in values {
                let value_segment = value_segment.as_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "getStaticParams for {route_path} must return strings in catch-all '{name}'"
                    )
                })?;
                validate_static_path_segment(value_segment, name, route_path)?;
                segments.push(value_segment.to_string());
            }
        } else if segment.starts_with("[[...") && segment.ends_with("]]") {
            let name = &segment[5..segment.len() - 2];
            let Some(value) = params.get(name) else {
                continue;
            };
            let values = value.as_array().ok_or_else(|| {
                anyhow::anyhow!(
                    "getStaticParams for {route_path} must return a string array for optional catch-all '{name}'"
                )
            })?;
            for value_segment in values {
                let value_segment = value_segment.as_str().ok_or_else(|| {
                    anyhow::anyhow!(
                        "getStaticParams for {route_path} must return strings in optional catch-all '{name}'"
                    )
                })?;
                validate_static_path_segment(value_segment, name, route_path)?;
                segments.push(value_segment.to_string());
            }
        } else {
            segments.push(segment.to_string());
        }
    }
    Ok(if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    })
}

pub(crate) fn validate_static_path_segment(
    value: &str,
    name: &str,
    route_path: &str,
) -> anyhow::Result<()> {
    if value.is_empty() || matches!(value, "." | "..") || value.contains(['/', '\\', '?', '#']) {
        anyhow::bail!(
            "getStaticParams returned unsafe value '{value}' for '{name}' in route {route_path}"
        );
    }
    Ok(())
}

pub(crate) fn route_has_dynamic_segments(route_path: &str) -> bool {
    route_path
        .split('/')
        .any(|segment| segment.starts_with('[') && segment.ends_with(']'))
}

pub(crate) fn locale_static_paths(
    i18n: Option<&ruvyxa_graph::I18nRouting>,
    route_path: &str,
) -> Option<Vec<StaticRouteParams>> {
    let i18n = i18n?;
    let marker = format!("[{}]", i18n.locale_param);
    let dynamic = route_path
        .split('/')
        .filter(|segment| segment.starts_with('[') && segment.ends_with(']'))
        .collect::<Vec<_>>();
    if dynamic.as_slice() != [marker.as_str()] {
        return None;
    }
    Some(
        i18n.locales
            .iter()
            .map(|locale| StaticRouteParams {
                path: route_path.replace(&marker, locale),
                params: RouteParams::from([(
                    i18n.locale_param.clone(),
                    serde_json::Value::String(locale.clone()),
                )]),
            })
            .collect(),
    )
}

/// Generate the output HTML file path for a pre-rendered route.
/// Map a render path to the file that stores its pre-rendered HTML.
///
/// Returns `None` when the path cannot be mapped inside `prerender_dir`.
/// Render paths for dynamic routes come from the app's own `getStaticParams()`,
/// so a parameter value such as `..` would otherwise write outside the build
/// output. Mirrors `prerenderRelativePath` in `serverless-handler.mjs`, which
/// reads these files back at request time.
pub(crate) fn prerender_html_path(prerender_dir: &Path, route_path: &str) -> Option<PathBuf> {
    let mut html_path = prerender_dir.to_path_buf();
    for segment in route_path.split('/') {
        if segment.is_empty() {
            continue;
        }
        if is_unsafe_prerender_segment(segment) {
            return None;
        }
        html_path.push(segment);
    }
    Some(html_path.join("index.html"))
}

pub(crate) fn is_unsafe_prerender_segment(segment: &str) -> bool {
    if segment == "." || segment == ".." {
        return true;
    }
    segment
        .chars()
        .any(|character| matches!(character, '/' | '\\' | ':') || character.is_control())
}

/// Generate a minimal CSR shell HTML document.
pub(crate) fn csr_shell_html(
    route_path: &str,
    client_assets: &BTreeMap<String, PrerenderClientAssets>,
    styles: &str,
) -> String {
    let assets = client_assets.get(route_path);
    let preload_links = assets
        .as_ref()
        .map(|assets| module_preload_links(&assets.preloads))
        .unwrap_or_default();
    let client_src = assets.map(|assets| assets.src.clone()).unwrap_or_else(|| {
        format!(
            "/__ruvyxa/client/{}.js",
            route_path.trim_start_matches('/').replace('/', "__")
        )
    });
    format!(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Loading...</title>
  <style data-ruvyxa-css>{styles}</style>
  {preload_links}
  {bootstrap}
</head>
<body>
  <div id="__ruvyxa"></div>
  <script type="module" src="{client_src}"></script>
</body>
</html>"#,
        client_src = escape_html_attribute(&client_src),
        // `params` is empty rather than the route's: this shell is written once
        // per route pattern, not per request, so it has no concrete parameters
        // to carry. The client bundle falls back to reading `location`.
        bootstrap = ruvyxa_dev_server::bootstrap_data_block(&RouteParams::new(), route_path, true),
    )
}

pub(crate) fn inject_prerender_styles(html: &str, styles: &str) -> String {
    let style_tag = format!(r#"<style data-ruvyxa-css>{styles}</style>"#);
    let lower = html.to_ascii_lowercase();
    if let Some(head_end) = lower.find("</head>") {
        let mut output = String::with_capacity(html.len() + style_tag.len());
        output.push_str(&html[..head_end]);
        output.push_str(&style_tag);
        output.push_str(&html[head_end..]);
        return output;
    }

    format!("<!doctype html><html><head>{style_tag}</head><body>{html}</body></html>")
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct PrerenderClientAssets {
    pub(crate) src: String,
    pub(crate) preloads: Vec<String>,
    pub(crate) hydration: HydrationMode,
    pub(crate) hydration_loader: Option<String>,
}

pub(crate) fn load_prerender_client_assets(
    client_dir: &Path,
) -> BTreeMap<String, PrerenderClientAssets> {
    let Ok(source) = fs::read_to_string(client_dir.join("manifest.json")) else {
        return BTreeMap::new();
    };
    let Ok(manifest) = serde_json::from_str::<serde_json::Value>(&source) else {
        return BTreeMap::new();
    };
    let Some(routes) = manifest.get("routes").and_then(|routes| routes.as_array()) else {
        return BTreeMap::new();
    };

    routes
        .iter()
        .filter_map(|route| {
            let path = route.get("path")?.as_str()?.to_string();
            let src = route.get("src")?.as_str()?.to_string();
            let preloads = route
                .get("sharedChunks")
                .and_then(|chunks| chunks.as_array())
                .into_iter()
                .flatten()
                .filter_map(|chunk| chunk.get("src").and_then(|src| src.as_str()))
                .map(str::to_string)
                .collect();
            let hydration = route
                .get("hydration")
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default();
            let hydration_loader = route
                .get("hydrationLoader")
                .and_then(|value| value.as_str())
                .map(str::to_string);
            Some((
                path,
                PrerenderClientAssets {
                    src,
                    preloads,
                    hydration,
                    hydration_loader,
                },
            ))
        })
        .collect()
}

pub(crate) fn module_preload_links(preloads: &[String]) -> String {
    preloads
        .iter()
        .map(|src| {
            let src = escape_html_attribute(src);
            format!(r#"<link rel="modulepreload" href="{src}">"#)
        })
        .collect()
}

/// Escape a value interpolated into a double-quoted HTML attribute.
///
/// Mirrors the dev server's `escape_html`, so a manifest entry cannot inject
/// markup in prerendered output while the same value stays inert in dev.
pub(crate) fn escape_html_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

pub(crate) fn inject_prerender_client_assets(
    html: &str,
    client_assets: &BTreeMap<String, PrerenderClientAssets>,
    route_path: &str,
    request_path: &str,
    params: &RouteParams,
) -> String {
    let Some(assets) = client_assets.get(route_path) else {
        return html.to_string();
    };
    let deferred = matches!(
        assets.hydration,
        HydrationMode::Idle | HydrationMode::Visible
    );
    let preload_links = if deferred {
        String::new()
    } else {
        module_preload_links(&assets.preloads)
    };
    let script_src = assets.hydration_loader.as_deref().map_or_else(
        || assets.src.clone(),
        |loader| ruvyxa_dev_server::hydration_loader_url(loader, &assets.src, assets.hydration),
    );
    let scripts = format!(
        r#"{}<script type="module" src="{}"></script>"#,
        ruvyxa_dev_server::bootstrap_data_block(params, request_path, false),
        escape_html_attribute(&script_src)
    );
    let lower = html.to_ascii_lowercase();
    if let (Some(head_end), Some(body_end)) = (lower.find("</head>"), lower.rfind("</body>"))
        && head_end <= body_end
    {
        let mut output = String::with_capacity(html.len() + preload_links.len() + scripts.len());
        output.push_str(&html[..head_end]);
        output.push_str(&preload_links);
        output.push_str(&html[head_end..body_end]);
        output.push_str(&scripts);
        output.push_str(&html[body_end..]);
        return output;
    }

    format!("<!doctype html><html><head>{preload_links}</head><body>{html}{scripts}</body></html>")
}

// `inline_script_json` lived here: it serialized a value and handed the result
// to `ruvyxa_dev_server::safe_json_for_script` before either of this module's
// two writers passed it on. Both now hand `bootstrap_data_block` the value
// itself and it does the serializing and escaping, so the helper had no callers
// and, more to the point, no reason to exist — a payload that escapes itself
// cannot be embedded unescaped.

#[cfg(test)]
mod csr_shell_tests {
    use super::*;

    /// The shell is not rendered from the route tree, so the client bootstrap
    /// has to be told to mount rather than hydrate. Without the flag React
    /// hydrates against markup that cannot match and reports #418.
    #[test]
    fn csr_shell_marks_itself_as_not_server_rendered() {
        let html = csr_shell_html("/hooks", &BTreeMap::new(), "");

        assert!(
            html.contains(r#""csr":true"#),
            "shell must flag itself for the client bootstrap: {html}"
        );
        // A data block, not an executable script. This writer was missed when
        // the other three were converted, because it names only the path and
        // the CSR flag — a search for the route-params global did not find it.
        assert!(
            html.contains(r#"<script type="application/json" id="__ruvyxa-bootstrap">"#),
            "{html}"
        );
        assert!(!html.contains("<script>"), "{html}");
    }
}
