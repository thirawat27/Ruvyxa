//! Reproducible production benchmark scenarios.
//!
//! The ordinary `bench` command remains a lightweight project probe. The
//! baseline mode in this module is deliberately stricter: every sample clones
//! the project into an ignored temporary workspace, uses a private build cache,
//! and exercises cold/warm builds, first-route rendering, and each supported
//! edit class through the real production pipeline. This makes cache and HMR
//! behavior explicit without deleting or warming the application's own cache.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, ensure};
use ruvyxa_graph::validate_app;
use serde::{Deserialize, Serialize};
use walkdir::{DirEntry, WalkDir};

use crate::*;

const BASELINE_CONTRACT_JSON: &str =
    include_str!("../../../tests/fixtures/build-bench-contract.json");
const GENERATED_TOP_LEVEL_ENTRIES: &[&str] = &[
    ".git",
    ".npm-pack",
    ".npm-smoke",
    ".ruvyxa",
    ".test-build",
    "dist",
    "node_modules",
    "target",
];
const TELEMETRY_FIELDS: &[&str] = &[
    "artifactCacheHit",
    "cacheHit",
    "cacheHits",
    "durationMs",
    "parallelism",
];

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaselineContract {
    schema_version: u32,
    methodology_id: String,
    budgets: BaselineBudgets,
    scenarios: Vec<BaselineScenarioContract>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaselineBudgets {
    max_scenario_ms: f64,
    max_peak_resident_bytes: u64,
    max_reload_fallbacks_per_sample: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BaselineScenarioContract {
    id: String,
    cache_state: String,
    mutation: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BaselineBenchmarkReport {
    schema_version: u32,
    methodology_id: String,
    samples: usize,
    runtime: String,
    edit_file: String,
    edit_files: BTreeMap<String, String>,
    isolated_workspace: bool,
    cold_warm_artifacts_equivalent: bool,
    peak_resident_bytes: u64,
    reload_fallbacks: usize,
    results: Vec<BaselineScenarioResult>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BaselineScenarioResult {
    #[serde(flatten)]
    timing: BenchmarkResult,
    cache_observations: Vec<BuildCacheObservation>,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct BuildCacheObservation {
    compile_hits: u64,
    graph_hits: u64,
    artifact_hits: u64,
}

struct BenchmarkWorkspace {
    path: PathBuf,
    parent: PathBuf,
}

impl BenchmarkWorkspace {
    /// A uniquely named directory under `parent`, removed when this value is
    /// dropped — and `parent` with it, if nothing else put anything there.
    fn disposable(parent: PathBuf, prefix: &str) -> anyhow::Result<Self> {
        let path = create_build_temp_dir(&parent, prefix)?;
        Ok(Self { path, parent })
    }

    /// Empties the directory without giving up the handle, for a scenario that
    /// needs the same path to start each sample with nothing in it.
    fn reset(&self) -> anyhow::Result<()> {
        ensure!(
            self.path.starts_with(&self.parent),
            "a benchmark directory must stay inside the directory it was created in"
        );
        if self.path.exists() {
            fs::remove_dir_all(&self.path)?;
        }
        fs::create_dir_all(&self.path)?;
        Ok(())
    }
}

impl Drop for BenchmarkWorkspace {
    fn drop(&mut self) {
        // `path` is created below `parent` and never reassigned. Keep the
        // containment check here anyway: cleanup code is a destructive boundary
        // and should fail closed if that invariant is ever changed.
        if self.path.starts_with(&self.parent) {
            let _ = fs::remove_dir_all(&self.path);
        }
        if self.parent.is_dir()
            && self
                .parent
                .read_dir()
                .is_ok_and(|mut entries| entries.next().is_none())
        {
            let _ = fs::remove_dir(&self.parent);
        }
    }
}

/// The scenarios `ruvyxa bench` reports, in the order a build reaches them,
/// each with the one line printed under the table to say what it measures.
///
/// The table is the declaration and [`run_project_benchmark`] is the
/// implementation; the two are checked against each other at the end of every
/// run rather than trusted to stay in step, because a scenario added to one and
/// not the other is invisible — the table would simply be missing a row nobody
/// knew to look for.
///
/// `build-cold` and `build-warm` replaced a single `production-build` row that
/// was reporting a lie. It ran N builds back to back against the project's own
/// cache, so the first sample was cold only if the project had never been
/// built, and the average mixed two costs that differ by an order of magnitude:
/// on the demo application the row reported a 938ms average while a genuinely
/// cold build took 7.7s. Splitting them is what makes the cache saving below
/// the table a measurement rather than an impression.
pub(crate) const PROJECT_SCENARIOS: [(&str, &str); 6] = [
    (
        "config-load",
        "reads ruvyxa.config.ts through the JavaScript runtime",
    ),
    (
        "route-discovery",
        "scans the app directory into a route manifest",
    ),
    (
        "route-validation",
        "checks every route and its server/client boundaries",
    ),
    (
        "build-cold",
        "a full production build against an empty cache",
    ),
    ("build-warm", "the same build with that cache reused"),
    (
        "first-route-render",
        "renders the first static page through the production server",
    ),
];

/// How much the build cache saved, as the two medians it is read from.
///
/// Carried as both numbers rather than as a percentage so the report can print
/// what was compared. A ratio with no operands is a claim; these are evidence.
pub(crate) struct CacheSaving {
    cold_ms: f64,
    warm_ms: f64,
}

impl CacheSaving {
    fn percent(&self) -> f64 {
        if self.cold_ms <= 0.0 {
            return 0.0;
        }
        ((self.cold_ms - self.warm_ms) / self.cold_ms) * 100.0
    }
}

/// The ordinary project probe: where this application's time actually goes.
///
/// Deliberately *not* the baseline. Nothing here clones the project or edits a
/// source file, so it stays cheap enough to run while working — the cost is
/// that only the cache can be controlled, not the source, which is why the edit
/// classes live in `--baseline` and not here.
pub(crate) async fn run_project_benchmark(args: &BenchArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    let samples = args.samples.max(1);
    let root = args.root.clone();
    let config = load_project_config(&root)?;
    let runtime = config.javascript_runtime().command().to_string();
    let app_dir = root.join(config.app_dir());
    let mut results = Vec::with_capacity(PROJECT_SCENARIOS.len());

    results.push(run_benchmark("config-load", &runtime, samples, || {
        load_project_config(&root)?;
        Ok(())
    })?);
    results.push(run_benchmark("route-discovery", &runtime, samples, || {
        discover_project_routes(&root, &config)?;
        Ok(())
    })?);
    results.push(run_benchmark(
        "route-validation",
        &runtime,
        samples,
        || {
            let manifest = discover_project_routes(&root, &config)?;
            let validation = validate_app(&root, &manifest)?;
            fail_on_diagnostics(&validation.diagnostics)?;
            Ok(())
        },
    )?);

    // A private cache directory, so neither scenario deletes or warms the cache
    // the project's own builds use. Cold is real: emptying this directory is
    // enough on its own, because reuse is decided here and nowhere else - a
    // build against a fresh cache costs the same whether or not the output
    // directory is already populated.
    let cache = BenchmarkWorkspace::disposable(root.join(".ruvyxa").join("bench"), ".cache")?;
    let mut cold = Vec::with_capacity(samples);
    for _ in 0..samples {
        cache.reset()?;
        let sample = Instant::now();
        build_with_cache_override(project_build_args(&root), false, Some(&cache.path)).await?;
        cold.push(sample.elapsed());
    }
    results.push(summarize_benchmark("build-cold", &runtime, cold));

    // The last cold sample already left the cache populated. Priming again
    // anyway keeps this scenario true on its own terms rather than true because
    // of what happens to run before it.
    build_with_cache_override(project_build_args(&root), false, Some(&cache.path)).await?;
    let mut warm = Vec::with_capacity(samples);
    for _ in 0..samples {
        let sample = Instant::now();
        build_with_cache_override(project_build_args(&root), false, Some(&cache.path)).await?;
        warm.push(sample.elapsed());
    }
    results.push(summarize_benchmark("build-warm", &runtime, warm));

    // Runs last because it needs the build the two scenarios above produced.
    let manifest = discover_project_routes(&root, &config)?;
    results.push(run_benchmark(
        "first-route-render",
        &runtime,
        samples,
        || render_first_route(&root, &config, &manifest),
    )?);

    ensure!(
        results
            .iter()
            .map(|result| result.name.as_str())
            .eq(PROJECT_SCENARIOS.iter().map(|(id, _)| *id)),
        "the benchmark produced scenarios the table in PROJECT_SCENARIOS does not declare, or skipped one it does"
    );
    let saving = cache_saving(&results);

    if args.json {
        write_machine_report(&serde_json::to_string_pretty(&results)?)?;
    } else {
        print_benchmark_table(samples, &results, &root, &app_dir, started.elapsed());
        print_cache_saving(saving);
        print_section("what each row measures");
        for (id, description) in PROJECT_SCENARIOS {
            print_field(id, dim(description));
        }
        println!();
    }

    Ok(())
}

/// The build every build scenario runs: this project, its configured target and
/// adapter, nothing overridden. A benchmark that measures a build no user asks
/// for is measuring the wrong thing.
fn project_build_args(root: &Path) -> BuildArgs {
    BuildArgs {
        root: root.to_path_buf(),
        target: None,
        adapter: None,
        runtime: None,
        server_only: false,
    }
}

fn cache_saving(results: &[BenchmarkResult]) -> Option<CacheSaving> {
    let median = |name: &str| {
        results
            .iter()
            .find(|result| result.name == name)
            .map(|result| result.median_ms)
    };
    Some(CacheSaving {
        cold_ms: median("build-cold")?,
        warm_ms: median("build-warm")?,
    })
}

fn print_cache_saving(saving: Option<CacheSaving>) {
    let Some(saving) = saving else {
        return;
    };
    let summary = format!(
        "{:.0}% · {} cold, {} warm",
        saving.percent(),
        format_duration(Duration::from_secs_f64(saving.cold_ms / 1000.0)),
        format_duration(Duration::from_secs_f64(saving.warm_ms / 1000.0)),
    );
    print_field(
        "cache saving",
        if saving.percent() >= 50.0 {
            ok_text(summary)
        } else {
            warn_text(summary)
        },
    );
}

/// Run the versioned production-build and edit-class baseline.
pub(crate) async fn run_baseline_benchmark(args: &BenchArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    let samples = args.samples.max(1);
    // Canonical so every sample and every containment check below compares
    // against one absolute path, and respelled because `canonicalize` writes an
    // extended-length `\\?\` prefix on Windows. A benchmark measuring a shape
    // of path no user's build has is measuring the wrong thing — and this one
    // did: the prefix reached the bundler and nothing else, so the workspace
    // failed to build for a reason the project itself never had.
    let source_root = fs::canonicalize(&args.root)
        .map(|path| ruvyxa_diagnostics::without_verbatim_prefix(&path))
        .with_context(|| format!("failed to resolve benchmark root {}", args.root.display()))?;
    let source_config = load_project_config(&source_root)?;
    let source_out_dir = safe_relative_directory(source_config.out_dir(), "build output")?;
    let contract = baseline_contract()?;
    let scenario_ids = validated_scenario_ids(&contract)?;
    let mut timings = vec![Vec::with_capacity(samples); scenario_ids.len()];
    let mut observations = (0..scenario_ids.len())
        .map(|_| Vec::with_capacity(samples))
        .collect::<Vec<_>>();
    let mut edit_file = None;
    let mut edit_files = BTreeMap::new();
    let mut peak_resident_bytes = 0_u64;
    let mut reload_fallbacks = 0_usize;

    for _ in 0..samples {
        let workspace = create_benchmark_workspace(&source_root, &source_out_dir)?;
        let workspace_config = load_project_config(&workspace.path)?;
        let workspace_out_dir = workspace.path.join(safe_relative_directory(
            workspace_config.out_dir(),
            "build output",
        )?);
        let cache_dir = workspace.path.join(".ruvyxa").join("benchmark-cache");
        let manifest = discover_project_routes(&workspace.path, &workspace_config)?;
        let editable_file = benchmark_edit_file(&workspace.path, &manifest)?;
        let css_file = benchmark_source_file(&workspace.path, "css", &editable_file)?;
        let client_file = benchmark_source_file(&workspace.path, "client", &editable_file)?;
        let server_file = benchmark_source_file(&workspace.path, "server", &editable_file)?;
        edit_file.get_or_insert_with(|| {
            editable_file
                .strip_prefix(&workspace.path)
                .unwrap_or(&editable_file)
                .to_string_lossy()
                .replace('\\', "/")
        });

        let cold_started = Instant::now();
        benchmark_build(&workspace.path, &cache_dir, &scenario_ids[0]).await?;
        timings[0].push(cold_started.elapsed());
        observations[0].push(read_cache_observation(&workspace_out_dir)?);
        let cold_artifacts = semantic_build_artifacts(&workspace_out_dir)?;

        let warm_started = Instant::now();
        benchmark_build(&workspace.path, &cache_dir, &scenario_ids[1]).await?;
        timings[1].push(warm_started.elapsed());
        observations[1].push(read_cache_observation(&workspace_out_dir)?);
        let warm_artifacts = semantic_build_artifacts(&workspace_out_dir)?;
        if cold_artifacts != warm_artifacts {
            anyhow::bail!(
                "cold and warm builds emitted different semantic artifacts ({}); refusing to publish a misleading baseline",
                semantic_artifact_differences(&cold_artifacts, &warm_artifacts).join(", ")
            );
        }

        let route_started = Instant::now();
        render_first_route(&workspace.path, &workspace_config, &manifest)?;
        timings[2].push(route_started.elapsed());
        observations[2].push(BuildCacheObservation::default());

        apply_leaf_edit(&css_file)?;
        let css_started = Instant::now();
        benchmark_build(&workspace.path, &cache_dir, &scenario_ids[3]).await?;
        timings[3].push(css_started.elapsed());
        observations[3].push(read_cache_observation(&workspace_out_dir)?);

        apply_leaf_edit(&client_file)?;
        let client_started = Instant::now();
        benchmark_build(&workspace.path, &cache_dir, &scenario_ids[4]).await?;
        timings[4].push(client_started.elapsed());
        observations[4].push(read_cache_observation(&workspace_out_dir)?);

        apply_leaf_edit(&server_file)?;
        let server_started = Instant::now();
        benchmark_build(&workspace.path, &cache_dir, &scenario_ids[5]).await?;
        timings[5].push(server_started.elapsed());
        observations[5].push(read_cache_observation(&workspace_out_dir)?);

        apply_leaf_edit(&editable_file)?;
        let edit_started = Instant::now();
        benchmark_build(&workspace.path, &cache_dir, &scenario_ids[6]).await?;
        timings[6].push(edit_started.elapsed());
        observations[6].push(read_cache_observation(&workspace_out_dir)?);

        let tracker = ruvyxa_dev_server::HmrTracker::new();
        tracker.populate_from_manifest(&manifest.routes);
        reload_fallbacks += [&css_file, &client_file, &server_file]
            .into_iter()
            .filter(|path| {
                tracker
                    .compute_update(std::slice::from_ref(path))
                    .full_reload
            })
            .count();
        peak_resident_bytes = peak_resident_bytes.max(process_peak_resident_bytes());
        for (kind, file) in [
            ("css", &css_file),
            ("client", &client_file),
            ("server", &server_file),
            ("leaf", &editable_file),
        ] {
            edit_files.entry(kind.to_string()).or_insert_with(|| {
                file.strip_prefix(&workspace.path)
                    .unwrap_or(file)
                    .to_string_lossy()
                    .replace('\\', "/")
            });
        }
    }

    let runtime = source_config.javascript_runtime().command().to_string();
    let results = scenario_ids
        .into_iter()
        .enumerate()
        .map(|(index, name)| BaselineScenarioResult {
            timing: summarize_benchmark(&name, &runtime, std::mem::take(&mut timings[index])),
            cache_observations: std::mem::take(&mut observations[index]),
        })
        .collect::<Vec<_>>();
    let report = BaselineBenchmarkReport {
        schema_version: contract.schema_version,
        methodology_id: contract.methodology_id,
        samples,
        runtime,
        edit_file: edit_file.unwrap_or_default(),
        edit_files,
        isolated_workspace: true,
        cold_warm_artifacts_equivalent: true,
        peak_resident_bytes,
        reload_fallbacks,
        results,
    };
    ensure!(
        report
            .results
            .iter()
            .all(|result| result.timing.p95_ms <= contract.budgets.max_scenario_ms),
        "a benchmark scenario exceeded the fixture p95 budget of {}ms",
        contract.budgets.max_scenario_ms
    );
    ensure!(
        report.peak_resident_bytes <= contract.budgets.max_peak_resident_bytes,
        "benchmark peak resident memory exceeded the fixture budget"
    );
    ensure!(
        report.reload_fallbacks
            <= contract
                .budgets
                .max_reload_fallbacks_per_sample
                .saturating_mul(samples),
        "benchmark reload fallbacks exceeded the fixture budget"
    );

    if args.json {
        write_machine_report(&serde_json::to_string_pretty(&report)?)?;
    } else {
        let rows = report
            .results
            .iter()
            .map(|result| result.timing.clone())
            .collect::<Vec<_>>();
        print_benchmark_table(
            samples,
            &rows,
            &source_root,
            &source_root.join(source_config.app_dir()),
            started.elapsed(),
        );
        print_field("method", report.methodology_id);
        print_field("workspace", "isolated per sample".to_string());
        print_field("artifact proof", ok_text("cold = warm"));
        print_field("edit file", report.edit_file);
        print_field(
            "peak resident",
            format_bytes(report.peak_resident_bytes as usize),
        );
        print_field("reload fallbacks", report.reload_fallbacks.to_string());
        println!();
    }

    Ok(())
}

/// Run one baseline build and name the scenario it belongs to if it fails.
///
/// Every sample runs seven builds that differ only in what the previous step
/// edited, so an unqualified error says nothing about which one broke: a
/// failure after the client edit and a failure on the very first cold build
/// arrive as the same sentence. The scenario id comes from the contract, so the
/// name in the error is the name in the report.
async fn benchmark_build(root: &Path, cache_dir: &Path, scenario: &str) -> anyhow::Result<()> {
    build_with_cache_override(
        BuildArgs {
            root: root.to_path_buf(),
            target: None,
            adapter: None,
            runtime: None,
            server_only: false,
        },
        false,
        Some(cache_dir),
    )
    .await
    .with_context(|| format!("benchmark scenario {scenario} failed"))
}

fn baseline_contract() -> anyhow::Result<BaselineContract> {
    serde_json::from_str(BASELINE_CONTRACT_JSON)
        .context("build baseline conformance fixture is invalid")
}

fn validated_scenario_ids(contract: &BaselineContract) -> anyhow::Result<Vec<String>> {
    let actual = contract
        .scenarios
        .iter()
        .map(|scenario| {
            (
                scenario.id.as_str(),
                scenario.cache_state.as_str(),
                scenario.mutation.as_str(),
            )
        })
        .collect::<Vec<_>>();
    let expected = vec![
        ("cold-build", "empty", "none"),
        ("warm-build", "reused", "none"),
        ("first-route", "built", "none"),
        ("css-edit-build", "reused", "css-source"),
        ("client-edit-build", "reused", "client-source"),
        ("server-edit-build", "reused", "server-source"),
        ("leaf-edit-build", "reused", "leaf-source"),
    ];
    ensure!(
        actual == expected,
        "build baseline fixture must define the complete cold, warm, route, and edit scenarios in dependency order"
    );
    Ok(contract
        .scenarios
        .iter()
        .map(|scenario| scenario.id.clone())
        .collect())
}

fn safe_relative_directory(value: &str, label: &str) -> anyhow::Result<PathBuf> {
    let path = PathBuf::from(value);
    ensure!(!path.as_os_str().is_empty(), "{label} directory is empty");
    ensure!(
        !path.is_absolute(),
        "{label} directory must be project-relative"
    );
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir)),
        "{label} directory must stay inside the project root"
    );
    Ok(path)
}

fn create_benchmark_workspace(
    source_root: &Path,
    source_out_dir: &Path,
) -> anyhow::Result<BenchmarkWorkspace> {
    let parent = source_root.join(".ruvyxa").join("bench");
    let workspace = BenchmarkWorkspace::disposable(parent, ".workspace")?;
    copy_benchmark_project(source_root, &workspace.path, source_out_dir)?;
    Ok(workspace)
}

fn copy_benchmark_project(source: &Path, destination: &Path, out_dir: &Path) -> anyhow::Result<()> {
    for entry in WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| benchmark_entry_is_included(entry, source, out_dir))
    {
        let entry = entry.with_context(|| {
            format!(
                "failed to walk {} while preparing the benchmark workspace",
                source.display()
            )
        })?;
        if entry.depth() == 0 {
            continue;
        }
        let relative = entry.path().strip_prefix(source)?;
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target).with_context(|| {
                format!("failed to copy benchmark input {}", entry.path().display())
            })?;
        } else if entry.file_type().is_symlink() {
            anyhow::bail!(
                "benchmark input {} is a symbolic link; use a real project file so the isolated baseline cannot escape its workspace",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn benchmark_entry_is_included(entry: &DirEntry, root: &Path, out_dir: &Path) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    let Ok(relative) = entry.path().strip_prefix(root) else {
        return false;
    };
    if relative == out_dir || relative.starts_with(out_dir) {
        return false;
    }
    let Some(first) = relative.components().next() else {
        return false;
    };
    let Component::Normal(first) = first else {
        return false;
    };
    !GENERATED_TOP_LEVEL_ENTRIES
        .iter()
        .any(|name| first == std::ffi::OsStr::new(name))
}

fn benchmark_edit_file(
    root: &Path,
    manifest: &ruvyxa_graph::RouteManifest,
) -> anyhow::Result<PathBuf> {
    let mut routes = manifest.routes.iter().collect::<Vec<_>>();
    routes.sort_by(|left, right| left.path.cmp(&right.path).then(left.id.cmp(&right.id)));
    let route = routes
        .into_iter()
        .find(|route| route.file.is_file())
        .context("the project has no route source file to use for the leaf-edit benchmark")?;
    // Respelled like `source_root`, or the containment check below compares a
    // verbatim path against an ordinary one and rejects a file that is inside
    // the workspace.
    let file = fs::canonicalize(&route.file)
        .map(|path| ruvyxa_diagnostics::without_verbatim_prefix(&path))
        .with_context(|| {
            format!(
                "failed to resolve benchmark edit file {}",
                route.file.display()
            )
        })?;
    ensure!(
        file.starts_with(root),
        "benchmark edit file {} escapes the isolated project workspace",
        file.display()
    );
    Ok(file)
}

fn benchmark_source_file(root: &Path, kind: &str, fallback: &Path) -> anyhow::Result<PathBuf> {
    let mut files = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| benchmark_entry_is_included(entry, root, Path::new("dist")))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect::<Vec<_>>();
    files.sort();
    let selected = match kind {
        "css" => files.into_iter().find(|file| {
            matches!(
                file.extension().and_then(|value| value.to_str()),
                Some("css" | "scss" | "sass" | "less")
            )
        }),
        "client" => files.into_iter().find(|file| {
            fs::read_to_string(file).is_ok_and(|source| {
                ruvyxa_bundler::reference_manifest::has_module_directive(&source, "use client")
            })
        }),
        "server" => files.into_iter().find(|file| {
            let name = file
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            name.starts_with("action.")
                || name.starts_with("route.")
                || file.components().any(|part| part.as_os_str() == "server")
        }),
        _ => None,
    };
    if kind == "css" {
        selected.context("the project has no stylesheet for the CSS-edit benchmark")
    } else {
        Ok(selected.unwrap_or_else(|| fallback.to_path_buf()))
    }
}

pub(crate) fn render_first_route(
    root: &Path,
    config: &ProjectConfig,
    manifest: &ruvyxa_graph::RouteManifest,
) -> anyhow::Result<()> {
    let route = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .find(|route| !route.path.contains('['))
        .context("the project has no static page path for the first-route benchmark")?;
    let server = production_server_config(
        &ServerArgs {
            root: root.to_path_buf(),
            host: None,
            port: None,
            runtime: None,
        },
        config,
    )?;
    let context = ruvyxa_dev_server::RenderContext::new(&server)?;
    let response =
        ruvyxa_dev_server::render_request_with_context(&server, &context, &route.path, "GET")?;
    ensure!(
        response.status().is_success(),
        "first route returned {}",
        response.status()
    );
    Ok(())
}

#[cfg(target_os = "linux")]
fn process_peak_resident_bytes() -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("VmHWM:")?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
        })
        .unwrap_or_default()
        .saturating_mul(1024)
}

