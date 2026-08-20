//! Unit tests for the CLI crate root and its sibling modules.
//!
//! These live in one file rather than beside each module because they were
//! written against the crate as a single unit and still exercise it that way:
//! `use super::*` here means the crate root, where every module is re-exported.

use super::{
    detect_platform_adapter, is_npm_package_name, is_unsafe_prerender_segment, parse_adapter_name,
    prerender_html_path,
};

#[test]
fn adapter_names_accept_known_and_package_shapes() {
    assert_eq!(parse_adapter_name("vercel").unwrap(), "vercel");
    assert_eq!(parse_adapter_name(" Netlify ").unwrap(), "netlify");
    assert_eq!(parse_adapter_name("Railway").unwrap(), "railway");
    assert_eq!(parse_adapter_name("Render").unwrap(), "render");
    assert_eq!(parse_adapter_name("Firebase").unwrap(), "firebase");
    assert_eq!(parse_adapter_name("AWS").unwrap(), "aws");
    assert_eq!(
        parse_adapter_name("@acme/ruvyxa-adapter-deno").unwrap(),
        "@acme/ruvyxa-adapter-deno"
    );
    assert_eq!(
        parse_adapter_name("ruvyxa-adapter-fastly").unwrap(),
        "ruvyxa-adapter-fastly"
    );

    assert!(parse_adapter_name("").is_err());
    assert!(parse_adapter_name("@bad").is_err());
    assert!(parse_adapter_name("bad/../escape").is_err());
    assert!(parse_adapter_name(".hidden").is_err());
}

#[test]
fn npm_package_name_rejects_path_like_values() {
    assert!(is_npm_package_name("@scope/name"));
    assert!(is_npm_package_name("plain-name"));
    assert!(!is_npm_package_name("a/b"));
    assert!(!is_npm_package_name("@scope/"));
    assert!(!is_npm_package_name("..\\escape"));
}

#[test]
fn platform_detection_reads_hosting_environment() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |key: &str| {
            vars.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        }
    };

    assert_eq!(
        detect_platform_adapter(env(&[("VERCEL", "1")])),
        Some(("vercel".to_string(), "VERCEL".to_string()))
    );
    assert_eq!(
        detect_platform_adapter(env(&[("NETLIFY", "true")])),
        Some(("netlify".to_string(), "NETLIFY".to_string()))
    );
    assert_eq!(
        detect_platform_adapter(env(&[("CF_PAGES", "1")])),
        Some(("cloudflare".to_string(), "CF_PAGES".to_string()))
    );
    assert_eq!(
        detect_platform_adapter(env(&[("RAILWAY_PROJECT_ID", "project-id")])),
        Some(("railway".to_string(), "RAILWAY_PROJECT_ID".to_string()))
    );
    assert_eq!(
        detect_platform_adapter(env(&[("RENDER", "true")])),
        Some(("render".to_string(), "RENDER".to_string()))
    );
    assert_eq!(
        detect_platform_adapter(env(&[("AWS_APP_ID", "amplify-app-id")])),
        Some(("aws".to_string(), "AWS_APP_ID".to_string()))
    );

    // Explicit override wins over the platform variable.
    assert_eq!(
        detect_platform_adapter(env(&[("RUVYXA_ADAPTER", "node"), ("VERCEL", "1")])),
        Some(("node".to_string(), "RUVYXA_ADAPTER".to_string()))
    );

    // Disabled or absent values fall through.
    assert_eq!(detect_platform_adapter(env(&[("VERCEL", "0")])), None);
    assert_eq!(detect_platform_adapter(env(&[("NETLIFY", "false")])), None);
    assert_eq!(detect_platform_adapter(env(&[])), None);
}

#[test]
fn prerender_paths_stay_inside_the_build_output() {
    let root = std::path::Path::new("/out/prerender");

    assert_eq!(
        prerender_html_path(root, "/"),
        Some(root.join("index.html"))
    );
    assert_eq!(
        prerender_html_path(root, "/blog/hello-world"),
        Some(root.join("blog").join("hello-world").join("index.html"))
    );

    // Render paths for dynamic routes come from the app's own
    // getStaticParams(), so a parameter value must never be able to walk
    // out of the build output or name a Windows stream.
    for escaping in [
        "/../etc/passwd",
        "/blog/../../secret",
        "/blog/./x",
        "/blog/a\\b",
        "/blog/a:b",
    ] {
        assert_eq!(prerender_html_path(root, escaping), None, "{escaping}");
    }
}

#[test]
fn unsafe_prerender_segments_cover_separators_and_control_characters() {
    assert!(is_unsafe_prerender_segment(".."));
    assert!(is_unsafe_prerender_segment("."));
    assert!(is_unsafe_prerender_segment("a\\b"));
    assert!(is_unsafe_prerender_segment("a:b"));
    assert!(is_unsafe_prerender_segment("a\u{7f}b"));
    assert!(!is_unsafe_prerender_segment("hello-world"));
    assert!(!is_unsafe_prerender_segment("a%20b"));
}

use clap::CommandFactory;
use serde_json::json;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ruvyxa_dev_server::{
    JavaScriptRuntime, MAX_ACTION_BODY_LIMIT_BYTES, MAX_ACTION_RATE_LIMIT_REQUESTS,
    MAX_ACTION_RATE_LIMIT_WINDOW_SECS, MAX_API_BODY_LIMIT_BYTES,
    MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES, TrustedProxies, find_runtime_script,
};
use ruvyxa_graph::{DiscoverOptions, RenderStrategy, RouteParams, discover_routes};

use super::*;

#[test]
fn plugin_create_scaffolds_the_canonical_plugin() {
    let temp = tempfile::tempdir().unwrap();

    scaffold_plugin(PluginCreateArgs {
        name: "request-logger".to_string(),
        root: temp.path().to_path_buf(),
        dir: None,
    })
    .unwrap();

    let plugin_dir = temp.path().join("request-logger");
    let source = fs::read_to_string(plugin_dir.join("src/index.ts")).unwrap();
    assert!(source.contains("import { definePlugin } from 'ruvyxa/plugin'"));
    assert!(source.contains("name: 'request-logger'"));
    assert!(source.contains("headers: { 'x-request-logger': 'active' }"));
    assert!(!source.contains("register({ http })"));
    let package: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(plugin_dir.join("package.json")).unwrap())
            .unwrap();
    assert_eq!(package["name"], "ruvyxa-plugin-request-logger");
    assert!(package.get("ruvyxa").is_none());
    assert_eq!(package["devDependencies"]["typescript"], "^7.0.2");
    assert_eq!(
        package["peerDependencies"]["ruvyxa"],
        format!("^{}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(package["scripts"]["prepublishOnly"], "npm test");
    assert!(plugin_dir.join("tsconfig.json").exists());
    assert!(plugin_dir.join("test/plugin.test.mjs").exists());
    assert!(plugin_dir.join(".gitignore").exists());
    let readme = fs::read_to_string(plugin_dir.join("README.md")).unwrap();
    assert!(readme.contains("ruvyxa-plugin-request-logger"));
    assert!(readme.contains("x-request-logger: active"));
    assert!(!temp.path().join("plugins").exists());
}

#[test]
fn plugin_create_leaves_no_authoring_literal_behind() {
    // Every other scaffold test uses `request-logger`, which is also the name
    // the templates were authored against. That makes a hardcoded literal
    // indistinguishable from a correctly substituted placeholder, and one did
    // survive in the generated test until it was caught by scaffolding under a
    // different name. Scaffold under a name that shares nothing with the
    // authoring name so any residue fails loudly.
    let temp = tempfile::tempdir().unwrap();

    scaffold_plugin(PluginCreateArgs {
        name: "audit-trail".to_string(),
        root: temp.path().to_path_buf(),
        dir: None,
    })
    .unwrap();

    let plugin_dir = temp.path().join("audit-trail");
    for (relative_path, _) in PLUGIN_TEMPLATE_FILES {
        let contents = fs::read_to_string(plugin_dir.join(relative_path))
            .unwrap_or_else(|error| panic!("{relative_path} should be scaffolded: {error}"));

        // A leftover authoring name means the template hardcoded a value where
        // it should have used a placeholder.
        assert!(
            !contents.contains("request-logger") && !contents.contains("request_logger"),
            "{relative_path} still carries the authoring plugin name:\n{contents}"
        );
        // An unsubstituted placeholder means a token was misspelled.
        assert!(
            !contents.contains("__PLUGIN_NAME__")
                && !contents.contains("__PLUGIN_IDENTIFIER__")
                && !contents.contains("__RUVYXA_VERSION__"),
            "{relative_path} has an unsubstituted placeholder:\n{contents}"
        );
    }
}

#[test]
fn plugin_create_scaffolds_into_a_custom_directory() {
    let temp = tempfile::tempdir().unwrap();

    scaffold_plugin(PluginCreateArgs {
        name: "request-logger".to_string(),
        root: temp.path().to_path_buf(),
        dir: Some(PathBuf::from("tools/my-logger")),
    })
    .unwrap();

    let plugin_dir = temp.path().join("tools/my-logger");
    assert!(plugin_dir.join("src/index.ts").exists());
    assert!(!temp.path().join("plugins").exists());
    let package: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(plugin_dir.join("package.json")).unwrap())
            .unwrap();
    assert_eq!(package["name"], "ruvyxa-plugin-request-logger");
}

#[test]
fn plugin_create_rejects_custom_directory_traversal() {
    let temp = tempfile::tempdir().unwrap();
    let error = scaffold_plugin(PluginCreateArgs {
        name: "request-logger".to_string(),
        root: temp.path().to_path_buf(),
        dir: Some(PathBuf::from("../outside")),
    })
    .unwrap_err()
    .to_string();

    assert!(error.contains("--dir must not contain `..`"));
}

#[test]
fn plugin_create_rejects_absolute_custom_directory() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("plugin");

    let error = scaffold_plugin(PluginCreateArgs {
        name: "request-logger".to_string(),
        root: root.path().to_path_buf(),
        dir: Some(target.clone()),
    })
    .unwrap_err()
    .to_string();

    assert!(error.contains("--dir must be relative to --root"));
    assert!(!target.exists());
}

#[test]
fn plugin_create_rejects_unsafe_names() {
    let temp = tempfile::tempdir().unwrap();
    let error = scaffold_plugin(PluginCreateArgs {
        name: "../escape".to_string(),
        root: temp.path().to_path_buf(),
        dir: None,
    })
    .unwrap_err()
    .to_string();

    assert!(error.contains("plugin name must use lowercase"));
    assert!(!temp.path().join("escape").exists());
}

