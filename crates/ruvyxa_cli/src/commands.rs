//! The non-build CLI commands: `routes`, `analyze`, `check`, `doctor`,
//! `clean`, `trace`, `bench`, and `test:parity`.
//!
//! These share one shape: load the project config, discover the route manifest,
//! report. None of them write build output, and `check` is the one that must
//! agree with `build` — it validates through the same graph and boundary rules
//! rather than a second implementation of them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use std::io::{IsTerminal, Write};
use std::process::Command as ProcessCommand;
use std::time::{Duration, Instant};

use anyhow::Context;
use ruvyxa_dev_server::{RenderContext, ServerConfig, render_request_with_context};
use ruvyxa_diagnostics::{Diagnostic, diagnostics_to_sarif};
use ruvyxa_graph::{DiscoverOptions, RouteEntry, RouteManifest, discover_routes, validate_app};
use walkdir::WalkDir;

use crate::*;

pub(crate) fn print_routes(args: RoutesArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let app_dir = args.root.join(config.app_dir());
    let manifest = discover_project_routes(&args.root, &config)?;
    if args.json {
        write_machine_report(&serde_json::to_string_pretty(&manifest)?)?;
        return Ok(());
    }
    let page_routes = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .count();
    let api_routes = manifest.routes.len().saturating_sub(page_routes);

    print_tui_header("Routes");
    print_field("root", path_text(&args.root));
    print_field("app dir", path_text(&app_dir));
    print_field("routes", number(manifest.routes.len().to_string()));
    print_field("pages", info(page_routes.to_string()));
    print_field("api", note(api_routes.to_string()));
    println!();

    // The route id duplicates the file path, so the table omits it to stay
    // narrow enough for typical terminals.
    let rows = manifest
        .routes
        .iter()
        .map(|route| {
            [
                format!("{:?}", route.kind).to_lowercase(),
                route.path.clone(),
                display_path_relative(&args.root, &route.file),
                match route.kind {
                    ruvyxa_graph::RouteKind::Page => {
                        format!("{:?}", route.render.strategy).to_lowercase()
                    }
                    _ => "-".to_string(),
                },
            ]
        })
        .collect::<Vec<_>>();
    let headers = ["kind", "path", "file", "strategy"];
    let widths = column_widths(&headers, &rows);

    print_table_separator(&widths);
    print_box_row(
        headers,
        [
            label(headers[0]),
            label(headers[1]),
            label(headers[2]),
            label(headers[3]),
        ],
        &widths,
        ROUTE_TABLE_ALIGNMENT,
    );
    print_table_separator(&widths);
    for (row, route) in rows.iter().zip(manifest.routes.iter()) {
        let strategy = match route.kind {
            ruvyxa_graph::RouteKind::Page => styled_strategy_word(route.render.strategy),
            _ => dim("-").to_string(),
        };
        print_box_row(
            [&row[0], &row[1], &row[2], &row[3]],
            [
                styled_route_kind(route.kind),
                row[1].clone(),
                dim(&row[2]),
                strategy,
            ],
            &widths,
            ROUTE_TABLE_ALIGNMENT,
        );
    }
    print_table_separator(&widths);
    println!();

    Ok(())
}

/// Every column of the route table holds text, so all four read left-aligned.
const ROUTE_TABLE_ALIGNMENT: [bool; 4] = [false; 4];

/// Page and API routes are the table's main axis; painting both the same accent
/// made the column decoration rather than information.
pub(crate) fn styled_route_kind(kind: ruvyxa_graph::RouteKind) -> String {
    match kind {
        ruvyxa_graph::RouteKind::Page => info("page"),
        ruvyxa_graph::RouteKind::Api => note("api"),
    }
}