#[cfg(windows)]
fn process_peak_resident_bytes() -> u64 {
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    let mut counters = PROCESS_MEMORY_COUNTERS {
        cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        PageFaultCount: 0,
        PeakWorkingSetSize: 0,
        WorkingSetSize: 0,
        QuotaPeakPagedPoolUsage: 0,
        QuotaPagedPoolUsage: 0,
        QuotaPeakNonPagedPoolUsage: 0,
        QuotaNonPagedPoolUsage: 0,
        PagefileUsage: 0,
        PeakPagefileUsage: 0,
    };
    // SAFETY: both pointers reference the current process and a correctly sized,
    // initialized structure for the duration of this synchronous call.
    let ok = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
        )
    };
    if ok == 0 {
        0
    } else {
        counters.PeakWorkingSetSize as u64
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn process_peak_resident_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `getrusage` initializes the provided structure on success.
    let ok = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if ok != 0 {
        return 0;
    }
    // macOS reports bytes, unlike Linux's kilobytes.
    unsafe { usage.assume_init().ru_maxrss.max(0) as u64 }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios", windows)))]
fn process_peak_resident_bytes() -> u64 {
    0
}

fn apply_leaf_edit(path: &Path) -> anyhow::Result<()> {
    let marker = match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "md" | "mdx" => "\n\n<!-- ruvyxa benchmark leaf edit -->\n",
        _ => "\n/* ruvyxa benchmark leaf edit */\n",
    };
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .and_then(|mut file| file.write_all(marker.as_bytes()))
        .with_context(|| format!("failed to apply isolated edit to {}", path.display()))
}