#[test]
fn plugin_create_rejects_repeated_hyphens() {
    let temp = tempfile::tempdir().unwrap();
    let error = scaffold_plugin(PluginCreateArgs {
        name: "request--logger".to_string(),
        root: temp.path().to_path_buf(),
        dir: None,
    })
    .unwrap_err()
    .to_string();

    assert!(error.contains("single hyphens"));
    assert!(!temp.path().join("request--logger").exists());
}

#[test]
fn plugin_cli_exposes_only_create_without_a_template_selector() {
    let cli = Cli::try_parse_from(["ruvyxa", "plugin", "create", "request-logger"])
        .expect("plugin create should parse");
    let Command::Plugin(plugin) = cli.command else {
        panic!("expected plugin command");
    };
    assert!(matches!(plugin.command, PluginCommand::Create(_)));

    assert!(Cli::try_parse_from(["ruvyxa", "plugin", "unsupported", "request-logger"]).is_err());
    assert!(
        Cli::try_parse_from([
            "ruvyxa",
            "plugin",
            "create",
            "request-logger",
            "--template",
            "http"
        ])
        .is_err()
    );
}

#[test]
fn adapter_runner_materializes_declared_artifacts_in_staging() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let staging = root.join(".ruvyxa-build-staging");
    fs::create_dir_all(&staging).unwrap();
    fs::write(
        root.join("ruvyxa.config.mjs"),
        r#"
export default {
  adapter: {
name: 'fixture',
target: 'serverless',
supports: ['ssg', 'api'],
build() {
  return {
    name: 'fixture',
    target: 'serverless',
    runtime: 'node',
    platform: 'aws',
    artifacts: [
      { kind: 'file', path: 'deploy/health.txt', contents: 'ready\\n' }
    ]
  }
}
  }
}
"#,
    )
    .unwrap();

    let inspection = inspect_adapter(root, &staging, JavaScriptRuntime::Node, None)
        .unwrap()
        .unwrap();
    assert_eq!(inspection.name, "fixture");
    assert_eq!(inspection.target, "serverless");
    assert_eq!(inspection.supports, ["ssg", "api"]);
    assert!(!staging.join("deploy/health.txt").exists());

    let artifacts = run_adapter_runner(root, &staging, JavaScriptRuntime::Node, None).unwrap();

    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].kind, "file");
    assert_eq!(artifacts[0].path, "deploy/health.txt");
    assert_eq!(
        fs::read_to_string(staging.join("deploy/health.txt")).unwrap(),
        "ready\\n"
    );
}

#[test]
fn config_renderer_invalid_output_reports_empty_stdout_and_stderr() {
    let error = parse_config_renderer_output(
        Path::new("."),
        b"",
        b"SyntaxError: Unexpected token",
        "exit status: 1",
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("config renderer returned invalid output for ."));
    assert!(error.contains("status: exit status: 1"));
    assert!(error.contains("stdout:\n(empty)"));
    assert!(error.contains("stderr:\nSyntaxError: Unexpected token"));
}

#[test]
fn rejects_successful_config_renderer_output_without_dependency_hash() {
    let result: ConfigRendererOutput = serde_json::from_value(json!({ "ok": true })).unwrap();
    let error = required_config_dependency_hash(&result)
        .unwrap_err()
        .to_string();

    assert!(error.contains("without dependencyHash"));
}

#[test]
fn parses_dependency_major_versions() {
    assert_eq!(major_version("^19.0.0"), Some(19));
    assert_eq!(major_version("~18.3.1"), Some(18));
    assert_eq!(major_version("workspace:*"), None);
}

#[test]
fn detects_react_version_compatibility() {
    let package = json!({
        "dependencies": {
            "react": "^19.0.0",
            "react-dom": "^19.1.0"
        }
    });

    assert_eq!(react_compatibility(&package), "ok (major 19)");
}

#[test]
fn lists_every_ruvyxa_package_across_dependency_sections() {
    let package = json!({
        "dependencies": { "ruvyxa": "^1.0.27", "react": "^19.0.0" },
        "devDependencies": { "@ruvyxa/adapter-node": "^1.0.27" },
        "peerDependencies": { "@ruvyxa/realtime": "^1.0.27" }
    });

    assert_eq!(
        ruvyxa_dependencies(&package),
        vec![
            ("@ruvyxa/adapter-node".to_string(), "^1.0.27".to_string()),
            ("@ruvyxa/realtime".to_string(), "^1.0.27".to_string()),
            ("ruvyxa".to_string(), "^1.0.27".to_string()),
        ]
    );
}

#[test]
fn compares_the_npm_package_against_the_cli_binary() {
    assert_eq!(cli_version_match(Some("^1.0.27"), "1.0.27"), "ok (1.0.27)");
    assert_eq!(
        cli_version_match(Some("^1.0.20"), "1.0.27"),
        "ok (package 1.0.20, cli 1.0.27)"
    );
    assert_eq!(
        cli_version_match(Some("^0.9.0"), "1.0.27"),
        "drift: package 0.9.0, cli 1.0.27"
    );
    assert_eq!(cli_version_match(None, "1.0.27"), "missing");
}

#[test]
fn local_dependency_protocols_are_not_reported_as_version_drift() {
    // The framework's own repository depends on itself through the workspace
    // protocol; there is no published version to compare against.
    for spec in [
        "workspace:*",
        "workspace:^",
        "link:../ruvyxa",
        "file:../pkg",
    ] {
        assert!(
            cli_version_match(Some(spec), "1.0.27").starts_with("ok "),
            "{spec} should not read as drift"
        );
    }
    assert!(cli_version_match(Some("*"), "1.0.27").starts_with("ok "));
    assert!(cli_version_match(Some("latest"), "1.0.27").starts_with("ok "));
}

#[test]
fn detects_duplicate_dependency_versions() {
    let package = json!({
        "dependencies": {
            "react": "^19.0.0"
        },
        "devDependencies": {
            "react": "^18.0.0"
        }
    });

    assert_eq!(
        duplicate_dependencies(&package),
        vec!["react (^19.0.0, ^18.0.0)"]
    );
}

#[test]
fn summarizes_benchmark_samples() {
    let result = summarize_benchmark(
        "sample",
        "deno",
        vec![
            Duration::from_millis(30),
            Duration::from_millis(10),
            Duration::from_millis(20),
        ],
    );

    assert_eq!(result.name, "sample");
    assert_eq!(result.samples, 3);
    assert_eq!(result.runtime, "deno");
    assert_eq!(result.sample_ms, vec![10.0, 20.0, 30.0]);
    assert_eq!(result.min_ms, 10.0);
    assert_eq!(result.median_ms, 20.0);
    assert_eq!(result.max_ms, 30.0);
    assert_eq!(result.p95_ms, 30.0);
}

#[test]
fn caps_build_parallelism_to_available_work() {
    assert_eq!(build_parallelism(Some(0), 4), 1);
    assert_eq!(build_parallelism(Some(3), 1), 1);
    assert_eq!(build_parallelism(Some(3), 5), 3);
    assert_eq!(build_parallelism(Some(usize::MAX), 2), 2);
}

#[test]
fn caps_default_prerender_parallelism_to_limit_and_available_work() {
    assert_eq!(prerender_parallelism(None, 1), 1);
    assert!(prerender_parallelism(None, 10) <= MAX_PRERENDER_PARALLELISM);
    assert_eq!(prerender_parallelism(Some(3), 2), 2);
    // An explicit configuration may exceed the default cap, up to the
    // worker pool limit.
    assert_eq!(prerender_parallelism(Some(3), 10), 3);
    assert_eq!(
        prerender_parallelism(Some(64), 32),
        MAX_CONFIGURED_PRERENDER_PARALLELISM
    );
}

#[test]
fn content_hash_is_deterministic() {
    assert_eq!(
        content_hash("console.log('a')"),
        content_hash("console.log('a')")
    );
    assert_ne!(
        content_hash("console.log('a')"),
        content_hash("console.log('b')")
    );
    assert_eq!(content_hash("console.log('a')").len(), 64);
    assert_eq!(ASSET_HASH_ALGORITHM, "blake3-256");
    assert_eq!(content_hash("metadata-check").len() * 4, 256);
}

#[test]
fn stable_process_environment_excludes_tooling_session_noise() {
    assert!(!is_stable_process_env_key("Path"));
    assert!(!is_stable_process_env_key("POSH_SESSION_ID"));
    assert!(!is_stable_process_env_key("CARGO_MANIFEST_DIR"));
    assert!(!is_stable_process_env_key("CODEX_THREAD_ID"));
    assert!(is_stable_process_env_key("NODE_ENV"));
    assert!(is_stable_process_env_key("DATABASE_URL"));
}

#[test]
fn artifact_fingerprints_are_shared_by_canonical_file_path() {
    let temp = tempfile::tempdir().unwrap();
    let shared = temp.path().join("shared.ts");
    fs::write(&shared, b"export const value = '\xF0\x9F\x9A\x80';").unwrap();
    let cache = ArtifactFingerprintCache::default();

    let first = cache.fingerprint(&shared).unwrap();
    let second = cache.fingerprint(&shared).unwrap();

    assert_eq!(
        first,
        content_hash_bytes(b"export const value = '\xF0\x9F\x9A\x80';")
    );
    assert_eq!(second, first);
    assert_eq!(cache.entry_count(), 1);
}

#[test]
fn stable_prerender_inputs_resolve_project_relative_worker_paths() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let page = app.join("page.tsx");
    fs::create_dir_all(&app).unwrap();
    fs::write(&page, "export default 1").unwrap();

    let inputs = stable_prerender_inputs(temp.path(), &app, &[PathBuf::from("app/page.tsx")]);

    assert_eq!(
        inputs,
        vec![ruvyxa_diagnostics::normalized_canonical_path(&page)]
    );
}

#[test]
fn prerender_artifact_cache_reuses_and_invalidates_dependency_content() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("page.tsx");
    fs::write(&source, "export default () => 'first'").unwrap();
    let job = PrerenderJob {
        route_path: "/cached".to_string(),
        render_path: "/cached".to_string(),
        params: RouteParams::new(),
        strategy: RenderStrategy::Ssg,
        revalidate: None,
        kind: PrerenderJobKind::Render {
            route_file: source.clone(),
            mode: "full",
        },
    };
    let cache = PrerenderArtifactCache {
        directory: temp.path().join("cache"),
        dependency_hash: "config-v1".to_string(),
        render_context_hash: "context-v1".to_string(),
        fingerprints: Arc::new(ArtifactFingerprintCache::default()),
        enabled: true,
    };

    store_prerender_artifact(
        &cache,
        &job,
        "renderer-v1",
        std::slice::from_ref(&source),
        "<main>first</main>",
    );
    assert_eq!(
        load_prerender_artifact(&cache, &job).as_deref(),
        Some("<main>first</main>")
    );

    fs::write(&source, "export default () => 'second'").unwrap();
    let next_build_cache = PrerenderArtifactCache {
        fingerprints: Arc::new(ArtifactFingerprintCache::default()),
        ..cache
    };
    assert!(load_prerender_artifact(&next_build_cache, &job).is_none());
}