pub(crate) fn analyze(args: AnalyzeArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    let config = load_project_config(&args.root)?;
    let manifest = discover_project_routes(&args.root, &config)?;
    let validation = validate_app(&args.root, &manifest)?;
    if args.html && args.format != AnalyzeFormat::Auto {
        anyhow::bail!("--html cannot be combined with --format; use one report selector");
    }
    let format = match (args.html, args.format) {
        (true, _) => AnalyzeFormat::Html,
        (false, format) => match format {
            AnalyzeFormat::Auto if args.output.is_some() => AnalyzeFormat::Json,
            AnalyzeFormat::Auto if std::io::stdout().is_terminal() => AnalyzeFormat::Human,
            AnalyzeFormat::Auto => AnalyzeFormat::Json,
            explicit => explicit,
        },
    };
    if format == AnalyzeFormat::Human && args.output.is_some() {
        anyhow::bail!("--output requires --format json, sarif, or html");
    }

    // Keep the machine-readable JSON contract for pipes and scripts; render the
    // house TUI only when a person is looking at a terminal.
    let serialized = match format {
        AnalyzeFormat::Human => None,
        AnalyzeFormat::Json => Some(serde_json::to_string_pretty(&validation)?),
        AnalyzeFormat::Sarif => Some(serde_json::to_string_pretty(&diagnostics_to_sarif(
            &validation.diagnostics,
            "Ruvyxa",
            env!("CARGO_PKG_VERSION"),
            &args.root,
        ))?),
        AnalyzeFormat::Html => {
            let bundle = analyze_client_bundle(&args.root, &config, &manifest)?;
            Some(render_analyzer_html(
                &args.root,
                &manifest,
                &validation,
                &bundle,
            )?)
        }
        AnalyzeFormat::Auto => unreachable!("auto format is resolved above"),
    };

    if let Some(report) = serialized {
        let html_default_output =
            (args.html && args.output.is_none()).then(|| args.root.join(".ruvyxa/analyze.html"));
        if let Some(output) = args.output.as_ref().or(html_default_output.as_ref()) {
            if let Some(parent) = output
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
            {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create report directory {}", parent.display())
                })?;
            }
            fs::write(output, format!("{report}\n"))
                .with_context(|| format!("failed to write analysis report {}", output.display()))?;
            if format == AnalyzeFormat::Html {
                print_tui_header("Analyze");
                print_field("format", accent("interactive html"));
                print_field("report", path_text(output));
                println!();
            }
        } else {
            write_machine_report(&report)?;
        }
    } else {
        print_tui_header("Analyze");
        print_field("root", path_text(&args.root));
        print_field("routes", number(validation.routes.to_string()));
        print_field("pages", info(validation.page_routes.to_string()));
        print_field("api", note(validation.api_routes.to_string()));
        print_field(
            "client modules",
            number(validation.client_modules.to_string()),
        );
        print_field(
            "server modules",
            number(validation.server_modules.to_string()),
        );
        if validation.is_ok() {
            print_field("diagnostics", ok_text("none"));
            print_success_banner("No issues found", started.elapsed());
        } else {
            print_field(
                "diagnostics",
                warn_text(validation.diagnostics.len().to_string()),
            );
            println!();
            for diagnostic in &validation.diagnostics {
                eprintln!("{diagnostic}");
            }
        }
    }

    if !validation.is_ok() {
        anyhow::bail!(
            "analysis found {} diagnostic(s); fix them before building",
            validation.diagnostics.len()
        );
    }

    Ok(())
}

/// Writes machine-readable output without treating a downstream closed pipe as a CLI failure.
fn write_machine_report(report: &str) -> anyhow::Result<()> {
    write_machine_report_to(&mut std::io::stdout().lock(), report)
}

fn write_machine_report_to(writer: &mut impl Write, report: &str) -> anyhow::Result<()> {
    if let Err(error) = writeln!(writer, "{report}")
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(error).context("failed to write machine-readable report");
    }
    Ok(())
}

pub(crate) async fn check(args: ProjectArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    print_tui_header("Check");
    print_field("root", path_text(&args.root));
    println!();

    // Route types must exist before `tsc` runs, or the first `check` in a fresh
    // clone type-checks against a registry the build has not written yet and
    // reports every `<Link href>` as an error.
    generate_route_types(&args.root)?;
    run_typecheck(&args.root)?;
    test_parity(args).await?;

    print_success_banner("Production readiness checks passed", started.elapsed());
    Ok(())
}