/// Read one build's cache counters out of its client build report.
///
/// The report is `client-report.json` at the build root — it used to be
/// `client/manifest.json`, and this function is the third reader the move had
/// to reach. It missed this one, and the shape of the miss is the reason this
/// no longer treats a missing report as an observation: the old code returned
/// `BuildCacheObservation::default()`, so the benchmark went on to publish
/// "zero cache hits" for every warm scenario. A benchmark that cannot find its
/// telemetry and answers zero is worse than one that stops, because the zero
/// is indistinguishable from a real measurement of a cache that never hit.
///
/// Every caller reads this straight after a completed `benchmark_build`, which
/// never runs `--server-only`, so the report is always written. A file that is
/// not there means the path is wrong, not that the cache was cold — the one
/// legitimate "nothing to observe" scenario (`first-route`, which renders
/// rather than builds) pushes `BuildCacheObservation::default()` itself.
fn read_cache_observation(out_dir: &Path) -> anyhow::Result<BuildCacheObservation> {
    let path = client_build_report_path(out_dir);
    ensure!(
        path.is_file(),
        "benchmark build emitted no client report at {}; refusing to report zero cache hits for a measurement that was never taken",
        path.display()
    );
    let value: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)
        .with_context(|| format!("failed to read cache telemetry from {}", path.display()))?;
    let artifact_hits = value
        .get("routes")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter(|route| {
            route
                .get("artifactCacheHit")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false)
        })
        .count() as u64;
    Ok(BuildCacheObservation {
        compile_hits: value
            .get("cacheHits")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        graph_hits: value
            .pointer("/cache/graphHits")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default(),
        artifact_hits,
    })
}