#[test]
fn dev_config_respects_overlay_and_trace_flags() {
    let args = ServerArgs {
        root: PathBuf::from("."),
        host: None,
        port: None,
        runtime: None,
    };
    let enabled: ProjectConfig = serde_json::from_value(json!({
        "debug": { "overlay": true, "traces": true }
    }))
    .unwrap();
    let disabled: ProjectConfig = serde_json::from_value(json!({
        "debug": { "overlay": false, "traces": false }
    }))
    .unwrap();

    let enabled = dev_server_config(&args, &enabled).unwrap();
    let disabled = dev_server_config(&args, &disabled).unwrap();
    assert!(enabled.error_overlay);
    assert!(enabled.debug_traces);
    assert!(!disabled.error_overlay);
    assert!(!disabled.debug_traces);
}

#[test]
fn sanitized_markdown_config_enables_the_native_content_bridge() {
    let enabled: ProjectConfig = serde_json::from_value(json!({ "markdown": true })).unwrap();
    let disabled: ProjectConfig = serde_json::from_value(json!({})).unwrap();

    assert!(enabled.markdown_enabled());
    assert!(!disabled.markdown_enabled());
}

#[test]
fn server_configs_apply_action_security_options() {
    let args = ServerArgs {
        root: PathBuf::from("."),
        host: None,
        port: None,
        runtime: None,
    };
    let config: ProjectConfig = serde_json::from_value(json!({
        "build": { "jsx": "classic" },
        "security": {
            "actionLimit": 8192,
            "apiLimit": 16384,
            "pluginLimit": 32768,
            "actionRateLimit": { "max": 240, "window": 30 },
            "sameOrigin": false,
            "fetchMeta": false,
            "trustedProxyIps": ["10.0.0.2", "2001:db8::2", "172.16.0.0/12"],
            "headers": false
        }
    }))
    .unwrap();

    for server in [
        dev_server_config(&args, &config).unwrap(),
        production_server_config(&args, &config).unwrap(),
    ] {
        assert_eq!(server.action_body_limit_bytes, 8192);
        assert_eq!(server.api_body_limit_bytes, 16384);
        assert_eq!(server.plugin_response_body_limit_bytes, 32768);
        assert_eq!(server.action_rate_limit_max, 240);
        assert_eq!(server.action_rate_limit_window, Duration::from_secs(30));
        assert!(!server.same_origin_actions);
        assert!(!server.fetch_metadata_actions);
        assert_eq!(
            server.trusted_proxies,
            TrustedProxies::parse_all(["10.0.0.2", "2001:db8::2", "172.16.0.0/12"]).unwrap(),
            "exact addresses and CIDR ranges must both reach the server"
        );
        assert!(!server.security_headers);
        assert!(matches!(
            server.jsx_runtime,
            ruvyxa_bundler::JsxRuntime::Classic
        ));
    }
}

#[test]
fn rejects_unknown_rust_config_fields() {
    let error = serde_json::from_value::<ProjectConfig>(json!({
        "debug": { "overlay": true, "unsupported": true }
    }))
    .unwrap_err();
    assert!(error.to_string().contains("unknown field `unsupported`"));

    let error = serde_json::from_value::<ProjectConfig>(json!({
        "unsupportedTopLevel": true
    }))
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown field `unsupportedTopLevel`")
    );
}

#[test]
fn rejects_zero_security_limits() {
    let config: ProjectConfig = serde_json::from_value(json!({
        "security": {
            "pluginLimit": 0
        }
    }))
    .unwrap();

    let error = config.validate_paths().unwrap_err();
    assert!(error.to_string().contains("security.pluginLimit"));
}

#[test]
fn rejects_security_limits_above_hard_ceiling() {
    let config: ProjectConfig = serde_json::from_value(json!({
        "security": {
            "actionLimit": MAX_ACTION_BODY_LIMIT_BYTES + 1,
            "apiLimit": MAX_API_BODY_LIMIT_BYTES + 1,
            "actionRateLimit": {
                "max": MAX_ACTION_RATE_LIMIT_REQUESTS + 1,
                "window": MAX_ACTION_RATE_LIMIT_WINDOW_SECS + 1
            }
        }
    }))
    .unwrap();

    let error = config.validate_paths().unwrap_err();
    assert!(error.to_string().contains("security.actionLimit"));
}

#[test]
fn rejects_invalid_trusted_proxy_ips() {
    for value in ["not-an-ip", "10.0.0.0/33", "10.0.0.0/"] {
        let config: ProjectConfig = serde_json::from_value(json!({
            "security": { "trustedProxyIps": [value] }
        }))
        .unwrap();

        let error = config.validate_paths().unwrap_err();
        assert!(
            error.to_string().contains("security.trustedProxyIps"),
            "{value} should be rejected by name, got: {error}"
        );
        assert!(error.to_string().contains(value), "{error}");
    }
}

/// The exact configuration the server-actions guide documents. It used to
/// fail `validate_paths` with `RUV1602`, so following the documentation
/// prevented the CLI from starting at all.
#[test]
fn accepts_documented_cidr_trusted_proxy_ranges() {
    let config: ProjectConfig = serde_json::from_value(json!({
        "security": { "trustedProxyIps": ["10.0.0.0/8", "172.16.0.0/12"] }
    }))
    .unwrap();

    config
        .validate_paths()
        .expect("documented CIDR ranges must be accepted");
}

#[test]
fn rejects_excessive_plugin_response_limit() {
    let accepted: ProjectConfig = serde_json::from_value(json!({
        "security": {
            "pluginLimit": MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES
        }
    }))
    .unwrap();
    assert!(accepted.validate_paths().is_ok());

    let config: ProjectConfig = serde_json::from_value(json!({
        "security": {
            "pluginLimit": MAX_PLUGIN_RESPONSE_BODY_LIMIT_BYTES + 1
        }
    }))
    .unwrap();

    let error = config.validate_paths().unwrap_err();
    assert!(error.to_string().contains("must not exceed"));
}

#[test]
fn parses_ruvyxa_bundler_build_options() {
    assert!(matches!(
        parse_jsx_runtime(None).unwrap(),
        ruvyxa_bundler::JsxRuntime::Automatic
    ));
    assert!(matches!(
        parse_jsx_runtime(Some("automatic")).unwrap(),
        ruvyxa_bundler::JsxRuntime::Automatic
    ));
    assert!(matches!(
        parse_split_strategy(Some("route")).unwrap(),
        ruvyxa_bundler::SplitStrategy::Route
    ));
    assert!(matches!(
        parse_split_strategy(Some("manual")).unwrap(),
        ruvyxa_bundler::SplitStrategy::Single
    ));

    let config: BuildConfigOptions = serde_json::from_value(json!({
        "treeShake": false,
        "manifest": true,
        "warm": false
        ,"prerenderCache": false
    }))
    .unwrap();
    assert_eq!(config.tree_shaking, Some(false));
    assert_eq!(config.emit_chunk_manifest, Some(true));
    assert_eq!(config.prebundle_dependencies, Some(false));
    assert_eq!(config.prerender_cache, Some(false));
}

#[test]
fn parses_js_build_plugin_metadata() {
    let config: ProjectConfig = serde_json::from_value(json!({
        "plugins": [
            {
                "name": "banner"
            }
        ]
    }))
    .unwrap();

    assert_eq!(config.plugins.len(), 1);
    assert_eq!(config.plugins[0].name, "banner");

    let manifest = build_plugin_manifest(&config.plugins);
    assert_eq!(manifest[0]["name"], "banner");
    assert_eq!(manifest[0].as_object().unwrap().len(), 1);
}

#[test]
fn parses_global_rendering_defaults() {
    let config: ProjectConfig = serde_json::from_value(json!({
        "render": {
            "strategy": "isr",
            "revalidate": 90
        }
    }))
    .unwrap();

    assert_eq!(config.rendering.default_strategy, Some(RenderStrategy::Isr));
    assert_eq!(config.rendering.default_revalidate, Some(90));
}

#[test]
fn resolves_shared_build_cache_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("project");
    let shared = temp.path().join("shared-cache");

    assert_eq!(
        resolve_build_cache_dir(&root, Some(".cache/build"), None),
        root.join(".cache/build")
    );
    assert_eq!(
        resolve_build_cache_dir(
            &root,
            Some("ignored"),
            Some(shared.clone().into_os_string())
        ),
        shared
    );
    assert_eq!(
        resolve_build_cache_dir(&root, None, None),
        root.join(".ruvyxa/cache/bundler")
    );
}

#[test]
fn rejects_invalid_ruvyxa_bundler_build_options() {
    assert!(parse_jsx_runtime(Some("runtime-x")).is_err());
    assert!(parse_split_strategy(Some("vendor")).is_err());
}