/// Write `.ruvyxa/types/routes.d.ts` for `check`, and report a config that
/// generates it without ever type-checking it.
///
/// The unreferenced-file case is a warning rather than a failure: the project
/// still builds and runs correctly, it simply gets none of the benefit.
pub(crate) fn generate_route_types(root: &Path) -> anyhow::Result<()> {
    let config = load_project_config(root)?;
    if !config.typed_routes() {
        return Ok(());
    }

    let manifest = discover_project_routes(root, &config)?;
    let output = write_route_types(root, &manifest)?;
    if output.included_by_tsconfig {
        println!(
            "{} typed routes generated ({} routes)",
            success(),
            output.routes
        );
    } else {
        println!("{}", tsconfig_include_diagnostic(root));
    }
    Ok(())
}

pub(crate) fn run_typecheck(root: &Path) -> anyhow::Result<()> {
    if !root.join("tsconfig.json").exists() {
        println!("{} TypeScript skipped (no tsconfig.json)", success());
        return Ok(());
    }

    let tsc = local_binary_upwards(root, "tsc").unwrap_or_else(|| PathBuf::from("tsc"));
    let mut command = ProcessCommand::new(&tsc);
    command.arg("--noEmit").current_dir(root);
    let output = ruvyxa_dev_server::process::output_with_timeout(
        &mut command,
        ruvyxa_dev_server::process::TYPECHECK_TIMEOUT,
    )
    .with_context(|| format!("failed to run TypeScript type check with {}", tsc.display()))?;

    if output.status.success() {
        println!("{} TypeScript type check passed", success());
        return Ok(());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    anyhow::bail!("TypeScript type check failed\nstdout:\n{stdout}\nstderr:\n{stderr}")
}

pub(crate) fn doctor(args: DoctorArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    let config = load_project_config(&args.root)?;
    let app_dir = args.root.join(config.app_dir());
    let package_json = args.root.join("package.json");
    let tsconfig = args.root.join("tsconfig.json");
    let (_, tsconfig_problem) = ruvyxa_bundler::resolver::TsConfigPaths::load_reporting(&args.root);
    let manifest = discover_project_routes(&args.root, &config)?;
    let validation = validate_app(&args.root, &manifest)?;
    let build_target = config.build_target(args.target);
    let build_target_name = format!("{build_target:?}").to_ascii_lowercase();
    let detected_adapter = if args.adapter.is_none() && config.adapter.is_none() {
        detect_platform_adapter(|key| std::env::var(key).ok())
    } else {
        None
    };
    let adapter_name = args
        .adapter
        .as_deref()
        .or_else(|| detected_adapter.as_ref().map(|(name, _)| name.as_str()));
    let adapter = if adapter_name.is_some() || config.adapter.is_some() {
        inspect_adapter(
            &args.root,
            &args.root.join(config.out_dir()),
            config.javascript_runtime(),
            adapter_name,
        )?
    } else {
        None
    }
    .unwrap_or_else(|| AdapterInspection {
        name: "ruvyxa-native".to_string(),
        target: build_target_name.clone(),
        runtime: config.javascript_runtime().command().to_string(),
        platform: None,
        supports: vec!["ssr", "ssg", "csr", "isr", "ppr", "api"]
            .into_iter()
            .map(str::to_string)
            .collect(),
    });
    let supported = adapter
        .supports
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let unsupported_routes = manifest
        .routes
        .iter()
        .filter_map(|route| {
            let capability = match route.kind {
                ruvyxa_graph::RouteKind::Api => "api".to_string(),
                ruvyxa_graph::RouteKind::Page => {
                    format!("{:?}", route.render.strategy).to_ascii_lowercase()
                }
            };
            (!supported.contains(capability.as_str()))
                .then(|| serde_json::json!({ "path": route.path, "requires": capability }))
        })
        .collect::<Vec<_>>();

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "frameworkVersion": env!("CARGO_PKG_VERSION"),
                "root": args.root,
                "buildTarget": build_target_name,
                "javascriptRuntime": config.javascript_runtime().command(),
                "adapter": adapter,
                "routes": manifest.routes.len(),
                "unsupportedRoutes": unsupported_routes,
                "tsconfigProblem": tsconfig_problem.as_ref().map(|problem| serde_json::json!({
                    "path": problem.path,
                    "message": problem.message,
                })),
                "diagnostics": validation.diagnostics,
            }))?
        );
        return Ok(());
    }

    let package = package_json
        .exists()
        .then(|| read_package_json(&package_json))
        .transpose()?;

    print_tui_header("Doctor");

    // Twenty-five fields in one block is a wall. The groups below are the
    // questions a reader actually arrives with: which Ruvyxa is this, where is
    // the project, what is installed, what will it deploy to, and what does the
    // graph look like.
    print_section("ruvyxa");
    print_field("cli", info(env!("CARGO_PKG_VERSION")));
    match &package {
        Some(package) => {
            let packages = ruvyxa_dependencies(package);
            let declared = dependency_version(package, "ruvyxa");
            print_field(
                "version match",
                compatibility_status(cli_version_match(
                    declared.as_deref(),
                    env!("CARGO_PKG_VERSION"),
                )),
            );
            if packages.is_empty() {
                print_field("packages", warn_text("none installed"));
            }
            for (name, version) in packages {
                // The scope is already the section heading, and the longest
                // scoped name is wider than the label column, so it is dropped
                // rather than allowed to push every value out of alignment.
                print_field(
                    name.strip_prefix("@ruvyxa/").unwrap_or(&name),
                    note(version),
                );
            }
        }
        None => print_field("version match", warn_text("no package.json")),
    }

    print_section("project");
    print_field("root", path_text(&args.root));
    print_field("config", exists_status(&args.root.join("ruvyxa.config.ts")));
    print_field("app dir", path_text(&app_dir));
    print_field("out dir", path_text(&args.root.join(config.out_dir())));
    print_field("app directory", exists_status(&app_dir));
    print_field("package.json", exists_status(&package_json));
    // "exists" was the whole answer here, and it was the wrong question: a
    // tsconfig Ruvyxa cannot parse exists just as hard as one it can, and the
    // only symptom was every aliased import failing to resolve with no mention
    // of the config that had been skipped.
    match &tsconfig_problem {
        Some(problem) => print_field(
            "tsconfig.json",
            warn_text(format!("unreadable — {}", problem.message)),
        ),
        None => print_field("tsconfig.json", exists_status(&tsconfig)),
    }

    print_section("toolchain");
    print_field("package manager", info(detect_package_manager(&args.root)));
    print_field("node", tool_status(tool_version("node", &["--version"])));
    print_field("rustc", tool_status(tool_version("rustc", &["--version"])));
    print_field("cargo", tool_status(tool_version("cargo", &["--version"])));
    print_field("bun", tool_status(bun_version()));
    print_field("deno", tool_status(deno_version()));
    if let Some(package) = &package {
        // React is part of what is installed, not a section of its own — the
        // three rows only ever answered "can this project render".
        print_field(
            "react",
            tool_status(
                dependency_version(package, "react").unwrap_or_else(|| "missing".to_string()),
            ),
        );
        print_field(
            "react-dom",
            tool_status(
                dependency_version(package, "react-dom").unwrap_or_else(|| "missing".to_string()),
            ),
        );
        print_field(
            "react compatibility",
            compatibility_status(react_compatibility(package)),
        );

        let duplicates = duplicate_dependencies(package);
        if duplicates.is_empty() {
            print_field("dependency duplicates", ok_text("ok"));
        } else {
            print_field("dependency duplicates", warn_text(duplicates.join(", ")));
        }
    }

    print_section("adapter");
    print_field("build target", info(&build_target_name));
    print_field("adapter", accent(&adapter.name));
    print_field("adapter target", info(&adapter.target));
    print_field("adapter runtime", info(&adapter.runtime));
    print_field("adapter supports", note(adapter.supports.join(", ")));
    if let Some(platform) = &adapter.platform {
        print_field("adapter platform", note(platform));
    }

    print_section("graph");
    print_field("routes", number(manifest.routes.len().to_string()));
    print_field("page routes", info(validation.page_routes.to_string()));
    print_field("api routes", note(validation.api_routes.to_string()));
    print_field(
        "client modules",
        number(validation.client_modules.to_string()),
    );
    print_field(
        "server modules",
        number(validation.server_modules.to_string()),
    );
    print_field(
        "diagnostics",
        if validation.diagnostics.is_empty() {
            ok_text("0")
        } else {
            warn_text(validation.diagnostics.len().to_string())
        },
    );
    print_field("env schema", exists_status(&args.root.join(".env.example")));
    print_field("native binary", ok_text("ok"));
    if unsupported_routes.is_empty() {
        print_field("target compatibility", ok_text("all routes supported"));
    } else {
        print_field(
            "target compatibility",
            warn_text(format!("{} unsupported route(s)", unsupported_routes.len())),
        );
        for route in &unsupported_routes {
            eprintln!(
                "  {} {} requires {}",
                warn_text("!"),
                route["path"].as_str().unwrap_or("/"),
                route["requires"].as_str().unwrap_or("unknown")
            );
        }
    }

    // Doctor never fails the process — it reports. The verdict line is what
    // saves the reader from scanning twenty-five fields to learn whether
    // anything needs attention.
    let concerns = validation.diagnostics.len() + unsupported_routes.len();
    if concerns == 0 {
        print_success_banner("Everything checks out", started.elapsed());
    } else {
        println!(
            "\n  {} {} {}\n",
            warn_text("!"),
            warn_text(format!("{concerns} item(s) need attention")),
            dim(format!("· {}", format_duration(started.elapsed())))
        );
    }
    Ok(())
}