fn semantic_build_artifacts(out_dir: &Path) -> anyhow::Result<BTreeMap<String, String>> {
    let mut files = WalkDir::new(out_dir)
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    files.retain(|entry| entry.file_type().is_file());
    files.sort_by(|left, right| left.path().cmp(right.path()));
    ensure!(!files.is_empty(), "benchmark build emitted no files");

    let mut artifacts = BTreeMap::new();
    for entry in files {
        let relative = entry.path().strip_prefix(out_dir)?;
        let normalized = relative.to_string_lossy().replace('\\', "/");
        if normalized == "build.json" {
            continue;
        }
        let source = fs::read(entry.path())?;
        // The client build report is telemetry-bearing and no longer lives in
        // `client/`; comparing it raw would report a cold/warm diff on the
        // cache counters it exists to record.
        let normalized_source = if normalized == CLIENT_BUILD_REPORT_FILE
            || matches!(
                normalized.as_str(),
                "assets/.ruvyxa-images.json" | "prerender/manifest.json"
            ) {
            normalize_telemetry_json(&source)?
        } else {
            source
        };
        artifacts.insert(
            normalized,
            blake3::hash(&normalized_source).to_hex().to_string(),
        );
    }
    Ok(artifacts)
}

fn semantic_artifact_differences(
    cold: &BTreeMap<String, String>,
    warm: &BTreeMap<String, String>,
) -> Vec<String> {
    let paths = cold
        .keys()
        .chain(warm.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut differences = paths
        .into_iter()
        .filter(|path| cold.get(path) != warm.get(path))
        .collect::<Vec<_>>();
    const DIAGNOSTIC_LIMIT: usize = 8;
    if differences.len() > DIAGNOSTIC_LIMIT {
        let remaining = differences.len() - DIAGNOSTIC_LIMIT;
        differences.truncate(DIAGNOSTIC_LIMIT);
        differences.push(format!("+{remaining} more"));
    }
    differences
}

fn normalize_telemetry_json(source: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut value: serde_json::Value = serde_json::from_slice(source)?;
    remove_telemetry_fields(&mut value);
    Ok(serde_json::to_vec(&value)?)
}

fn remove_telemetry_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for field in TELEMETRY_FIELDS {
                object.remove(*field);
            }
            // The client manifest's cache object contains its private directory
            // and process-local counters, never deployed behavior.
            object.remove("cache");
            for child in object.values_mut() {
                remove_telemetry_fields(child);
            }
        }
        serde_json::Value::Array(values) => {
            for child in values {
                remove_telemetry_fields(child);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn cli_keeps_legacy_bench_default_and_opts_into_the_baseline() {
        let legacy = Cli::try_parse_from(["ruvyxa", "bench"]).unwrap();
        let Command::Bench(legacy) = legacy.command else {
            panic!("bench command did not parse")
        };
        assert!(!legacy.baseline);

        let baseline =
            Cli::try_parse_from(["ruvyxa", "bench", "--baseline", "--samples", "5", "--json"])
                .unwrap();
        let Command::Bench(baseline) = baseline.command else {
            panic!("bench command did not parse")
        };
        assert!(baseline.baseline);
        assert_eq!(baseline.samples, 5);
        assert!(baseline.json);
    }

    #[test]
    fn baseline_fixture_defines_the_dependency_order() {
        let contract = baseline_contract().unwrap();
        assert_eq!(contract.schema_version, 1);
        assert_eq!(contract.methodology_id, "ruvyxa.build-bench");
        assert_eq!(
            validated_scenario_ids(&contract).unwrap(),
            [
                "cold-build",
                "warm-build",
                "first-route",
                "css-edit-build",
                "client-edit-build",
                "server-edit-build",
                "leaf-edit-build",
            ]
        );
    }

    #[test]
    fn semantic_fingerprint_ignores_only_build_telemetry() {
        let temp = tempfile::tempdir().unwrap();
        let client = temp.path().join("client");
        fs::create_dir_all(&client).unwrap();
        fs::write(temp.path().join("manifest.json"), r#"{"routes":[]}"#).unwrap();
        fs::write(
            client_build_report_path(temp.path()),
            r#"{"routes":[{"file":"a.js","cacheHits":0}],"durationMs":10,"cache":{"directory":"one"}}"#,
        )
        .unwrap();
        fs::write(client.join("a.js"), "export default 1").unwrap();
        let cold = semantic_build_artifacts(temp.path()).unwrap();

        fs::write(
            client_build_report_path(temp.path()),
            r#"{"routes":[{"file":"a.js","cacheHits":9}],"durationMs":1,"cache":{"directory":"two"}}"#,
        )
        .unwrap();
        assert_eq!(cold, semantic_build_artifacts(temp.path()).unwrap());

        fs::write(client.join("a.js"), "export default 2").unwrap();
        assert_ne!(cold, semantic_build_artifacts(temp.path()).unwrap());
    }

    /// The counters live in the client build report, which moved out of the
    /// published `client/` directory to the build root. A benchmark reading the
    /// old path found nothing and reported a cache that never hit.
    #[test]
    fn cache_observation_reads_the_client_report_at_the_build_root() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("client")).unwrap();
        fs::write(
            client_build_report_path(temp.path()),
            r#"{"cacheHits":7,"cache":{"graphHits":4},"routes":[{"artifactCacheHit":true},{"artifactCacheHit":false},{"artifactCacheHit":true}]}"#,
        )
        .unwrap();

        let observation = read_cache_observation(temp.path()).unwrap();
        assert_eq!(observation.compile_hits, 7);
        assert_eq!(observation.graph_hits, 4);
        assert_eq!(observation.artifact_hits, 2);
    }

    /// "The report is not there" and "the report says zero" are different
    /// answers, and only one of them is a real measurement. Reporting the
    /// missing file as a zeroed observation is how the move to
    /// `client-report.json` stayed invisible: the number still looked real.
    #[test]
    fn missing_client_report_fails_instead_of_reporting_zero_cache_hits() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("client")).unwrap();
        // The published directory the report used to live in is not a fallback.
        fs::write(
            temp.path().join("client").join("manifest.json"),
            r#"{"cacheHits":7}"#,
        )
        .unwrap();

        let error = read_cache_observation(temp.path()).unwrap_err().to_string();
        assert!(
            error.contains(CLIENT_BUILD_REPORT_FILE),
            "error should name the report it could not find: {error}"
        );
    }

    #[test]
    fn project_copy_excludes_generated_state_and_keeps_sources() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        fs::create_dir_all(source.path().join("app")).unwrap();
        fs::create_dir_all(source.path().join("node_modules/pkg")).unwrap();
        fs::create_dir_all(source.path().join("custom-output")).unwrap();
        fs::write(source.path().join("app/page.tsx"), "export default 1").unwrap();
        fs::write(source.path().join("node_modules/pkg/index.js"), "generated").unwrap();
        fs::write(source.path().join("custom-output/build.json"), "{}").unwrap();

        copy_benchmark_project(
            source.path(),
            destination.path(),
            Path::new("custom-output"),
        )
        .unwrap();

        assert!(destination.path().join("app/page.tsx").is_file());
        assert!(!destination.path().join("node_modules").exists());
        assert!(!destination.path().join("custom-output").exists());
    }

    #[test]
    fn leaf_edit_uses_syntax_safe_markers() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("page.tsx");
        let markdown = temp.path().join("page.mdx");
        fs::write(&script, "export default 1").unwrap();
        fs::write(&markdown, "# Page").unwrap();

        apply_leaf_edit(&script).unwrap();
        apply_leaf_edit(&markdown).unwrap();

        assert!(fs::read_to_string(script).unwrap().contains("/* ruvyxa"));
        assert!(
            fs::read_to_string(markdown)
                .unwrap()
                .contains("<!-- ruvyxa")
        );
    }
}