/// `build.target` selects no transform, so it must stay *accepted* rather than
/// validated or rejected. `deny_unknown_fields` means deleting the key would
/// hard-fail every existing config that sets it, and validating it would go
/// back to implying the setting takes effect. Both halves are asserted here so
/// a future edit cannot quietly pick either one.
#[test]
fn build_target_is_accepted_and_reaches_no_bundle_option() {
    let build: BuildConfigOptions =
        serde_json::from_str(r#"{"target":"es2018","jsx":"automatic"}"#).unwrap();
    assert_eq!(
        build._es_target,
        Some(serde_json::Value::String("es2018".to_string()))
    );

    // A value no ECMAScript target list contains is accepted too: an inert key
    // has nothing to validate against.
    let nonsense: BuildConfigOptions = serde_json::from_str(r#"{"target":"es5"}"#).unwrap();
    assert_eq!(
        nonsense._es_target,
        Some(serde_json::Value::String("es5".to_string()))
    );

    // The option struct handed to the bundler carries no target at all, so
    // there is no field for a future change to leave silently unread.
    let options = client_bundle_options(&build).unwrap();
    let encoded = serde_json::to_value(&options).unwrap();
    assert!(encoded.get("es_target").is_none());
    assert!(encoded.get("esTarget").is_none());
    assert!(encoded.get("target").is_none());
}

#[test]
fn emit_client_bundles_writes_chunk_manifest_when_enabled() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    let client_dir = root.join(".ruvyxa").join("client");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(
        app.join("page.tsx"),
        "export default function Page() { return <main>Home</main>; }",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let build = BuildConfigOptions {
        minify: Some(false),
        sourcemap: Some(false),
        tree_shaking: Some(true),
        split_strategy: Some("route".to_string()),
        parallelism: Some(1),
        jsx_runtime: Some("classic".to_string()),
        _es_target: None,
        emit_chunk_manifest: Some(true),
        prebundle_dependencies: Some(true),
        prerender_cache: Some(true),
    };

    let client_manifest = emit_client_bundles(
        root,
        &app,
        &manifest,
        &client_dir,
        &build,
        &[],
        RuvyxaBuildCache {
            dependency_hash: "no-config",
            directory: &root.join(".ruvyxa/cache/bundler"),
        },
    )
    .unwrap();

    assert!(client_dir.join("chunk-manifest.json").is_file());
    assert_eq!(client_manifest["emitChunkManifest"], true);
    assert!(client_manifest["moduleCount"].as_u64().unwrap() > 0);
    assert!(client_manifest["routes"][0]["chunkManifest"].is_object());
}

#[test]
fn client_manifest_attaches_shared_chunks_to_affected_routes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    let client_dir = root.join("client");
    std::fs::create_dir_all(app.join("about")).unwrap();
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(app.join("shared.ts"), "export const label = 'shared'").unwrap();
    std::fs::write(
        app.join("layout.tsx"),
        "import { label } from './shared';\nexport default function Layout({ children }) { return <section data-label={label}>{children}</section> }",
    )
    .unwrap();
    std::fs::write(
        app.join("page.tsx"),
        "export default function Page() { return <main>Home</main> }",
    )
    .unwrap();
    std::fs::write(
        app.join("about/page.tsx"),
        "export default function About() { return <main>About</main> }",
    )
    .unwrap();
    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let build = BuildConfigOptions {
        minify: Some(false),
        split_strategy: Some("route".to_string()),
        emit_chunk_manifest: Some(true),
        parallelism: Some(2),
        ..BuildConfigOptions::default()
    };

    let client_manifest = emit_client_bundles(
        root,
        &app,
        &manifest,
        &client_dir,
        &build,
        &[],
        RuvyxaBuildCache {
            dependency_hash: "no-config",
            directory: &root.join(".ruvyxa/cache/bundler"),
        },
    )
    .unwrap();

    for route in client_manifest["routes"].as_array().unwrap() {
        assert_eq!(route["sharedChunks"].as_array().unwrap().len(), 1);
        assert!(
            route["sharedChunks"][0]["src"]
                .as_str()
                .unwrap()
                .starts_with("/__ruvyxa/client/shared.")
        );
        let route_file = route["file"].as_str().unwrap();
        let route_code = std::fs::read_to_string(client_dir.join(route_file)).unwrap();
        assert!(route_code.starts_with("import \"./shared."), "{route_code}");
        assert!(!route_code.contains("const label = "), "{route_code}");
    }
    let expected_order = manifest
        .routes
        .iter()
        .filter(|route| route.kind == ruvyxa_graph::RouteKind::Page)
        .map(|route| route.path.as_str())
        .collect::<Vec<_>>();
    let actual_order = client_manifest["routes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|route| route["path"].as_str())
        .collect::<Vec<_>>();
    assert_eq!(actual_order, expected_order);
    let shared_file = client_manifest["sharedRouteChunks"][0]["file"]
        .as_str()
        .unwrap()
        .to_string();
    let shared_code = std::fs::read_to_string(client_dir.join(&shared_file)).unwrap();
    assert!(
        shared_code.contains("__RUVYXA_SHARED_MODULES__"),
        "{shared_code}"
    );
    assert!(
        shared_code.lines().any(|line| {
            let line = line.trim();
            line.starts_with("const label = ") && line.contains("shared")
        }),
        "{shared_code}"
    );

    let plan_dir = root.join(".ruvyxa/cache/bundler/client-route-plans");
    let plan_files = std::fs::read_dir(&plan_dir)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(plan_files.len(), 2);
    let cached_plan: serde_json::Value =
        serde_json::from_slice(&std::fs::read(plan_files[0].path()).unwrap()).unwrap();
    assert!(cached_plan["module_paths"].is_array());
    assert!(cached_plan.get("bundle").is_none());
    let shared_artifact_dir = root.join(".ruvyxa/cache/bundler/shared-route-artifacts");
    assert_eq!(
        std::fs::read_dir(&shared_artifact_dir)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
            .len(),
        1
    );

    let cached_manifest = emit_client_bundles(
        root,
        &app,
        &manifest,
        &client_dir,
        &build,
        &[],
        RuvyxaBuildCache {
            dependency_hash: "no-config",
            directory: &root.join(".ruvyxa/cache/bundler"),
        },
    )
    .unwrap();
    assert!(
        cached_manifest["routes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|route| route["artifactCacheHit"] == true)
    );

    std::fs::write(app.join("shared.ts"), "export const label = 'shared-after'").unwrap();
    let invalidated_manifest = emit_client_bundles(
        root,
        &app,
        &manifest,
        &client_dir,
        &build,
        &[],
        RuvyxaBuildCache {
            dependency_hash: "no-config",
            directory: &root.join(".ruvyxa/cache/bundler"),
        },
    )
    .unwrap();
    assert!(
        invalidated_manifest["routes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|route| route["artifactCacheHit"] == false)
    );
    let invalidated_shared_file = invalidated_manifest["sharedRouteChunks"][0]["file"]
        .as_str()
        .unwrap();
    assert_ne!(invalidated_shared_file, shared_file);
    let invalidated_shared_code =
        std::fs::read_to_string(client_dir.join(invalidated_shared_file)).unwrap();
    assert!(
        invalidated_shared_code.contains("shared-after"),
        "{invalidated_shared_code}"
    );
}

#[test]
fn client_artifact_cache_invalidates_dynamic_import_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    let client_dir = root.join("client");
    let cache_dir = root.join(".ruvyxa/cache/bundler");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(
        app.join("page.tsx"),
        "export default async function Page() { return (await import('./lazy')).label }",
    )
    .unwrap();
    std::fs::write(app.join("lazy.ts"), "export const label = 'before'").unwrap();
    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let build = BuildConfigOptions {
        minify: Some(false),
        split_strategy: Some("route".to_string()),
        emit_chunk_manifest: Some(true),
        parallelism: Some(1),
        ..BuildConfigOptions::default()
    };
    let emit = || {
        emit_client_bundles(
            root,
            &app,
            &manifest,
            &client_dir,
            &build,
            &[],
            RuvyxaBuildCache {
                dependency_hash: "no-config",
                directory: &cache_dir,
            },
        )
        .unwrap()
    };

    let first = emit();
    assert_eq!(first["routes"][0]["artifactCacheHit"], false);
    let warm = emit();
    assert_eq!(warm["routes"][0]["artifactCacheHit"], true);

    std::fs::write(app.join("lazy.ts"), "export const label = 'after'").unwrap();
    let changed = emit();
    assert_eq!(changed["routes"][0]["artifactCacheHit"], false);
    let chunk_file = changed["routes"][0]["chunks"][0]["file"].as_str().unwrap();
    let chunk = std::fs::read_to_string(client_dir.join(chunk_file)).unwrap();
    assert!(chunk.contains("after"), "{chunk}");
}

#[test]
fn prerender_html_includes_hashed_hydration_and_preload_assets() {
    let temp = tempfile::tempdir().unwrap();
    let client_dir = temp.path().join("client");
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(
        client_dir.join("manifest.json"),
        r#"{"routes":[{"path":"/docs/[slug]","src":"/__ruvyxa/client/docs.123.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.456.js"}]}]}"#,
    )
    .unwrap();
    let client_assets = load_prerender_client_assets(&client_dir);
    assert_eq!(client_assets.len(), 1);

    let html = inject_prerender_client_assets(
        "<!doctype html><html><head><title>Docs</title></head><body><main>Guide</main></body></html>",
        &client_assets,
        "/docs/[slug]",
        "/docs/start",
        &BTreeMap::from([("slug".to_string(), serde_json::json!("start"))]),
    );

    assert!(html.contains(r#"<link rel="modulepreload" href="/__ruvyxa/client/shared.456.js">"#));
    assert!(html.contains(r#"<script type="module" src="/__ruvyxa/client/docs.123.js"></script>"#));
    // A data block, not an executable script: every page carries this, so an
    // inline assignment here is what a strict `script-src` blocks.
    assert!(html.contains(
        r#"<script type="application/json" id="__ruvyxa-bootstrap">{"params":{"slug":"start"},"path":"/docs/start"}</script>"#
    ));
    assert!(!html.contains("globalThis.__RUVYXA_ROUTE_PARAMS__ ="));
    assert!(html.find("modulepreload").unwrap() < html.find("</head>").unwrap());
    assert!(html.find("docs.123.js").unwrap() < html.find("</body>").unwrap());
}

#[test]
fn prerender_deferred_hydration_loads_bundle_only_through_loader() {
    let temp = tempfile::tempdir().unwrap();
    let client_dir = temp.path().join("client");
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(
        client_dir.join("manifest.json"),
        r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/home.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.js"}],"hydration":"visible","hydrationLoader":"/__ruvyxa/client/hydration.js"}]}"#,
    )
    .unwrap();
    let assets = load_prerender_client_assets(&client_dir);

    let html = inject_prerender_client_assets(
        "<!doctype html><html><head></head><body><main>Home</main></body></html>",
        &assets,
        "/",
        "/",
        &BTreeMap::new(),
    );

    assert!(!html.contains("modulepreload"), "{html}");
    assert!(html.contains("hydration.js?strategy=visible&amp;src=/__ruvyxa/client/home.js"));
    assert!(!html.contains(r#"src="/__ruvyxa/client/home.js""#));
}

#[test]
fn prerender_html_includes_global_styles_in_the_document_head() {
    let html = inject_prerender_styles(
        "<!doctype html><html><head><title>Docs</title></head><body><main>Guide</main></body></html>",
        "body { color: rebeccapurple; }",
    );

    assert!(html.contains(r#"<style data-ruvyxa-css>body { color: rebeccapurple; }</style>"#));
    assert!(html.find("data-ruvyxa-css").unwrap() < html.find("</head>").unwrap());
    assert!(html.contains("<main>Guide</main>"));
}

#[test]
fn native_client_build_applies_js_config_transform_plugin() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    let client_dir = root.join(".ruvyxa").join("client");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(
        app.join("page.tsx"),
        "import { virtualLabel } from 'virtual:label'; export default function Page() { return <main>{virtualLabel} Before</main>; }",
    )
    .unwrap();
    std::fs::write(
        root.join("ruvyxa.config.ts"),
        r#"
import { config } from "ruvyxa/config"
import { definePlugin } from "ruvyxa/plugin"
import path from "node:path"

export default config({
  build: {
minify: false,
map: true,
manifest: true,
  },
  plugins: [definePlugin({
name: "replace-before",
register({ build }) {
  build.onResolve(({ id, root }) =>
    id === "virtual:label" ? path.join(root, "virtual-label.ts") : undefined
  )
  build.onLoad(({ id }) =>
    id.endsWith("virtual-label.ts")
      ? 'export const virtualLabel = "LoadedByPlugin"'
      : undefined
  )
  build.onTransform(({ code, id, environment }) => {
    if (environment !== "client" || !id.endsWith("page.tsx")) return null
    return {
      code: code.replace("Before", "After"),
      map: {
        version: 3,
        sources: ["plugin-original.tsx"],
        sourcesContent: [code],
        names: [],
        mappings: "AAAA",
      },
    }
  })
},
  })],
})
"#,
    )
    .unwrap();

    let config = load_project_config(root).unwrap();
    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let client_manifest = emit_client_bundles(
        root,
        &app,
        &manifest,
        &client_dir,
        &config.build,
        &config.plugins,
        RuvyxaBuildCache {
            dependency_hash: &config.config_dependency_hash,
            directory: &build_cache_dir(root, &config.cache),
        },
    )
    .unwrap();
    let route_file = client_manifest["routes"][0]["file"].as_str().unwrap();
    let output = std::fs::read_to_string(client_dir.join(route_file)).unwrap();

    assert!(output.contains("After"), "{output}");
    assert!(output.contains("LoadedByPlugin"), "{output}");
    assert!(!output.contains("Before"), "{output}");
    assert_eq!(client_manifest["plugins"][0]["name"], "replace-before");
    let source_map_file = client_manifest["routes"][0]["sourceMap"].as_str().unwrap();
    let source_map: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(client_dir.join(source_map_file)).unwrap())
            .unwrap();
    assert!(
        source_map["sources"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source.as_str() == Some("plugin-original.tsx"))
    );
}

#[test]
fn imported_plugin_change_invalidates_compile_cache_without_clean() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    let client_dir = root.join(".ruvyxa").join("client");
    let plugin_file = root.join("build-plugin.ts");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(
        app.join("page.tsx"),
        "export default function Page() { return <main>Before</main>; }",
    )
    .unwrap();
    std::fs::write(
        root.join("ruvyxa.config.ts"),
        r#"
import { plugin } from "./build-plugin.js"
export default { build: { minify: false }, plugins: [plugin] }
"#,
    )
    .unwrap();

    let write_plugin = |replacement: &str| {
        std::fs::write(
            &plugin_file,
            format!(
                r#"import {{ definePlugin }} from "ruvyxa/plugin"
export const plugin = definePlugin({{
  name: "replace-label",
  register({{ build }}) {{
build.onTransform(({{ code, id }}) => {{
  if (!id.endsWith("page.tsx")) return null
return {{ code: code.replace("Before", "{replacement}") }}
}})
  }}
}})
"#
            ),
        )
        .unwrap();
    };

    write_plugin("FirstBuild");
    let first_config = load_project_config(root).unwrap();
    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let cache_dir = build_cache_dir(root, &first_config.cache);
    let first_manifest = emit_client_bundles(
        root,
        &app,
        &manifest,
        &client_dir,
        &first_config.build,
        &first_config.plugins,
        RuvyxaBuildCache {
            dependency_hash: &first_config.config_dependency_hash,
            directory: &cache_dir,
        },
    )
    .unwrap();
    let first_file = first_manifest["routes"][0]["file"].as_str().unwrap();
    let first_output = std::fs::read_to_string(client_dir.join(first_file)).unwrap();

    write_plugin("SecondRun");
    let second_config = load_project_config(root).unwrap();
    assert_ne!(
        first_config.config_dependency_hash,
        second_config.config_dependency_hash
    );
    let second_manifest = emit_client_bundles(
        root,
        &app,
        &manifest,
        &client_dir,
        &second_config.build,
        &second_config.plugins,
        RuvyxaBuildCache {
            dependency_hash: &second_config.config_dependency_hash,
            directory: &cache_dir,
        },
    )
    .unwrap();
    let second_file = second_manifest["routes"][0]["file"].as_str().unwrap();
    let second_output = std::fs::read_to_string(client_dir.join(second_file)).unwrap();

    assert!(first_output.contains("FirstBuild"), "{first_output}");
    assert!(second_output.contains("SecondRun"), "{second_output}");
    assert!(!second_output.contains("FirstBuild"), "{second_output}");
}

#[test]
fn native_client_build_compiles_mdx_with_configured_unified_plugins() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    let client_dir = root.join(".ruvyxa").join("client");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(app.join("page.mdx"), "# Original heading").unwrap();
    std::fs::write(
        root.join("ruvyxa.config.ts"),
        r#"
import { config } from "ruvyxa/config"
function remarkConfigured() {
  return (tree) => {
    for (const node of tree.children ?? []) {
      for (const child of node.children ?? []) {
        if (child.type === "text") child.value = child.value.replace("Original", "Configured")
      }
    }
  }
}
export default config({
  build: { minify: false, map: true, manifest: true },
  markdown: { remarkPlugins: [remarkConfigured] },
})
"#,
    )
    .unwrap();

    let config = load_project_config(root).unwrap();
    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let session = TypeScriptPluginBuildSession::new(
        root,
        &config.plugins,
        config.javascript_runtime(),
        config.markdown_enabled(),
        config.react_compiler.unwrap_or(false),
    )
    .unwrap();
    let client_manifest = emit_client_bundles_with_session(
        root,
        &app,
        &manifest,
        &client_dir,
        &config.build,
        &config.plugins,
        RuvyxaBuildCache {
            dependency_hash: &config.config_dependency_hash,
            directory: &build_cache_dir(root, &config.cache),
        },
        &session,
    )
    .unwrap();
    let route_file = client_manifest["routes"][0]["file"].as_str().unwrap();
    let output = std::fs::read_to_string(client_dir.join(route_file)).unwrap();

    assert!(output.contains("Configured heading"), "{output}");
    assert!(!output.contains("Original heading"), "{output}");
    assert!(output.contains("ruvyxa-content"), "{output}");
}

#[test]
fn typescript_plugin_bridge_reuses_worker_state() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("ruvyxa.config.mjs"),
        r#"
import { definePlugin } from "ruvyxa/plugin"
let calls = 0
export default {
  plugins: [definePlugin({
name: "counter",
register({ build }) {
  build.onTransform(({ code }) => {
    calls += 1
    return {
      code: `${code}\nexport const pluginCall = ${calls}`,
      map: {
        version: 3,
        sources: ["counter-input.ts"],
        sourcesContent: [code],
        names: [],
        mappings: "AAAA",
      },
    }
  })
},
  })],
}
"#,
    )
    .unwrap();

    let runner = find_runtime_script(root, "plugin-runtime.mjs").unwrap();
    let bridge = TypeScriptPluginBridge {
        project_root: root.to_path_buf(),
        workers: Arc::new(vec![Mutex::new(
            TypeScriptPluginWorker::spawn(&runner, root, JavaScriptRuntime::Node).unwrap(),
        )]),
        next_worker: Arc::new(AtomicUsize::new(0)),
        content_compiler_enabled: false,
    };
    let context = ruvyxa_bundler::hooks::BuildHookContext {
        project_root: root.to_path_buf(),
        importer: None,
        target: ruvyxa_bundler::BundleTarget::Client,
    };

    let first = ruvyxa_bundler::hooks::BuildHooks::transform(
        &bridge,
        "export const value = 1",
        &root.join("first.ts"),
        &context,
    )
    .unwrap()
    .unwrap();
    let second = ruvyxa_bundler::hooks::BuildHooks::transform(
        &bridge,
        "export const value = 2",
        &root.join("second.ts"),
        &context,
    )
    .unwrap()
    .unwrap();

    assert!(first.code.contains("pluginCall = 1"));
    assert!(second.code.contains("pluginCall = 2"));
    assert!(second.map.unwrap().contains("counter-input.ts"));
}

#[test]
fn native_content_bridge_runs_configured_mdx_plugins() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("ruvyxa.config.mjs"),
        r#"
function remarkConfigured() {
  return (tree) => {
    for (const node of tree.children ?? []) {
      for (const child of node.children ?? []) {
        if (child.type === "text") child.value = child.value.replace("Original", "Configured")
      }
    }
  }
}
export default { markdown: { remarkPlugins: [remarkConfigured] } }
"#,
    )
    .unwrap();

    let runner = find_runtime_script(root, "plugin-runtime.mjs").unwrap();
    let bridge = TypeScriptPluginBridge {
        project_root: root.to_path_buf(),
        workers: Arc::new(vec![Mutex::new(
            TypeScriptPluginWorker::spawn(&runner, root, JavaScriptRuntime::Node).unwrap(),
        )]),
        next_worker: Arc::new(AtomicUsize::new(0)),
        content_compiler_enabled: true,
    };
    let context = ruvyxa_bundler::hooks::BuildHookContext {
        project_root: root.to_path_buf(),
        importer: None,
        target: ruvyxa_bundler::BundleTarget::Client,
    };

    let compiled = ruvyxa_bundler::hooks::BuildHooks::compile_content(
        &bridge,
        "# Original heading",
        &root.join("page.mdx"),
        &context,
    )
    .unwrap()
    .unwrap();

    assert!(compiled.code.contains("Configured heading"));
    assert!(compiled.code.contains("ruvyxa-content"));
}

#[test]
fn typescript_plugin_build_complete_runs_after_output_commit() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let out_dir = root.join(".ruvyxa");
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::write(
        root.join("ruvyxa.config.mjs"),
        r#"
import { definePlugin } from "ruvyxa/plugin"
export default {
  plugins: [definePlugin({
name: "complete",
register({ build }) {
  build.onComplete(async ({ outDir, manifest }) => {
    await import("node:fs/promises").then(({ writeFile }) =>
      writeFile(`${outDir}/plugin-complete.json`, JSON.stringify(manifest)))
  })
},
  })],
}
"#,
    )
    .unwrap();
    let plugins = vec![BuildPluginConfig {
        name: "complete".to_string(),
        head: Vec::new(),
    }];

    let session =
        TypeScriptPluginBuildSession::new(root, &plugins, JavaScriptRuntime::Node, false, false)
            .unwrap();
    session
        .run_complete(&out_dir, &serde_json::json!({ "routes": 1 }))
        .unwrap();

    let marker = std::fs::read_to_string(out_dir.join("plugin-complete.json")).unwrap();
    assert!(marker.contains("\"routes\":1"));
}

#[test]
fn typescript_plugin_build_session_reuses_worker_across_lifecycle_hooks() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let out_dir = root.join(".ruvyxa");
    std::fs::create_dir_all(&out_dir).unwrap();
    std::fs::write(
        root.join("ruvyxa.config.mjs"),
        r#"
import { definePlugin } from "ruvyxa/plugin"
let phase = "registered"
export default {
  plugins: [definePlugin({
name: "lifecycle-state",
register({ build }) {
  build.onStart(() => { phase = "started" })
  build.onTransform(({ code }) => {
    const observed = phase
    phase = "transformed"
    return `${code}\nexport const lifecyclePhase = ${JSON.stringify(observed)}`
  })
  build.onComplete(async ({ outDir }) => {
    const { writeFile } = await import("node:fs/promises")
    await writeFile(`${outDir}/plugin-phase.txt`, phase)
  })
},
  })],
}
"#,
    )
    .unwrap();
    let plugins = vec![BuildPluginConfig {
        name: "lifecycle-state".to_string(),
        head: Vec::new(),
    }];
    let session =
        TypeScriptPluginBuildSession::new(root, &plugins, JavaScriptRuntime::Node, false, false)
            .unwrap();

    session.run_start(&out_dir).unwrap();
    let context = ruvyxa_bundler::hooks::BuildHookContext {
        project_root: root.to_path_buf(),
        importer: None,
        target: ruvyxa_bundler::BundleTarget::Client,
    };
    let transformed = ruvyxa_bundler::hooks::BuildHooks::transform(
        session.bridge().unwrap(),
        "export const value = 1",
        &root.join("page.ts"),
        &context,
    )
    .unwrap()
    .unwrap();
    session
        .run_complete(&out_dir, &serde_json::json!({ "routes": 1 }))
        .unwrap();

    assert!(
        transformed.code.contains("lifecyclePhase = \"started\""),
        "{}",
        transformed.code
    );
    assert_eq!(
        std::fs::read_to_string(out_dir.join("plugin-phase.txt")).unwrap(),
        "transformed"
    );
}

#[test]
fn production_session_runs_opt_in_react_compiler_before_oxc() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("ruvyxa.config.mjs"),
        "export default { reactCompiler: true }\n",
    )
    .unwrap();

    let session =
        TypeScriptPluginBuildSession::new(root, &[], JavaScriptRuntime::Node, false, true).unwrap();
    let context = ruvyxa_bundler::hooks::BuildHookContext {
        project_root: root.to_path_buf(),
        importer: None,
        target: ruvyxa_bundler::BundleTarget::Client,
    };
    let transformed = ruvyxa_bundler::hooks::BuildHooks::transform(
        session.bridge().unwrap(),
        "export function Counter({ count }) { return <span>{count}</span> }",
        &root.join("Counter.tsx"),
        &context,
    )
    .unwrap()
    .unwrap();

    assert!(transformed.code.contains("react/compiler-runtime"));
    assert!(transformed.map.is_some());
}