pub(crate) fn clean(args: ProjectArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    let config = load_project_config(&args.root)?;
    let out_dir = args.root.join(config.out_dir());
    let removed = out_dir.exists();
    if removed {
        fs::remove_dir_all(&out_dir)?;
    }
    print_tui_header("Clean");
    print_field(
        "status",
        if removed {
            ok_text("removed")
        } else {
            dim("already clean")
        },
    );
    print_field("out dir", path_text(&out_dir));
    print_success_banner_at(
        if removed {
            "Removed"
        } else {
            "Nothing to remove at"
        },
        Some(&out_dir),
        started.elapsed(),
    );
    Ok(())
}

pub(crate) fn trace(args: TraceArgs) -> anyhow::Result<()> {
    let config = load_project_config(&args.root)?;
    let manifest = discover_project_routes(&args.root, &config)?;
    let route = manifest
        .routes
        .iter()
        .find(|entry| entry.path == args.route)
        .with_context(|| format!("route {} was not found", args.route))?;

    println!("{}", serde_json::to_string_pretty(route)?);
    Ok(())
}

pub(crate) async fn bench(args: BenchArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    let samples = args.samples.max(1);
    let root = args.root;
    let config = load_project_config(&root)?;
    let runtime = config.javascript_runtime().command().to_string();
    let app_dir = root.join(config.app_dir());
    let mut results = Vec::new();

    results.push(run_benchmark("route-discovery", &runtime, samples, || {
        let _manifest = discover_project_routes(&root, &config)?;
        Ok(())
    })?);
    results.push(run_benchmark(
        "analyze-validation",
        &runtime,
        samples,
        || {
            let manifest = discover_project_routes(&root, &config)?;
            let validation = validate_app(&root, &manifest)?;
            fail_on_diagnostics(&validation.diagnostics)?;
            Ok(())
        },
    )?);
    let mut build_timings = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        build_with_output(
            BuildArgs {
                root: root.clone(),
                target: None,
                adapter: None,
                runtime: None,
                server_only: false,
            },
            false,
        )
        .await?;
        build_timings.push(started.elapsed());
    }
    results.push(summarize_benchmark(
        "production-build",
        &runtime,
        build_timings,
    ));

    if args.json {
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        print_benchmark_table(samples, &results, &root, &app_dir, started.elapsed());
        println!();
    }

    Ok(())
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BenchmarkResult {
    pub(crate) name: String,
    pub(crate) samples: usize,
    pub(crate) runtime: String,
    pub(crate) sample_ms: Vec<f64>,
    pub(crate) min_ms: f64,
    pub(crate) median_ms: f64,
    pub(crate) avg_ms: f64,
    pub(crate) max_ms: f64,
    pub(crate) p95_ms: f64,
    pub(crate) std_dev_ms: f64,
}