#[test]
fn top_level_help_uses_framework_name_and_command_descriptions() {
    let help = Cli::command().render_long_help().to_string();

    assert!(help.contains("Usage: Ruvyxa <COMMAND>"));
    assert!(!help.contains("Ruvyxa Framework"));
    assert!(!help.contains("+==============================================================+"));
    assert!(!help.contains("build  |  validate  |  serve"));
    assert!(!help.contains("Rust-powered full-stack TypeScript framework"));
    assert!(!help.contains("ruvyxa.exe"));
    assert!(help.contains("dev          Run the development server with hot reload"));
    assert!(help.contains("build        Build the application for production output"));
    assert!(help.contains("check        Run app-level production readiness checks"));
    assert!(help.contains("plugin       Create a publishable plugin package"));
    assert!(help.contains("test:parity  Compare dev/prod routes and smoke-render page routes"));
}

#[test]
fn tui_headers_use_the_shared_fox_branding() {
    assert_eq!(tui_header_title("Build"), "🦊 Ruvyxa Build");
    assert_eq!(tui_header_title("Check"), "🦊 Ruvyxa Check");
    assert_eq!(
        tui_header_title("Benchmark (3 sample(s))"),
        "🦊 Ruvyxa Benchmark (3 sample(s))"
    );
}

#[test]
fn config_paths_must_stay_project_relative() {
    assert!(validate_project_relative_path("outDir", ".ruvyxa").is_ok());
    assert!(validate_project_relative_path("appDir", "src/app").is_ok());
    assert!(validate_project_relative_path("css.entries", "styles/theme.css").is_ok());
    assert!(validate_project_relative_path("outDir", "../outside").is_err());
    assert!(validate_project_relative_path("css.entries", "../outside.css").is_err());
    assert!(validate_project_relative_path("outDir", "/tmp/out").is_err());
    assert!(validate_project_relative_path("appDir", "").is_err());
}

#[test]
fn validates_and_propagates_i18n_routing_config() {
    let config: ProjectConfig = serde_json::from_value(json!({
        "i18n": {
            "locales": ["en", "th", "fr-FR"],
            "defaultLocale": "en",
            "localeParam": "lang"
        }
    }))
    .unwrap();
    config.validate_paths().unwrap();
    let routing = config.discover_options(Path::new("project")).i18n.unwrap();
    assert_eq!(routing.locales, ["en", "th", "fr-FR"]);
    assert_eq!(routing.default_locale, "en");
    assert!(routing.detect_locale);
    assert_eq!(routing.cookie, "RUVYXA_LOCALE");

    let duplicate: ProjectConfig = serde_json::from_value(json!({
        "i18n": { "locales": ["en", "EN"], "defaultLocale": "en" }
    }))
    .unwrap();
    assert!(
        duplicate
            .validate_paths()
            .unwrap_err()
            .to_string()
            .contains("duplicate locale")
    );

    let missing_default: ProjectConfig = serde_json::from_value(json!({
        "i18n": { "locales": ["th"], "defaultLocale": "en" }
    }))
    .unwrap();
    assert!(
        missing_default
            .validate_paths()
            .unwrap_err()
            .to_string()
            .contains("defaultLocale")
    );
}

#[test]
fn locale_only_dynamic_routes_expand_without_static_params_boilerplate() {
    let routing = ruvyxa_graph::I18nRouting {
        locales: vec!["en".into(), "th".into()],
        default_locale: "en".into(),
        locale_param: "lang".into(),
        detect_locale: true,
        cookie: "RUVYXA_LOCALE".into(),
    };
    let paths = locale_static_paths(Some(&routing), "/[lang]/about").unwrap();
    assert_eq!(
        paths
            .iter()
            .map(|path| path.path.as_str())
            .collect::<Vec<_>>(),
        ["/en/about", "/th/about"]
    );
    assert!(locale_static_paths(Some(&routing), "/[lang]/blog/[slug]").is_none());
}

#[test]
fn copies_external_style_sources_into_server_output() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let source = root.join("styles/theme.css");
    let server = root.join("output/server");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, ":root { color-scheme: dark; }").unwrap();

    copy_style_sources(root, &server, std::slice::from_ref(&source)).unwrap();

    assert_eq!(
        std::fs::read_to_string(server.join("styles/theme.css")).unwrap(),
        ":root { color-scheme: dark; }"
    );
}

/// A style collection carries watch inputs as well as stylesheets, and a PostCSS
/// plugin may report a whole directory as one — Tailwind reports the trees it
/// scans for class names that way. Copying a directory as a file failed the
/// whole build with a bare `Access is denied` on Windows.
#[test]
fn skips_directory_watch_inputs_when_copying_style_sources() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let scanned = root.join("app");
    let source = root.join("styles/theme.css");
    let server = root.join("output/server");
    std::fs::create_dir_all(&scanned).unwrap();
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, ":root { color-scheme: dark; }").unwrap();

    copy_style_sources(root, &server, &[scanned, source]).unwrap();

    assert!(server.join("styles/theme.css").is_file());
    assert!(
        !server.join("app").exists(),
        "a directory watch input must not be copied into the server output"
    );
}

#[test]
fn parses_top_level_commands_case_insensitively() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args([
        "Ruvyxa",
        "BUILD",
        "--root",
        "examples/demo",
    ])))
    .unwrap();

    assert!(matches!(cli.command, Command::Build(_)));
}

#[test]
fn parses_adds_scaffold_command_and_rejects_the_old_name() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args(["Ruvyxa", "ADDS", "form"])))
        .expect("adds should parse");

    let Command::Adds(args) = cli.command else {
        panic!("expected adds command");
    };
    assert_eq!(args.templates, vec![AddTemplate::Form]);

    assert!(Cli::try_parse_from(normalized_cli_args(os_args(["Ruvyxa", "add", "form",]))).is_err());
}

#[test]
fn parses_check_command_case_insensitively() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args([
        "Ruvyxa",
        "CHECK",
        "--root",
        "examples/demo",
    ])))
    .unwrap();

    assert!(matches!(cli.command, Command::Check(_)));
}

#[test]
fn parses_value_enums_case_insensitively() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args([
        "Ruvyxa",
        "BUILD",
        "--target",
        "EDGE",
        "--root",
        "examples/demo",
    ])))
    .unwrap();

    let Command::Build(args) = cli.command else {
        panic!("expected build command");
    };
    assert!(matches!(args.target, Some(BuildTarget::Edge)));
}

#[test]
fn parses_deno_as_a_javascript_runtime() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args([
        "Ruvyxa",
        "build",
        "--runtime",
        "DENO",
    ])))
    .unwrap();

    let Command::Build(args) = cli.command else {
        panic!("expected build command");
    };
    assert!(matches!(args.runtime, Some(CliRuntime::Deno)));
}

#[test]
fn parses_analyze_sarif_output_options() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args([
        "Ruvyxa",
        "ANALYZE",
        "--FORMAT",
        "SARIF",
        "--OUTPUT",
        "reports/ruvyxa.sarif",
    ])))
    .unwrap();

    let Command::Analyze(args) = cli.command else {
        panic!("expected analyze command");
    };
    assert_eq!(args.format, AnalyzeFormat::Sarif);
    assert_eq!(args.output, Some(PathBuf::from("reports/ruvyxa.sarif")));
}

#[test]
fn parses_interactive_analyze_html_shorthand() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args([
        "Ruvyxa",
        "ANALYZE",
        "--HTML",
        "--OUTPUT",
        "reports/bundle.html",
    ])))
    .unwrap();

    let Command::Analyze(args) = cli.command else {
        panic!("expected analyze command");
    };
    assert!(args.html);
    assert_eq!(args.format, AnalyzeFormat::Auto);
    assert_eq!(args.output, Some(PathBuf::from("reports/bundle.html")));
}

#[test]
fn parses_routes_json_for_tooling_consumers() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args([
        "Ruvyxa",
        "ROUTES",
        "--JSON",
        "--ROOT",
        "examples/demo",
    ])))
    .unwrap();

    let Command::Routes(args) = cli.command else {
        panic!("expected routes command");
    };
    assert!(args.json);
    assert_eq!(args.root, PathBuf::from("examples/demo"));
}