pub(crate) fn run_benchmark(
    name: &str,
    runtime: &str,
    samples: usize,
    mut run: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<BenchmarkResult> {
    let mut timings = Vec::with_capacity(samples);

    for _ in 0..samples {
        let started = Instant::now();
        run()?;
        timings.push(started.elapsed());
    }

    Ok(summarize_benchmark(name, runtime, timings))
}

pub(crate) fn summarize_benchmark(
    name: &str,
    runtime: &str,
    mut timings: Vec<Duration>,
) -> BenchmarkResult {
    timings.sort();
    let samples = timings.len();
    let min_ms = duration_ms(timings[0]);
    let max_ms = duration_ms(timings[samples - 1]);
    let median_ms = duration_ms(timings[samples / 2]);
    let avg_ms = timings
        .iter()
        .map(|duration| duration_ms(*duration))
        .sum::<f64>()
        / samples as f64;
    let sample_ms = timings
        .iter()
        .map(|duration| duration_ms(*duration))
        .collect::<Vec<_>>();
    let p95_index = ((samples as f64 * 0.95).ceil() as usize)
        .saturating_sub(1)
        .min(samples - 1);
    let p95_ms = sample_ms[p95_index];
    let variance = sample_ms
        .iter()
        .map(|sample| (sample - avg_ms).powi(2))
        .sum::<f64>()
        / samples as f64;

    BenchmarkResult {
        name: name.to_string(),
        samples,
        runtime: runtime.to_string(),
        sample_ms,
        min_ms,
        median_ms,
        avg_ms,
        max_ms,
        p95_ms,
        std_dev_ms: variance.sqrt(),
    }
}

pub(crate) fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

pub(crate) async fn test_parity(args: ProjectArgs) -> anyhow::Result<()> {
    let started = Instant::now();
    let config = load_project_config(&args.root)?;
    print_tui_header("Parity");
    print_field("root", path_text(&args.root));
    print_field("dev app", path_text(&args.root.join(config.app_dir())));
    print_field(
        "prod app",
        path_text(
            &args
                .root
                .join(config.out_dir())
                .join("server")
                .join(config.app_dir()),
        ),
    );
    println!();
    build(BuildArgs {
        root: args.root.clone(),
        target: None,
        adapter: None,
        runtime: None,
        server_only: false,
    })
    .await?;

    // Timed from here, not from `started`: `started` also covers the production
    // build above, and reporting that as the comparison's duration would claim
    // the manifest diff took ten seconds.
    let comparison_started = Instant::now();
    let dev_manifest = discover_project_routes(&args.root, &config)?;
    let prod_manifest = discover_routes(
        DiscoverOptions::new(
            args.root
                .join(config.out_dir())
                .join("server")
                .join(config.app_dir()),
        )
        .with_rendering_defaults(
            config.rendering.default_strategy,
            config.rendering.default_revalidate,
        ),
    )?;
    let dev_routes = parity_routes(&dev_manifest);
    let prod_routes = parity_routes(&prod_manifest);
    let mut failures = Vec::new();

    // One line per matching route said nothing twenty-four times over. The
    // count below carries the same fact, and every mismatch is still reported
    // in full.
    let mut matched = 0;
    for (key, dev_route) in &dev_routes {
        match prod_routes.get(key) {
            Some(prod_route) if prod_route == dev_route => matched += 1,
            Some(prod_route) => {
                failures.push(format!(
                    "{key} mismatch\n  dev:  {:?}\n  prod: {:?}",
                    dev_route, prod_route
                ));
            }
            None => failures.push(format!("{key} exists in dev but not production")),
        }
    }

    for key in prod_routes.keys() {
        if !dev_routes.contains_key(key) {
            failures.push(format!("{key} exists in production but not dev"));
        }
    }
    print_phase(
        "manifests matched",
        format!("{matched} of {} routes", dev_routes.len()),
        comparison_started.elapsed(),
    );
    println!();

    failures.extend(smoke_render_parity(
        &dev_server_config(
            &ServerArgs {
                root: args.root.clone(),
                host: None,
                port: None,
                runtime: None,
            },
            &config,
        )?,
        &production_server_config(
            &ServerArgs {
                root: args.root.clone(),
                host: None,
                port: None,
                runtime: None,
            },
            &config,
        )?,
        &dev_manifest,
    ));

    if failures.is_empty() {
        print_success_banner(
            format!("Parity passed for {} routes", dev_routes.len()),
            started.elapsed(),
        );
        return Ok(());
    }

    for failure in failures {
        eprintln!("{} {failure}", error_label());
    }

    anyhow::bail!("dev/prod parity failed")
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ParityRoute {
    pub(crate) file: String,
    pub(crate) layout_chain: Vec<String>,
    pub(crate) server_modules: Vec<String>,
    pub(crate) client_modules: Vec<String>,
    pub(crate) runtime: String,
}

pub(crate) fn parity_routes(manifest: &RouteManifest) -> BTreeMap<String, ParityRoute> {
    manifest
        .routes
        .iter()
        .map(|route| {
            (
                format!("{:?} {}", route.kind, route.path),
                parity_route(manifest, route),
            )
        })
        .collect()
}

pub(crate) fn parity_route(manifest: &RouteManifest, route: &RouteEntry) -> ParityRoute {
    ParityRoute {
        file: normalize_route_path(&manifest.app_dir, &route.file),
        layout_chain: route.layout_chain.clone(),
        server_modules: normalize_module_paths(manifest, &route.server_modules),
        client_modules: normalize_module_paths(manifest, &route.client_modules),
        runtime: format!("{:?}", route.runtime),
    }
}

pub(crate) fn smoke_render_parity(
    dev_config: &ServerConfig,
    prod_config: &ServerConfig,
    manifest: &RouteManifest,
) -> Vec<String> {
    let mut failures = Vec::new();

    // One context per config. Each must discover its own route graph: the dev
    // config points at the source app directory and the production config at
    // the built one, and the two disagree on every route's module paths.
    // Rendering through `render_request` instead would redo that discovery,
    // recompile the router, and re-collect every stylesheet twice per route.
    let (dev_context, prod_context) = match (
        RenderContext::new(dev_config),
        RenderContext::new(prod_config),
    ) {
        (Ok(dev), Ok(prod)) => (dev, prod),
        (Err(error), _) | (_, Err(error)) => {
            failures.push(format!("route discovery failed for smoke render: {error}"));
            return failures;
        }
    };

    let pages = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .collect::<Vec<_>>();
    let path_width = pages
        .iter()
        .map(|route| display_width(&route.path))
        .max()
        .unwrap_or(0);

    for route in pages {
        let request_path = parity_smoke_path(&route.path);
        let dev = smoke_render_side(
            "dev",
            dev_config,
            &dev_context,
            &route.path,
            &request_path,
            &mut failures,
        );
        let prod = smoke_render_side(
            "prod",
            prod_config,
            &prod_context,
            &route.path,
            &request_path,
            &mut failures,
        );

        // Both sides on one line: the question is whether dev and prod agree,
        // and two separate lines made the reader hold one of them in their head
        // while looking for the other.
        println!(
            "  {} {}{}  {} {}  {} {}",
            if dev && prod {
                ok_text("✓")
            } else {
                alert_text("✗")
            },
            route.path,
            spaces(path_width, display_width(&route.path)),
            label("dev"),
            render_mark(dev),
            label("prod"),
            render_mark(prod)
        );
    }

    failures
}

fn render_mark(ok: bool) -> String {
    if ok {
        ok_text("ok")
    } else {
        alert_text("fail")
    }
}

/// Renders one side of a parity smoke test, recording a failure rather than
/// returning it so the caller can report both sides of a route together.
fn smoke_render_side(
    side: &str,
    config: &ServerConfig,
    context: &RenderContext,
    route_path: &str,
    request_path: &str,
    failures: &mut Vec<String>,
) -> bool {
    match render_request_with_context(config, context, request_path, "GET") {
        Ok(response) if !response.status().is_server_error() => true,
        Ok(response) => {
            failures.push(format!(
                "Page {route_path} {side} runtime render returned {} for {request_path}",
                response.status()
            ));
            false
        }
        Err(error) => {
            failures.push(format!(
                "Page {route_path} {side} runtime render failed for {request_path}: {error}"
            ));
            false
        }
    }
}

pub(crate) fn parity_smoke_path(route_path: &str) -> String {
    if route_path == "/" {
        return "/".to_string();
    }

    let segments = route_path
        .trim_start_matches('/')
        .split('/')
        .filter_map(|segment| {
            if segment.starts_with("[[...") && segment.ends_with("]]") {
                None
            } else if segment.starts_with("[...") && segment.ends_with(']') {
                Some("smoke/path")
            } else if segment.starts_with('[') && segment.ends_with(']') {
                Some("smoke")
            } else {
                Some(segment)
            }
        })
        .collect::<Vec<_>>();

    if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    }
}

pub(crate) fn normalize_module_paths(manifest: &RouteManifest, paths: &[String]) -> Vec<String> {
    let mut paths = paths
        .iter()
        .map(|path| normalize_route_path(&manifest.app_dir, Path::new(path)))
        .collect::<Vec<_>>();
    paths.sort();
    paths
}

pub(crate) fn normalize_route_path(app_dir: &Path, path: &Path) -> String {
    path.strip_prefix(app_dir)
        .unwrap_or(path)
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().replace('\\', "/")),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

pub(crate) fn copy_style_sources(
    root: &Path,
    server_dir: &Path,
    files: &[PathBuf],
) -> anyhow::Result<()> {
    let root = ruvyxa_diagnostics::normalized_canonical_path(root);
    for file in files {
        let file = ruvyxa_diagnostics::normalized_canonical_path(file);
        let Ok(relative) = file.strip_prefix(&root) else {
            continue;
        };
        if relative.starts_with("node_modules") {
            continue;
        }
        // A style collection records watch inputs as well as stylesheets, and a
        // PostCSS plugin may report a whole directory as one — Tailwind reports
        // the trees it scans for class names that way. Those belong in the watch
        // set, not in the copied server sources.
        if !file.is_file() {
            continue;
        }
        let target = server_dir.join(relative);
        if target == file {
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(file, target)?;
    }
    Ok(())
}

pub(crate) fn copy_optional_dir(from: &Path, to: &Path) -> anyhow::Result<()> {
    if from.exists() {
        copy_dir_all(from, to)?;
    }
    Ok(())
}

pub(crate) fn copy_dir_all(from: &Path, to: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(to)?;

    // A walk error (permission denied, locked file, broken reparse point)
    // must fail the copy: dropping the entry would commit a silently
    // incomplete build tree that only surfaces later at runtime.
    for entry in WalkDir::new(from) {
        let entry = entry
            .with_context(|| format!("failed to walk {} during build copy", from.display()))?;
        let relative = entry.path().strip_prefix(from)?;
        let target = to.join(relative);

        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(entry.path(), &target)?;
        }
    }

    Ok(())
}

pub(crate) fn count_files(path: &Path) -> usize {
    if !path.exists() {
        return 0;
    }

    WalkDir::new(path)
        .into_iter()
        .filter_map(std::result::Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .count()
}

pub(crate) fn fail_on_diagnostics(diagnostics: &[Diagnostic]) -> anyhow::Result<()> {
    if diagnostics.is_empty() {
        return Ok(());
    }

    for diagnostic in diagnostics {
        eprintln!("{diagnostic}");
    }

    anyhow::bail!(
        "build validation failed with {} diagnostic(s)",
        diagnostics.len()
    )
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use super::write_machine_report_to;

    struct ClosedPipe;

    impl Write for ClosedPipe {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::from(io::ErrorKind::BrokenPipe))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn ignores_closed_pipe_for_machine_reports() {
        assert!(write_machine_report_to(&mut ClosedPipe, "{}\n").is_ok());
    }
}