#[test]
fn parses_long_options_case_insensitively() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args([
        "Ruvyxa",
        "BUILD",
        "--TARGET=EDGE",
        "--ROOT",
        "examples/demo",
    ])))
    .unwrap();

    let Command::Build(args) = cli.command else {
        panic!("expected build command");
    };
    assert!(matches!(args.target, Some(BuildTarget::Edge)));
    assert_eq!(args.root, PathBuf::from("examples/demo"));
}

#[test]
fn parses_command_aliases_case_insensitively() {
    let cli = Cli::try_parse_from(normalized_cli_args(os_args([
        "Ruvyxa",
        "PARITY",
        "--root",
        "examples/demo",
    ])))
    .unwrap();

    assert!(matches!(cli.command, Command::TestParity(_)));
}

#[test]
fn uses_config_runtime_when_the_cli_target_is_omitted() {
    let config = ProjectConfig {
        runtime: Some(BuildTarget::Static),
        ..ProjectConfig::default()
    };

    assert_eq!(config.build_target(None), BuildTarget::Static);
    assert_eq!(config.javascript_runtime(), JavaScriptRuntime::Node);
    assert_eq!(
        config.build_target(Some(BuildTarget::Edge)),
        BuildTarget::Edge
    );
    assert_eq!(
        ProjectConfig::default().build_target(None),
        BuildTarget::Node
    );
    assert_eq!(
        ProjectConfig::default().javascript_runtime(),
        JavaScriptRuntime::detect()
    );
}

#[test]
fn parses_bun_runtime_as_build_and_javascript_runtime() {
    let config: ProjectConfig = serde_json::from_value(serde_json::json!({
        "runtime": "bun"
    }))
    .unwrap();

    assert_eq!(config.build_target(None), BuildTarget::Bun);
    assert_eq!(config.javascript_runtime(), JavaScriptRuntime::Bun);
}

#[test]
fn parses_deno_runtime_as_build_and_javascript_runtime() {
    let config: ProjectConfig = serde_json::from_value(serde_json::json!({
        "runtime": "deno"
    }))
    .unwrap();

    assert_eq!(config.build_target(None), BuildTarget::Deno);
    assert_eq!(config.javascript_runtime(), JavaScriptRuntime::Deno);
}

#[test]
fn normalizes_help_target_command_case() {
    let args = normalized_cli_args(os_args(["Ruvyxa", "HELP", "BUILD"]));

    assert_eq!(args[1], OsString::from("help"));
    assert_eq!(args[2], OsString::from("build"));
}

#[test]
fn normalizes_help_option_case() {
    let args = normalized_cli_args(os_args(["Ruvyxa", "--HELP"]));

    assert_eq!(args[1], OsString::from("--help"));
}

#[test]
fn builds_smoke_paths_for_dynamic_routes() {
    assert_eq!(parity_smoke_path("/"), "/");
    assert_eq!(parity_smoke_path("/blog/[slug]"), "/blog/smoke");
    assert_eq!(parity_smoke_path("/docs/[...path]"), "/docs/smoke/path");
    assert_eq!(parity_smoke_path("/shop/[[...category]]"), "/shop");
}

#[test]
fn staged_build_commit_replaces_outputs_and_preserves_cache_directory() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join(".ruvyxa");
    let cache_dir = out_dir.join("cache").join("bundler");
    let old_server_dir = out_dir.join("server");
    let old_assets_dir = out_dir.join("assets");
    let staging_dir = create_build_staging_dir(&out_dir).unwrap();
    let new_server_dir = staging_dir.join("server");
    let new_client_dir = staging_dir.join("client");

    fs::create_dir_all(&cache_dir).unwrap();
    fs::create_dir_all(&old_server_dir).unwrap();
    fs::create_dir_all(&old_assets_dir).unwrap();
    fs::create_dir_all(&new_server_dir).unwrap();
    fs::create_dir_all(&new_client_dir).unwrap();
    fs::write(cache_dir.join("cached.js"), "compiled").unwrap();
    fs::write(old_server_dir.join("old.js"), "old").unwrap();
    fs::write(old_assets_dir.join("old.txt"), "old").unwrap();
    fs::write(out_dir.join("manifest.json"), "{}").unwrap();
    fs::write(out_dir.join("build.json"), "{}").unwrap();
    fs::write(new_server_dir.join("new.js"), "new").unwrap();
    fs::write(new_client_dir.join("new.js"), "new").unwrap();
    let new_deploy_dir = staging_dir.join("deploy").join("vercel");
    fs::create_dir_all(&new_deploy_dir).unwrap();
    fs::write(new_deploy_dir.join("config.json"), "{}").unwrap();
    fs::write(staging_dir.join("manifest.json"), "{\"routes\":[]}").unwrap();
    fs::write(staging_dir.join("build.json"), "{\"framework\":\"Ruvyxa\"}").unwrap();

    commit_staged_build_outputs(&staging_dir, &out_dir).unwrap();

    assert!(cache_dir.join("cached.js").exists());
    assert!(out_dir.join("server/new.js").exists());
    assert!(out_dir.join("client/new.js").exists());
    assert!(out_dir.join("deploy/vercel/config.json").exists());
    assert!(!out_dir.join("server/old.js").exists());
    assert!(!out_dir.join("assets").exists());
    assert!(out_dir.join("manifest.json").exists());
    assert!(out_dir.join("build.json").exists());
    assert!(!staging_dir.exists());
    assert!(!has_temp_build_dir(&out_dir, ".build-rollback"));
}

#[test]
fn incomplete_build_staging_is_removed_when_its_owner_drops() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join(".ruvyxa");
    let staging_dir = create_build_staging_dir(&out_dir).unwrap();

    {
        let _cleanup = BuildStagingCleanup::new(staging_dir.clone());
        fs::write(staging_dir.join("partial-output.txt"), "incomplete").unwrap();
        assert!(staging_dir.exists());
    }

    assert!(!staging_dir.exists());
    assert!(!has_temp_build_dir(&out_dir, ".build-staging"));
}

#[test]
fn staged_build_commit_removes_old_output_when_staging_omits_it() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join(".ruvyxa");
    let staging_dir = create_build_staging_dir(&out_dir).unwrap();

    fs::create_dir_all(out_dir.join("assets")).unwrap();
    fs::write(out_dir.join("assets/old.txt"), "old").unwrap();
    fs::write(staging_dir.join("manifest.json"), "{}").unwrap();
    fs::write(staging_dir.join("build.json"), "{}").unwrap();

    commit_staged_build_outputs(&staging_dir, &out_dir).unwrap();

    assert!(!out_dir.join("assets").exists());
    assert!(out_dir.join("manifest.json").exists());
}

#[test]
fn static_route_path_preserves_page_params_and_rejects_traversal() {
    let params = BTreeMap::from([("slug".to_string(), serde_json::json!("hello-world"))]);
    assert_eq!(
        static_route_path("/blog/[slug]", &params).unwrap(),
        "/blog/hello-world"
    );

    let unsafe_params =
        BTreeMap::from([("slug".to_string(), serde_json::json!("../manifest.json"))]);
    assert!(static_route_path("/blog/[slug]", &unsafe_params).is_err());
}

#[test]
fn static_route_path_allows_valid_catch_all_segments() {
    let params = BTreeMap::from([("path".to_string(), serde_json::json!(["guides", "routing"]))]);
    assert_eq!(
        static_route_path("/docs/[...path]", &params).unwrap(),
        "/docs/guides/routing"
    );
}

#[test]
fn static_route_path_allows_an_omitted_optional_catch_all() {
    let params = RouteParams::new();
    assert_eq!(
        static_route_path("/shop/[[...path]]", &params).unwrap(),
        "/shop"
    );
}

#[test]
fn static_param_segments_describe_scalar_and_catch_all_routes() {
    let segments = static_param_segments("/[locale]/docs/[[...path]]");
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].name, "locale");
    assert!(!segments[0].catch_all);
    assert!(!segments[0].optional);
    assert_eq!(segments[1].name, "path");
    assert!(segments[1].catch_all);
    assert!(segments[1].optional);
}

fn os_args<const N: usize>(args: [&str; N]) -> Vec<OsString> {
    args.into_iter().map(OsString::from).collect()
}

fn has_temp_build_dir(out_dir: &Path, prefix: &str) -> bool {
    fs::read_dir(out_dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .any(|entry| {
            entry.file_type().is_ok_and(|file_type| file_type.is_dir())
                && entry.file_name().to_string_lossy().starts_with(prefix)
        })
}

// ─── `build --server-only` ────────────────────────────────────────────────────

fn server_only_route(path: &str, kind: ruvyxa_graph::RouteKind) -> ruvyxa_graph::RouteEntry {
    ruvyxa_graph::RouteEntry {
        id: path.to_string(),
        path: path.to_string(),
        kind,
        file: PathBuf::from(format!("app{path}/route.ts")),
        layout_chain: Vec::new(),
        server_modules: Vec::new(),
        client_modules: Vec::new(),
        runtime: ruvyxa_graph::RuntimeTarget::Node,
        render: ruvyxa_graph::RenderMeta::default(),
    }
}

fn server_only_manifest(routes: Vec<ruvyxa_graph::RouteEntry>) -> ruvyxa_graph::RouteManifest {
    ruvyxa_graph::RouteManifest {
        app_dir: PathBuf::from("app"),
        routes,
        i18n: None,
    }
}

#[test]
fn parses_the_server_only_flag_in_every_accepted_spelling() {
    for spelling in ["--server-only", "--SERVER-ONLY", "--server_only"] {
        let cli = Cli::try_parse_from(normalized_cli_args(os_args([
            "Ruvyxa",
            "build",
            spelling,
            "--root",
            "examples/demo",
        ])))
        .unwrap_or_else(|error| panic!("{spelling} should parse: {error}"));

        let Command::Build(args) = cli.command else {
            panic!("expected build command");
        };
        assert!(args.server_only, "{spelling}");
    }

    // The default path must stay exactly what it was before the flag existed.
    let cli = Cli::try_parse_from(normalized_cli_args(os_args(["Ruvyxa", "build"]))).unwrap();
    let Command::Build(args) = cli.command else {
        panic!("expected build command");
    };
    assert!(!args.server_only);
}

#[test]
fn server_only_accepts_long_lived_server_targets_only() {
    for target in [BuildTarget::Node, BuildTarget::Bun, BuildTarget::Deno] {
        assert!(
            server_only_target_diagnostic(target).is_none(),
            "{target:?} should be supported"
        );
    }

    for target in [BuildTarget::Static, BuildTarget::Edge] {
        let diagnostic = server_only_target_diagnostic(target)
            .unwrap_or_else(|| panic!("{target:?} should be rejected"));
        let rendered = diagnostic.to_string();
        assert!(rendered.contains("RUV1211"), "{rendered}");
        assert!(
            rendered.contains(&format!("{target:?}").to_lowercase()),
            "{rendered}"
        );
    }
}

#[test]
fn server_only_rejects_page_routes_and_names_the_first_one() {
    use ruvyxa_graph::RouteKind;

    let api_only = server_only_manifest(vec![
        server_only_route("/api/health", RouteKind::Api),
        server_only_route("/api/users", RouteKind::Api),
    ]);
    assert!(server_only_page_route_diagnostic(&api_only).is_none());

    // Discovery order must not change which path is reported.
    let with_pages = server_only_manifest(vec![
        server_only_route("/api/health", RouteKind::Api),
        server_only_route("/settings", RouteKind::Page),
        server_only_route("/dashboard", RouteKind::Page),
    ]);
    let rendered = server_only_page_route_diagnostic(&with_pages)
        .expect("a page route must fail the build")
        .to_string();
    assert!(rendered.contains("RUV1210"), "{rendered}");
    assert!(rendered.contains("/dashboard"), "{rendered}");
    assert!(
        rendered.contains("1 more page route"),
        "the count of remaining pages should be reported: {rendered}"
    );
    assert!(rendered.contains("app/api/"), "{rendered}");
}

#[test]
fn server_only_compatibility_gate_passes_only_for_api_only_node_builds() {
    use ruvyxa_graph::RouteKind;

    let api_only = server_only_manifest(vec![server_only_route("/api/health", RouteKind::Api)]);
    assert!(ensure_server_only_supported(BuildTarget::Node, &api_only).is_ok());
    assert!(ensure_server_only_supported(BuildTarget::Bun, &api_only).is_ok());
    assert!(ensure_server_only_supported(BuildTarget::Static, &api_only).is_err());

    let with_page = server_only_manifest(vec![server_only_route("/", RouteKind::Page)]);
    assert!(ensure_server_only_supported(BuildTarget::Node, &with_page).is_err());

    // An empty project is not a page project; the gate must not invent a failure.
    let empty = server_only_manifest(Vec::new());
    assert!(ensure_server_only_supported(BuildTarget::Node, &empty).is_ok());
}

/// A plugin worker that never answers must fail the build, not hang it.
///
/// This is the defect the timeout exists for: the hook protocol used to read
/// the response with a blocking `read_line`, so a plugin with an unresolved
/// promise or a blocking loop stalled the whole build with no diagnostic and no
/// way out but killing the CLI.
#[test]
fn a_plugin_hook_that_never_answers_fails_the_build_instead_of_hanging() {
    use crate::plugins::TypeScriptPluginWorker;
    use ruvyxa_dev_server::JavaScriptRuntime;
    use std::time::{Duration, Instant};

    if !JavaScriptRuntime::Node.is_available() {
        eprintln!("skipping: node is not available on this machine");
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let runner = temp.path().join("never-answers.mjs");
    // Reads its request but never writes a response line, and holds the event
    // loop open — exactly how a plugin awaiting a promise that never settles
    // presents to the host.
    std::fs::write(
        &runner,
        "process.stdin.resume(); setInterval(() => {}, 1000);\n",
    )
    .unwrap();

    let mut worker =
        TypeScriptPluginWorker::spawn(&runner, temp.path(), JavaScriptRuntime::Node).unwrap();

    let started = Instant::now();
    let error = worker
        .call_with_timeout(
            &serde_json::json!({ "hook": "build.transform" }),
            Duration::from_millis(400),
        )
        .expect_err("a worker that never answers must not resolve");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "the call must give up on its own budget, took {elapsed:?}"
    );
    let rendered = error.to_string();
    assert!(rendered.contains("RUV1701"), "{rendered}");
    assert!(rendered.contains("did not respond"), "{rendered}");

    // The stalled process is stopped rather than left spinning until the CLI
    // exits, and the dead worker refuses further work instead of pairing a late
    // response with the next request.
    let reused = worker
        .call_with_timeout(
            &serde_json::json!({ "hook": "build.transform" }),
            Duration::from_millis(400),
        )
        .expect_err("a poisoned worker must not accept another hook");
    assert!(
        reused.to_string().contains("earlier hook timed out"),
        "{reused}"
    );
}

/// A worker that exits without answering is reported as an exit, not a timeout.
#[test]
fn a_plugin_worker_that_exits_without_answering_is_reported_immediately() {
    use crate::plugins::TypeScriptPluginWorker;
    use ruvyxa_dev_server::JavaScriptRuntime;
    use std::time::{Duration, Instant};

    if !JavaScriptRuntime::Node.is_available() {
        eprintln!("skipping: node is not available on this machine");
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let runner = temp.path().join("exits.mjs");
    std::fs::write(&runner, "process.exit(3);\n").unwrap();

    let mut worker =
        TypeScriptPluginWorker::spawn(&runner, temp.path(), JavaScriptRuntime::Node).unwrap();

    // Generous budget: reaching it would mean the exit was detected by timing
    // out rather than by the closed pipe, which is the distinction under test.
    let started = Instant::now();
    let error = worker
        .call_with_timeout(
            &serde_json::json!({ "hook": "build.transform" }),
            Duration::from_secs(20),
        )
        .expect_err("a worker that exited cannot answer");

    assert!(
        started.elapsed() < Duration::from_secs(10),
        "an exited worker must be detected by EOF, not by the timeout"
    );
    assert!(
        error.to_string().contains("exited before responding"),
        "{error}"
    );
}

/// The config cache must notice every input the renderer told it about.
///
/// Rendering `ruvyxa.config.ts` costs a JavaScript runtime start plus a bundle
/// recompile on every command, so it is cached — which is only safe while the
/// validity check is exact. These cases pin the parts that are easy to get
/// subtly wrong and impossible to notice: a stale config produces a correct
/// looking build against the wrong settings.
#[test]
fn the_config_cache_is_reused_only_while_every_recorded_input_still_holds() {
    use crate::runtime_config::{CONFIG_CACHE_VERSION, ConfigLoadCache, config_cache_is_current};
    use ruvyxa_dev_server::JavaScriptRuntime;
    use std::collections::BTreeMap;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("ruvyxa.config.ts"), "export default {}").unwrap();
    let config_hash = blake3::hash(b"export default {}").to_hex().to_string();

    let base = |mutate: &dyn Fn(&mut ConfigLoadCache)| {
        let mut cache = ConfigLoadCache {
            version: CONFIG_CACHE_VERSION,
            runtime: JavaScriptRuntime::Node.command().to_string(),
            renderer_fingerprint: "renderer-v1".to_string(),
            inputs: BTreeMap::from([("ruvyxa.config.ts".to_string(), config_hash.clone())]),
            env: BTreeMap::new(),
            stdout: "{}".to_string(),
        };
        mutate(&mut cache);
        cache
    };
    let is_current = |cache: &ConfigLoadCache| {
        config_cache_is_current(cache, root, JavaScriptRuntime::Node, "renderer-v1")
    };

    assert!(is_current(&base(&|_| {})), "an untouched project is a hit");

    // Each of these invalidates on its own.
    assert!(
        !is_current(&base(&|cache| cache.version += 1)),
        "a cache written by another format must be discarded, not reinterpreted"
    );
    assert!(
        !is_current(&base(&|cache| cache.runtime = "bun".to_string())),
        "a config may branch on the runtime, so a Node result cannot answer for Bun"
    );
    assert!(
        !is_current(&base(&|cache| {
            cache.renderer_fingerprint = "renderer-v2".to_string()
        })),
        "upgrading the ruvyxa package must not replay results from the old renderer"
    );
    assert!(
        !is_current(&base(&|cache| {
            cache
                .inputs
                .insert("ruvyxa.config.ts".to_string(), "different".to_string());
        })),
        "an edited input must invalidate"
    );
    assert!(
        !is_current(&base(&|cache| {
            cache
                .inputs
                .insert("plugins/index.ts".to_string(), "any".to_string());
        })),
        "a recorded input that no longer exists must invalidate"
    );
    assert!(
        !is_current(&base(&|cache| cache.inputs.clear())),
        "a cache that claims no inputs can never be shown to be current"
    );
}

/// A config that reads an environment variable must re-render when it changes.
///
/// This is what makes caching safe for `process.env.NODE_ENV ? ... : ...`
/// configs: the renderer reports which variables it actually read, and only
/// those pin the result. A variable read while unset is recorded too, so the
/// config also re-renders when the variable first appears.
#[test]
fn the_config_cache_tracks_only_the_environment_the_config_actually_read() {
    use crate::runtime_config::{CONFIG_CACHE_VERSION, ConfigLoadCache, config_cache_is_current};
    use ruvyxa_dev_server::JavaScriptRuntime;
    use std::collections::BTreeMap;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("ruvyxa.config.ts"), "export default {}").unwrap();
    let inputs = BTreeMap::from([(
        "ruvyxa.config.ts".to_string(),
        blake3::hash(b"export default {}").to_hex().to_string(),
    )]);

    let cache = |env: BTreeMap<String, Option<String>>| ConfigLoadCache {
        version: CONFIG_CACHE_VERSION,
        runtime: JavaScriptRuntime::Node.command().to_string(),
        renderer_fingerprint: "renderer-v1".to_string(),
        inputs: inputs.clone(),
        env,
        stdout: "{}".to_string(),
    };
    let is_current = |cache: &ConfigLoadCache| {
        config_cache_is_current(cache, root, JavaScriptRuntime::Node, "renderer-v1")
    };

    // A uniquely named variable keeps this test independent of the ambient
    // environment and of other tests running in the same process.
    let key = "RUVYXA_TEST_CONFIG_CACHE_ENV";
    unsafe { std::env::remove_var(key) };

    assert!(
        is_current(&cache(BTreeMap::from([(key.to_string(), None)]))),
        "a variable recorded as unset, and still unset, is a hit"
    );

    unsafe { std::env::set_var(key, "1") };
    assert!(
        !is_current(&cache(BTreeMap::from([(key.to_string(), None)]))),
        "a variable that was read while unset must invalidate once it appears"
    );
    assert!(
        is_current(&cache(BTreeMap::from([(
            key.to_string(),
            Some("1".to_string())
        )]))),
        "an unchanged value is a hit"
    );
    assert!(
        !is_current(&cache(BTreeMap::from([(
            key.to_string(),
            Some("2".to_string())
        )]))),
        "a changed value must invalidate"
    );

    // A config that never read this variable must not be pinned to it.
    assert!(
        is_current(&cache(BTreeMap::new())),
        "an unread variable must not invalidate anything"
    );

    unsafe { std::env::remove_var(key) };
}
