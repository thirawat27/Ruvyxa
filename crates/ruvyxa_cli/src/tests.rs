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
    // Workers Builds is not Pages, and sets its own variable: a build there saw
    // no platform at all and fell back to whatever the project had configured.
    assert_eq!(
        detect_platform_adapter(env(&[("WORKERS_CI", "1")])),
        Some(("cloudflare".to_string(), "WORKERS_CI".to_string()))
    );
    assert_eq!(
        detect_platform_adapter(env(&[("DENO_DEPLOY", "true")])),
        Some(("deno".to_string(), "DENO_DEPLOY".to_string()))
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
fn the_bind_address_prefers_the_flag_then_the_environment_then_the_config() {
    let env = |vars: &'static [(&'static str, &'static str)]| {
        move |key: &str| {
            vars.iter()
                .find(|(name, _)| *name == key)
                .map(|(_, value)| value.to_string())
        }
    };
    let args = |host: Option<&str>, port: Option<u16>| ServerArgs {
        root: PathBuf::from("."),
        host: host.map(str::to_string),
        port,
        runtime: None,
    };
    let empty = ProjectConfig::default();
    let configured: ProjectConfig =
        serde_json::from_value(json!({ "server": { "host": "127.0.0.1", "port": 4100 } })).unwrap();

    // Nothing set: each command's own default.
    assert_eq!(
        resolve_bind_address(&args(None, None), &empty, env(&[]), DEV_DEFAULT_HOST).unwrap(),
        ("localhost".to_string(), DEFAULT_PORT)
    );
    assert_eq!(
        resolve_bind_address(&args(None, None), &empty, env(&[]), PRODUCTION_DEFAULT_HOST).unwrap(),
        ("0.0.0.0".to_string(), DEFAULT_PORT)
    );

    // The platform's variables beat a committed config file, which is the whole
    // point: a container injects PORT and the repository cannot know it.
    assert_eq!(
        resolve_bind_address(
            &args(None, None),
            &configured,
            env(&[("PORT", "8080"), ("HOST", "0.0.0.0")]),
            DEV_DEFAULT_HOST,
        )
        .unwrap(),
        ("0.0.0.0".to_string(), 8080)
    );

    // An explicit flag beats the environment.
    assert_eq!(
        resolve_bind_address(
            &args(Some("::1"), Some(5000)),
            &configured,
            env(&[("PORT", "8080"), ("HOST", "0.0.0.0")]),
            PRODUCTION_DEFAULT_HOST,
        )
        .unwrap(),
        ("::1".to_string(), 5000)
    );

    // With no variable set the config file is still honoured.
    assert_eq!(
        resolve_bind_address(&args(None, None), &configured, env(&[]), DEV_DEFAULT_HOST).unwrap(),
        ("127.0.0.1".to_string(), 4100)
    );

    // A declared-but-empty variable says nothing, so the config still applies.
    assert_eq!(
        resolve_bind_address(
            &args(None, None),
            &configured,
            env(&[("PORT", "  "), ("HOST", "")]),
            DEV_DEFAULT_HOST,
        )
        .unwrap(),
        ("127.0.0.1".to_string(), 4100)
    );

    // Whitespace around a real value is the platform's, not the user's.
    assert_eq!(
        resolve_bind_address(
            &args(None, None),
            &empty,
            env(&[("PORT", " 8080 "), ("HOST", " 0.0.0.0 ")]),
            DEV_DEFAULT_HOST,
        )
        .unwrap(),
        ("0.0.0.0".to_string(), 8080)
    );

    // A PORT that cannot be a port fails loudly rather than binding 3000, which
    // would show up only as a failing health check with nothing naming why.
    for invalid in ["abc", "70000", "-1", "8080.0"] {
        let error = resolve_bind_address(
            &args(None, None),
            &empty,
            |key| (key == "PORT").then(|| invalid.to_string()),
            PRODUCTION_DEFAULT_HOST,
        )
        .expect_err("an unusable PORT must be reported");
        assert!(
            format!("{error:#}").contains("PORT must be a number"),
            "unexpected error for PORT={invalid}: {error:#}"
        );
    }
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

/// Replay the shared cross-language path-safety table through the *writer*.
///
/// The fixture was introduced for the two readers — the native server's
/// `is_safe_relative_path` and the deployed handler's `isUnsafeSegment` — and
/// this third copy of the same rule, the one place a bad segment actually
/// becomes a filesystem path, was never enrolled. Its hand-written test was a
/// strict subset of the table: nothing about the safe cases `hello world`,
/// `a.b/c-d_e`, or a non-ASCII segment, and nothing about `foo:bar`, which the
/// fixture records as a past incident.
///
/// A writer stricter than the readers is not harmless either: it decides what
/// `getStaticParams()` may emit, so a divergence fails a build over a URL the
/// readers would have served.
#[test]
fn prerender_paths_replay_the_shared_cross_language_path_safety_table() {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/prerender-path-conformance.json");
    let fixture: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&fixture_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
    )
    .expect("conformance fixture is valid JSON");

    let cases = fixture["cases"].as_array().expect("fixture declares cases");
    assert!(!cases.is_empty(), "an empty table gates nothing");

    let prerender_dir = std::path::Path::new("/out/prerender");
    for case in cases {
        let path = case["path"].as_str().expect("case declares a path");
        let safe = case["safe"].as_bool().expect("case declares a verdict");
        let why = case["why"].as_str().unwrap_or_default();

        // The segment rule and the path mapper that consults it are held to the
        // same table: a safe path has only safe segments, and an unsafe one is
        // unsafe because at least one of its segments is.
        let unsafe_segments = path
            .split('/')
            .filter(|segment| !segment.is_empty())
            .filter(|segment| is_unsafe_prerender_segment(segment))
            .count();
        assert_eq!(unsafe_segments == 0, safe, "{path:?} — {why}");

        let written = prerender_html_path(prerender_dir, &format!("/{path}"));
        assert_eq!(written.is_some(), safe, "{path:?} — {why}");
        if let Some(written) = written {
            assert!(
                written.starts_with(prerender_dir),
                "{path:?} would be written outside the build output: {written:?}"
            );
        }
    }
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

/// The adapter runs last and can be checked first.
///
/// `--adapter herkou` used to spend a whole build — compile, bundle,
/// pre-render — before reporting a name that its own spelling had already
/// settled. The CLI cannot answer it alone, and deliberately so: any npm
/// package may be an adapter, so anything shaped like a package name is
/// accepted and resolution is left to the runner.
///
/// What makes the early check possible is the property asserted here:
/// inspection resolves the adapter **without reading build output**, which does
/// not exist yet when the build starts. A change that made inspection depend on
/// the output directory would move the failure back to the end of the build,
/// silently.
#[test]
fn an_adapter_resolves_before_a_build_has_produced_anything() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(
        root.join("ruvyxa.config.mjs"),
        "export default {}
",
    )
    .unwrap();
    let never_built = root.join(".ruvyxa");
    assert!(!never_built.exists(), "the point is that this is absent");

    let error = inspect_adapter(root, &never_built, JavaScriptRuntime::Node, Some("herkou"))
        .expect_err("an adapter that is not installed cannot be inspected");
    let message = error.to_string();
    assert!(message.contains("RUV2203"), "{message}");
    assert!(
        message.contains("herkou"),
        "the rejected name must be quoted back: {message}"
    );

    // And a resolvable one inspects from the same starting point, so the check
    // costs a correct build nothing but the resolution it was going to do
    // anyway.
    //
    // The adapter is fabricated here rather than named -- `node`, say -- on
    // purpose. `@ruvyxa/adapter-node` resolves through its `exports`, which
    // points at `dist/index.js`, and `require.resolve` fails when that file is
    // absent. CI runs `cargo test` before `pnpm -r build`, so every workspace
    // adapter is uncompiled at this moment and a built-in name would fail with
    // the very RUV2203 this test is meant to distinguish from success. It
    // passed locally only because a previous build had left `dist/` behind.
    let adapter_package = root.join("node_modules").join("ruvyxa-adapter-fixture");
    fs::create_dir_all(&adapter_package).unwrap();
    fs::write(
        adapter_package.join("package.json"),
        r#"{"name":"ruvyxa-adapter-fixture","version":"0.0.0","type":"module","main":"index.mjs"}"#,
    )
    .unwrap();
    fs::write(
        adapter_package.join("index.mjs"),
        "export default () => ({
  name: 'fixture',
  target: 'node',
  supports: ['ssr'],
  build: () => ({ name: 'fixture', target: 'node' }),
})
",
    )
    .unwrap();

    let inspection = inspect_adapter(root, &never_built, JavaScriptRuntime::Node, Some("fixture"))
        .expect("a resolvable adapter inspects with no build output")
        .expect("inspection describes it");
    assert_eq!(inspection.name, "fixture");
    // `.ruvyxa` does exist by now, but only because the config loader compiles
    // its bundle to the fixed `.ruvyxa/cache/` — the same directory that
    // happens to be the default `outDir`. What must stay absent is build
    // output: `loadBuildInfo` and `loadDeployManifest` both have to come back
    // empty-handed and let the inspection through.
    for produced_by_a_build in ["build.json", "manifest.json"] {
        assert!(
            !never_built.join(produced_by_a_build).exists(),
            "inspection ran without {produced_by_a_build}, and must not invent one"
        );
    }
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

/// Write a `node_modules/<name>/package.json` carrying nothing but a version,
/// which is all the installed-version probe reads.
fn install_package(root: &std::path::Path, name: &str, version: &str) {
    let mut dir = root.join("node_modules");
    for segment in name.split('/') {
        dir.push(segment);
    }
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("package.json"),
        format!(r#"{{"name":"{name}","version":"{version}"}}"#),
    )
    .unwrap();
}

#[test]
fn detects_react_version_compatibility() {
    let temp = tempfile::tempdir().unwrap();
    let package = json!({
        "dependencies": {
            "react": "^19.0.0",
            "react-dom": "^19.1.0"
        }
    });

    assert_eq!(react_compatibility(temp.path(), &package), "ok (major 19)");
}

/// npm installs a dependency's peers without adding them to the project
/// manifest, so an app that only ever ran `npm install ruvyxa` declares no
/// React at all and still renders. Reading the manifest alone reported that
/// working app as broken.
#[test]
fn reads_react_versions_installed_as_peer_dependencies() {
    let temp = tempfile::tempdir().unwrap();
    install_package(temp.path(), "react", "19.2.8");
    install_package(temp.path(), "react-dom", "19.2.8");
    let package = json!({ "dependencies": { "ruvyxa": "^1.0.32" } });

    assert_eq!(
        resolved_dependency_version(temp.path(), &package, "react").as_deref(),
        Some("19.2.8")
    );
    assert_eq!(react_compatibility(temp.path(), &package), "ok (major 19)");
}

/// The installed copy is what renders, so a manifest range that no longer
/// matches it must not decide the answer.
#[test]
fn installed_react_version_outranks_the_declared_range() {
    let temp = tempfile::tempdir().unwrap();
    install_package(temp.path(), "react", "19.2.8");
    let package = json!({ "dependencies": { "react": "^18.3.1" } });

    assert_eq!(
        resolved_dependency_version(temp.path(), &package, "react").as_deref(),
        Some("19.2.8")
    );
}

/// A project nested under a workspace root installs into that root, and the
/// probe walks up to find it — the same way the local binary lookup does.
#[test]
fn finds_react_installed_at_a_workspace_root() {
    let temp = tempfile::tempdir().unwrap();
    install_package(temp.path(), "react", "19.2.8");
    let app = temp.path().join("apps/site");
    std::fs::create_dir_all(&app).unwrap();

    assert_eq!(
        installed_dependency_version(&app, "react").as_deref(),
        Some("19.2.8")
    );
}

/// Neither installed nor declared is the one case that is genuinely broken.
#[test]
fn reports_react_missing_when_it_is_neither_installed_nor_declared() {
    let temp = tempfile::tempdir().unwrap();
    let package = json!({ "dependencies": { "ruvyxa": "^1.0.32" } });

    assert_eq!(react_compatibility(temp.path(), &package), "missing react");
}

/// Server components are opt-in, so a project without the RSC runtime gets no
/// row at all. Reporting it as missing would put a permanent warning on every
/// app that never enables the feature.
#[test]
fn says_nothing_about_the_rsc_runtime_when_the_project_does_not_use_it() {
    let temp = tempfile::tempdir().unwrap();
    install_package(temp.path(), "react", "19.2.8");

    assert_eq!(server_components_compatibility(temp.path()), None);
}

/// `react-server-dom-webpack` reaches into React internals rather than a public
/// API, so the versions have to be equal — a matching major is not enough, and
/// the pair that fails this way fails inside React with no mention of either.
#[test]
fn holds_the_rsc_runtime_to_the_exact_react_version() {
    let matched = tempfile::tempdir().unwrap();
    install_package(matched.path(), "react", "19.2.8");
    install_package(matched.path(), "react-server-dom-webpack", "19.2.8");

    assert_eq!(
        server_components_compatibility(matched.path()).as_deref(),
        Some("ok (19.2.8)")
    );

    let drifted = tempfile::tempdir().unwrap();
    install_package(drifted.path(), "react", "19.2.8");
    install_package(drifted.path(), "react-server-dom-webpack", "19.3.0");

    assert_eq!(
        server_components_compatibility(drifted.path()).as_deref(),
        Some("mismatch react 19.2.8 vs react-server-dom-webpack 19.3.0")
    );
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
fn every_benchmark_scenario_is_named_once_and_explained() {
    // The run itself checks that the table and the runner agree, which is the
    // drift that matters. What it cannot catch is a duplicate id: two rows
    // spelled the same would satisfy that comparison while the table describes
    // one scenario twice and another not at all.
    let ids = PROJECT_SCENARIOS
        .iter()
        .map(|(id, _)| *id)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        ids.len(),
        PROJECT_SCENARIOS.len(),
        "two benchmark scenarios share an id"
    );
    for (id, description) in PROJECT_SCENARIOS {
        assert!(!id.trim().is_empty(), "a scenario has no id");
        assert!(
            !description.trim().is_empty(),
            "{id} has no line explaining what it measures"
        );
    }
}

#[test]
fn the_benchmark_measures_a_cold_build_and_a_warm_one_separately() {
    // The row these two replaced ran every sample against the project's own
    // cache and averaged the results, so its number was neither cold nor warm.
    // Keeping both names asserted here is what stops them being folded back
    // into one row that looks tidier and means less.
    let ids = PROJECT_SCENARIOS.map(|(id, _)| id);
    assert!(ids.contains(&"build-cold"));
    assert!(ids.contains(&"build-warm"));
    assert!(
        !ids.contains(&"production-build"),
        "the row that mixed cold and warm costs is back"
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

/// The CPU ceiling, which is the half of the decision that has exact answers.
///
/// This used to be asserted through `prerender_parallelism`, which also passes
/// the budget through `prerender_worker_budget` — a memory bound that lowers
/// the answer on a machine short of RAM, deliberately. So the assertions read
/// as claims about this code and were partly claims about the host: on this
/// tree, with 1.2 GB free, `prerender_parallelism(Some(64), 32)` answered 5
/// where the test demanded 8, and the same test passed seconds later once the
/// build beside it had exited.
#[test]
fn caps_the_configured_prerender_cpu_budget_at_the_worker_pool_limit() {
    assert_eq!(prerender_cpu_budget(Some(3)), 3);
    // An explicit configuration may exceed the default cap, up to the worker
    // pool limit — and no further.
    assert_eq!(
        prerender_cpu_budget(Some(64)),
        MAX_CONFIGURED_PRERENDER_PARALLELISM
    );
    assert!(prerender_cpu_budget(None) <= MAX_PRERENDER_PARALLELISM);
    assert!(prerender_cpu_budget(None) >= 1);
}

/// What `prerender_parallelism` promises whatever the machine has spare: never
/// more workers than there is work, and never fewer than one.
#[test]
fn caps_default_prerender_parallelism_to_limit_and_available_work() {
    assert_eq!(prerender_parallelism(None, 1), 1);
    assert_eq!(prerender_parallelism(Some(3), 0), 1);
    assert!(prerender_parallelism(None, 10) <= MAX_PRERENDER_PARALLELISM);
    assert!(prerender_parallelism(Some(3), 2) <= 2);
    assert!(prerender_parallelism(Some(64), 32) <= MAX_CONFIGURED_PRERENDER_PARALLELISM);
    assert!(prerender_parallelism(Some(64), 32) >= 1);
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

/// Every path this returns is canonical, including the ones remapped out of the
/// staging tree.
///
/// Both callers hand the result straight to `store_prerender_artifact`, which
/// keys the cached artifact by exactly what it is given and does not
/// canonicalize again — so the guarantee has to hold on every branch, not just
/// the one that covers most inputs. The staging remap rebuilt its answer from
/// the caller's own `root`, which is the value `--root` was given rather than a
/// resolved path, so the same file was keyed two ways depending on which branch
/// produced it.
#[test]
fn stable_prerender_inputs_are_canonical_even_when_remapped_out_of_staging() {
    let temp = tempfile::tempdir().unwrap();
    let canonical = ruvyxa_diagnostics::normalized_canonical_path(temp.path());
    let project = canonical.join("project");
    let page = project.join("app").join("page.tsx");
    fs::create_dir_all(page.parent().unwrap()).unwrap();
    fs::write(&page, "export default 1").unwrap();

    // The staging tree the build renders from: `<staging>/server/app`, sitting
    // outside the project because the output directory does.
    let staged_app = canonical.join("out").join("server").join("app");
    fs::create_dir_all(&staged_app).unwrap();
    let staged_page = staged_app.join("page.tsx");
    fs::write(&staged_page, "export default 1").unwrap();

    // `--root` is used as typed. This spelling names the project and is not
    // canonical, which is the whole difference the remap branch let through.
    let typed_root = project.join("app").join("..");

    let inputs = stable_prerender_inputs(&typed_root, &staged_app, &[staged_page]);

    assert_eq!(
        inputs,
        vec![page],
        "a staging path remapped onto the project must come back canonical"
    );
}

/// The prerender artifact store keys by the paths it is handed, and does not
/// canonicalize them a second time.
///
/// Its callers pass `stable_prerender_inputs` output, which has already resolved
/// and canonicalized every path — and `normalized_canonical_path` is a
/// filesystem syscall, the expensive one on Windows. Repeating it here cost two
/// per input for every stored artifact, so a dynamic route expanded to thousands
/// of paths paid `2 × modules` of them per path with nothing to show for the
/// second.
///
/// Asserted through the stored file because there is nothing else to see: the
/// second call answered identically by construction, which is exactly why it
/// survived. If a future caller has to pass unresolved paths, canonicalize at
/// that caller — the store is on the hot loop and this one is not.
#[test]
fn store_prerender_artifact_keys_by_the_paths_it_is_given() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("page.tsx");
    fs::write(&source, "export default () => 'first'").unwrap();
    // The same file, spelled the way a caller that had not resolved it would.
    let unresolved = temp.path().join("nested").join("..").join("page.tsx");
    fs::create_dir_all(temp.path().join("nested")).unwrap();

    let job = PrerenderJob {
        route_path: "/keyed".to_string(),
        render_path: "/keyed".to_string(),
        params: RouteParams::new(),
        strategy: RenderStrategy::Ssg,
        revalidate: None,
        kind: PrerenderJobKind::Render {
            route_file: source.clone(),
            mode: "full",
            server_components: false,
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
        std::slice::from_ref(&unresolved),
        "<main>keyed</main>",
    );

    let stored: serde_json::Value = serde_json::from_slice(
        &fs::read(prerender_artifact_cache_file(&cache.directory, &job)).unwrap(),
    )
    .unwrap();
    let keys = stored["files"]
        .as_object()
        .expect("the artifact records the files it was keyed against")
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(
        keys,
        vec![unresolved.to_string_lossy().into_owned()],
        "the store must record the path it was given, not a re-canonicalized one"
    );

    // And an artifact keyed that way is still validated against the file it
    // names, which is the only thing the key is for.
    assert_eq!(
        load_prerender_artifact(&cache, &job).as_deref(),
        Some("<main>keyed</main>")
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
            server_components: false,
        },
    };
    let cache = PrerenderArtifactCache {
        directory: temp.path().join("cache"),
        dependency_hash: "config-v1".to_string(),
        render_context_hash: "context-v1".to_string(),
        fingerprints: Arc::new(ArtifactFingerprintCache::default()),
        enabled: true,
    };

    // Keyed exactly as the two real callers key it: `stable_prerender_inputs`
    // output, already resolved and canonical, with no second canonicalization
    // inside the store.
    let inputs = stable_prerender_inputs(
        temp.path(),
        &temp.path().join("app"),
        std::slice::from_ref(&source),
    );
    assert_eq!(
        inputs,
        vec![ruvyxa_diagnostics::normalized_canonical_path(&source)]
    );

    store_prerender_artifact(&cache, &job, "renderer-v1", &inputs, "<main>first</main>");
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

/// A server-components answer is only reusable while every file behind it is
/// unchanged — and "behind it" means both compiles, not just the one that names
/// the route.
///
/// The expensive part of a warm production build was asking the `react-server`
/// graph what a route's browser entry contains and which `'use server'` modules
/// it reaches. Neither depends on the request, so both are cached. What makes
/// that safe is the input list the worker reports: the union of the server
/// graph's files and the client registry's, because the server graph reads a
/// `'use client'` module and stops there. The action module behind one is
/// invisible to it, and the reference ids in this answer are versioned by that
/// module's source — so an answer kept across an edit to it hands the browser a
/// proxy for a function the server no longer registers.
/// `server-component-entry-inputs.test.mjs` holds the worker to reporting both.
#[test]
fn server_component_entry_cache_invalidates_on_an_edit_behind_a_client_component() {
    let temp = tempfile::tempdir().unwrap();
    let page = temp.path().join("page.tsx");
    let actions = temp.path().join("actions.ts");
    fs::write(&page, "export const serverComponents = true").unwrap();
    fs::write(&actions, "'use server'\nexport const save = () => {}").unwrap();
    let inputs = vec![page.clone(), actions.clone()];
    let reference = ruvyxa_dev_server::ServerReferenceSource {
        id: "ruv:s_0123456789abcdef".to_string(),
        file: actions.clone(),
        source: "export const save = () => {}".to_string(),
    };
    let cache = ServerComponentEntryCache {
        directory: temp.path().join("cache"),
        dependency_hash: "config-v1".to_string(),
        context_hash: "worker-v1".to_string(),
        fingerprints: Arc::new(ArtifactFingerprintCache::default()),
    };

    store_server_component_entry(&cache, "/rsc", &inputs, "export default 1", &[reference]);
    let hit = load_server_component_entry(&cache, "/rsc").expect("a stored entry must be reusable");
    assert_eq!(hit.entry_source, "export default 1");
    assert_eq!(hit.server_references.len(), 1);

    fs::write(&actions, "'use server'\nexport const save = (word) => word").unwrap();
    let next_build = ServerComponentEntryCache {
        fingerprints: Arc::new(ArtifactFingerprintCache::default()),
        ..cache
    };
    assert!(
        load_server_component_entry(&next_build, "/rsc").is_none(),
        "an edited action must not be answered from the previous reference list"
    );
}

/// Upgrading Ruvyxa, or changing what the worker is started with, changes what
/// the same files compile to — so neither may be left out of cache identity.
#[test]
fn server_component_entry_cache_answers_only_for_its_own_worker_context() {
    let temp = tempfile::tempdir().unwrap();
    let page = temp.path().join("page.tsx");
    fs::write(&page, "export const serverComponents = true").unwrap();
    let cache = ServerComponentEntryCache {
        directory: temp.path().join("cache"),
        dependency_hash: "config-v1".to_string(),
        context_hash: "worker-v1".to_string(),
        fingerprints: Arc::new(ArtifactFingerprintCache::default()),
    };
    store_server_component_entry(&cache, "/rsc", std::slice::from_ref(&page), "entry", &[]);

    for (dependency_hash, context_hash) in [("config-v2", "worker-v1"), ("config-v1", "worker-v2")]
    {
        let changed = ServerComponentEntryCache {
            directory: cache.directory.clone(),
            dependency_hash: dependency_hash.to_string(),
            context_hash: context_hash.to_string(),
            fingerprints: Arc::new(ArtifactFingerprintCache::default()),
        };
        assert!(
            load_server_component_entry(&changed, "/rsc").is_none(),
            "{dependency_hash}/{context_hash} must not read an entry produced under another"
        );
    }
}

/// The environment the worker runs under decides which modules its graph
/// resolves, so two environments cannot share one cached answer.
#[test]
fn server_component_context_hash_follows_the_worker_environment() {
    let temp = tempfile::tempdir().unwrap();
    let base = BTreeMap::from([("RUVYXA_JSX_RUNTIME".to_string(), "automatic".to_string())]);
    let classic = BTreeMap::from([("RUVYXA_JSX_RUNTIME".to_string(), "classic".to_string())]);
    let runtime = ruvyxa_dev_server::JavaScriptRuntime::Node;

    let baseline = server_component_context_hash(temp.path(), runtime, &base);
    assert_eq!(
        baseline,
        server_component_context_hash(temp.path(), runtime, &base),
        "the same inputs must produce the same hash"
    );
    assert_ne!(
        baseline,
        server_component_context_hash(temp.path(), runtime, &classic),
        "a different JSX runtime compiles to different modules"
    );
    assert_ne!(
        baseline,
        server_component_context_hash(
            temp.path(),
            ruvyxa_dev_server::JavaScriptRuntime::Bun,
            &base
        ),
        "a config may branch on `process.versions`, so the runtime is part of the answer"
    );
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

/// The two mistakes every project makes once must name themselves.
///
/// Both used to reach route discovery, which sees only that a directory is
/// absent and answers RUV1001 — *"Create app/page.tsx … or set appDir in
/// ruvyxa.config.ts"* — with a build-output path in its `File:` line. For
/// `start` before `build` that is advice to edit a `page.tsx` that already
/// exists; for a mistyped `--root` it is advice to create a file inside a
/// directory that is not there. A diagnostic that cannot be followed is worse
/// than none, because the reader believes it.
#[test]
fn a_missing_build_is_reported_as_a_missing_build() {
    let temp = tempfile::tempdir().unwrap();
    let args = ServerArgs {
        root: temp.path().to_path_buf(),
        host: None,
        port: None,
        runtime: None,
    };
    let config: ProjectConfig = serde_json::from_value(json!({})).unwrap();

    let error = ensure_build_output_exists(&args, &config)
        .expect_err("a project that has never been built must not be served");
    let message = error.to_string();
    assert!(message.contains("RUV1015"), "{message}");
    assert!(
        message.contains("ruvyxa build"),
        "the fix must name the command that produces what is missing: {message}"
    );
    assert!(
        !message.contains("RUV1001") && !message.contains("appDir"),
        "must not send the reader to create a page or edit appDir: {message}"
    );

    // And it stops complaining once the build output is there.
    std::fs::create_dir_all(temp.path().join(".ruvyxa/server/app")).unwrap();
    ensure_build_output_exists(&args, &config).expect("a built project is servable");
}

#[test]
fn a_missing_project_root_is_reported_as_a_missing_root() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("no-such-project");

    let error = load_project_config(&missing).expect_err("there is no project to load");
    let message = error.to_string();
    assert!(message.contains("RUV1014"), "{message}");
    assert!(
        message.contains("--root"),
        "the fix must name the flag that is wrong: {message}"
    );
    assert!(
        !message.contains("page.tsx"),
        "a directory that does not exist cannot be fixed by creating a file in it: {message}"
    );
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

/// Every other numeric field in the file fails a build when it is out of
/// range. `image.quality` and `image.effort` were silently clamped instead, so
/// a project asking for something impossible got a different build than it
/// asked for and was told nothing; `image.workers` was handed straight to
/// `rayon::ThreadPoolBuilder`, where a typo becomes a spawn failure naming
/// rayon rather than the config key, or a thrashing build.
#[test]
fn rejects_out_of_range_image_settings() {
    for (field, value) in [
        ("quality", json!(0)),
        ("quality", json!(101)),
        ("effort", json!(7)),
        ("workers", json!(100_000)),
    ] {
        let config: ProjectConfig =
            serde_json::from_value(json!({ "image": { field: value } })).unwrap();
        let error = config.validate_paths().unwrap_err().to_string();
        assert!(
            error.contains(&format!("image.{field}")),
            "image.{field} = {value} should be rejected by name, got: {error}"
        );
    }
}

/// The bounds are exactly the ones the shared image fixture declares, and the
/// two documented "let the host decide" zeros stay legal: `image.workers: 0`
/// means Rayon's own worker count, and `image.effort: 0` is libwebp's fastest
/// encode.
#[test]
fn accepts_image_settings_at_their_documented_bounds() {
    for value in [
        json!({ "quality": 1 }),
        json!({ "quality": 100 }),
        json!({ "effort": 0 }),
        json!({ "effort": 6 }),
        json!({ "workers": 0 }),
        json!({ "workers": crate::image_optimizer::MAX_CONFIGURED_IMAGE_WORKERS }),
    ] {
        let config: ProjectConfig = serde_json::from_value(json!({ "image": value })).unwrap();
        config
            .validate_paths()
            .unwrap_or_else(|error| panic!("{value} must be accepted, got: {error}"));
    }
}

/// The quality bounds are a shared contract with the two request hosts that
/// answer `/__ruvyxa/image`, and this is the third implementation of them.
#[test]
fn image_quality_bounds_match_the_shared_fixture() {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/dynamic-image-conformance.json");
    let fixture: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture_path).unwrap()).unwrap();
    assert_eq!(fixture["quality"]["min"], json!(1));
    assert_eq!(
        fixture["quality"]["max"],
        json!(crate::image_optimizer::MAX_IMAGE_QUALITY)
    );
}

/// Machine-readable output must go through `write_machine_report`.
///
/// The helper exists because `println!` panics when the reader closes the pipe
/// — `ruvyxa doctor --json | jq '.adapter'` turns a successful diagnostic run
/// into an abort and a failed CI step — and the helper's own test tests the
/// helper, so it could never notice a command that does not call it. Three did:
/// `doctor --json`, `trace`, and `bench --json`.
///
/// The needles are assembled from pieces so this test does not match itself.
#[test]
fn no_command_prints_json_through_a_panicking_writer() {
    let print_macro = concat!("print", "ln!");
    let serializer = concat!("serde_json::", "to_string");
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources: Vec<std::path::PathBuf> = std::fs::read_dir(&src)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "rs"))
        .collect();
    sources.sort();
    assert!(sources.len() > 5, "no sources were scanned");

    let mut offenders = Vec::new();
    for path in &sources {
        let text = std::fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        for (index, line) in lines.iter().enumerate() {
            if !line.contains(print_macro) {
                continue;
            }
            // The serializer call sits on the same line for a one-liner and on
            // the next for a `json!` block; a short window covers both without
            // parsing the macro.
            let window = lines[index..lines.len().min(index + 5)].join("\n");
            if window.contains(serializer) {
                offenders.push(format!("{}:{}", path.display(), index + 1));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these write machine-readable output through a writer that panics on a \
         closed pipe; call write_machine_report instead: {offenders:?}"
    );
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

/// `build.target` reaches the transform, and an unusable value is refused.
///
/// It spent several releases accepted-and-inert: validated, carried into
/// `BundleOptions`, and consumed by neither compiler. The three halves that
/// make it real are asserted together so a future edit cannot quietly drop one
/// — it parses, it reaches the option struct the bundler reads, and a value the
/// transformer cannot honour is named rather than accepted.
#[test]
fn build_target_is_parsed_validated_and_reaches_the_bundle_option() {
    let build: BuildConfigOptions =
        serde_json::from_str(r#"{"target":"es2018","jsx":"automatic"}"#).unwrap();
    assert_eq!(
        parse_es_target(build.es_target.as_ref()).unwrap(),
        ruvyxa_bundler::EsTarget::Es2018
    );

    // Absent means the level every project already emitted, so a config that
    // says nothing keeps producing the bytes it always did.
    let empty: BuildConfigOptions = serde_json::from_str("{}").unwrap();
    assert_eq!(
        parse_es_target(empty.es_target.as_ref()).unwrap(),
        ruvyxa_bundler::EsTarget::EsNext
    );

    // `es5` is the value a reader is most likely to try, and oxc does not
    // implement it. Accepting it was the old behaviour and the whole defect.
    let unusable: BuildConfigOptions = serde_json::from_str(r#"{"target":"es5"}"#).unwrap();
    let error = parse_es_target(unusable.es_target.as_ref()).unwrap_err();
    assert!(error.to_string().contains("RUV1601"), "{error}");
    assert!(
        error.to_string().contains("es5 is not implemented"),
        "{error}"
    );

    let wrong_type: BuildConfigOptions = serde_json::from_str(r#"{"target":2018}"#).unwrap();
    assert!(parse_es_target(wrong_type.es_target.as_ref()).is_err());

    // The option struct handed to the bundler carries it, so there is no field
    // for a future change to leave silently unread.
    let options = client_bundle_options(&build).unwrap();
    let encoded = serde_json::to_value(&options).unwrap();
    assert_eq!(
        encoded.get("es_target").and_then(serde_json::Value::as_str),
        Some("es2018")
    );
}

/// The written `.js` and the written `.js.map` describe the same file.
///
/// The bundler emits a map for the bundle it produced, and this module then
/// prepends `import "./shared.<hash>.js";` to routes that read the shared
/// registry — after the map was built. Every mapping named the line above the
/// one it described, and the whole feature was one line short on exactly the
/// routes a real project has.
///
/// The check is the one a debugger makes: find a token in the emitted file,
/// resolve its position through the map, and require the source line it names
/// to be the line that token came from.
#[test]
fn an_emitted_bundle_and_its_source_map_agree_after_the_shared_import() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    let client_dir = root.join(".ruvyxa").join("client");
    std::fs::create_dir_all(app.join("second")).unwrap();
    std::fs::create_dir_all(&client_dir).unwrap();

    // A module both routes import, so the build lifts it into a shared chunk
    // and both route bundles get the prepended import.
    std::fs::write(
        app.join("shared-label.ts"),
        "export const SHARED_LABEL = 'shared-label-token';\n",
    )
    .unwrap();
    std::fs::write(
        app.join("page.tsx"),
        "import { SHARED_LABEL } from './shared-label';\n\
         export default function Page() { return <main>{SHARED_LABEL}{\"route-token-alpha\"}</main>; }\n",
    )
    .unwrap();
    std::fs::write(
        app.join("second/page.tsx"),
        "import { SHARED_LABEL } from '../shared-label';\n\
         export default function Second() { return <main>{SHARED_LABEL}{\"route-token-beta\"}</main>; }\n",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let build = BuildConfigOptions {
        minify: Some(false),
        sourcemap: Some(true),
        tree_shaking: Some(false),
        split_strategy: Some("route".to_string()),
        parallelism: Some(1),
        jsx_runtime: Some("classic".to_string()),
        es_target: None,
        emit_chunk_manifest: Some(true),
        prebundle_dependencies: Some(true),
        prerender_cache: Some(true),
    };

    emit_client_bundles(
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

    let maps = std::fs::read_dir(&client_dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.to_string_lossy().ends_with(".js.map"))
        .collect::<Vec<_>>();
    assert!(!maps.is_empty(), "sourcemap: true must emit maps");

    let mut resolved_any = false;
    for map_path in maps {
        let script_path = PathBuf::from(map_path.to_string_lossy().trim_end_matches(".map"));
        let script = std::fs::read_to_string(&script_path).unwrap();
        // Only the routes that actually took the shared import are interesting,
        // and they are the ones this bug applied to.
        if !script.starts_with("import \"./shared.") {
            continue;
        }
        let map: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&map_path).unwrap()).unwrap();
        let contents = map["sourcesContent"]
            .as_array()
            .expect("sourcesContent")
            .iter()
            .map(|value| {
                // `null` is a legal entry meaning "content not available", and
                // reading it as an empty line list would let this test locate a
                // token at a position no source has. Every source in this
                // fixture carries content, so a `null` here means the map is not
                // the one the assertions below are written against.
                value
                    .as_str()
                    .expect("every source in this fixture carries its content")
                    .lines()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>();

        // A literal the route bundle keeps: the shared value itself is emitted
        // into the shared chunk, which is a different file with its own lines.
        const TOKEN: &str = "route-token-";
        let Some((line, column)) = script
            .lines()
            .enumerate()
            .find_map(|(index, text)| text.find(TOKEN).map(|column| (index, column)))
        else {
            continue;
        };
        let decoded =
            ruvyxa_bundler::sourcemap::decode_mappings(map["mappings"].as_str().expect("mappings"));
        let resolved = decoded
            .iter()
            .filter(|mapping| mapping.generated_line == line && mapping.generated_column <= column)
            .max_by_key(|mapping| mapping.generated_column)
            .unwrap_or_else(|| {
                panic!("nothing maps the position holding {TOKEN} in {script_path:?}")
            });
        let original = contents
            .get(resolved.source)
            .and_then(|lines| lines.get(resolved.original_line))
            .unwrap_or_else(|| panic!("mapping points past the end of its source"));
        assert!(
            original.contains(TOKEN),
            "{script_path:?}: the position holding {TOKEN} maps to {} line {} — {original:?}",
            map["sources"][resolved.source],
            resolved.original_line
        );
        resolved_any = true;
    }
    assert!(
        resolved_any,
        "no route took the shared import, so nothing was actually checked"
    );
}

/// The shared chunk runs its modules in the order the routes run them.
///
/// A route bundle `import`s the shared chunk, so the whole chunk is evaluated
/// before the route's own first statement. Once two modules are both in there,
/// the route's import order has no say left — the chunk's own order is the only
/// thing deciding which runs first, and it therefore has to be the route's.
///
/// The pair below has a load-order dependency the module graph cannot express:
/// one writes a global and the other reads it, with no import between them. The
/// filenames are chosen so that sorting by path reverses the routes' order,
/// because sorting is exactly what used to happen — the shared module list was
/// collected into a `BTreeSet` — and it put React's server-components decoder
/// ahead of the module that installs the globals it reads at load time. Every
/// server-components page stopped hydrating in production and nothing failed
/// anywhere else.
#[test]
fn the_shared_chunk_keeps_the_order_the_routes_evaluate_in() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    let client_dir = root.join(".ruvyxa").join("client");
    std::fs::create_dir_all(app.join("second")).unwrap();
    std::fs::create_dir_all(&client_dir).unwrap();

    // Sorts last, must run first.
    std::fs::write(
        app.join("zz-install.ts"),
        "export const INSTALLED = 'zz-install-token';\n\
         globalThis.__ruvyxaOrderProbe = INSTALLED;\n",
    )
    .unwrap();
    // Sorts first, must run second: it reads at module scope what the other one
    // wrote at module scope.
    std::fs::write(
        app.join("aa-reader.ts"),
        "export const SEEN = 'aa-reader-token' + String(globalThis.__ruvyxaOrderProbe);\n",
    )
    .unwrap();

    for (file, marker) in [
        (app.join("page.tsx"), "route-alpha"),
        (app.join("second/page.tsx"), "route-beta"),
    ] {
        let prefix = if file.ends_with("second/page.tsx") {
            ".."
        } else {
            "."
        };
        std::fs::write(
            &file,
            format!(
                "import {{ INSTALLED }} from '{prefix}/zz-install';\n\
                 import {{ SEEN }} from '{prefix}/aa-reader';\n\
                 export default function Page() {{ return <main>{{INSTALLED}}{{SEEN}}{{\"{marker}\"}}</main>; }}\n"
            ),
        )
        .unwrap();
    }

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let build = BuildConfigOptions {
        minify: Some(false),
        sourcemap: Some(false),
        tree_shaking: Some(false),
        split_strategy: Some("route".to_string()),
        parallelism: Some(1),
        jsx_runtime: Some("classic".to_string()),
        es_target: None,
        emit_chunk_manifest: Some(false),
        prebundle_dependencies: Some(true),
        prerender_cache: Some(true),
    };

    emit_client_bundles(
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

    let shared = std::fs::read_dir(&client_dir)
        .unwrap()
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("shared."))
        })
        .expect("two routes sharing two modules must produce a shared chunk");
    let code = std::fs::read_to_string(&shared).unwrap();

    let installs = code
        .find("zz-install-token")
        .expect("the shared chunk must contain the installing module");
    let reads = code
        .find("aa-reader-token")
        .expect("the shared chunk must contain the reading module");
    assert!(
        installs < reads,
        "the shared chunk evaluates the reader before the installer, which is \
         what sorting the module list by path does: {shared:?}"
    );
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
        es_target: None,
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
    let client_report = temp.path().join(CLIENT_BUILD_REPORT_FILE);
    std::fs::write(
        &client_report,
        r#"{"routes":[{"path":"/docs/[slug]","src":"/__ruvyxa/client/docs.123.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.456.js"}]}]}"#,
    )
    .unwrap();
    let client_assets = load_prerender_client_assets(&client_report).unwrap();
    assert_eq!(client_assets.len(), 1);

    let html = inject_prerender_client_assets(
        "<!doctype html><html><head><title>Docs</title></head><body><main>Guide</main></body></html>",
        &client_assets,
        "/docs/[slug]",
        "/docs/start",
        &BTreeMap::from([("slug".to_string(), serde_json::json!("start"))]),
        None,
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
    let client_report = temp.path().join(CLIENT_BUILD_REPORT_FILE);
    std::fs::write(
        &client_report,
        r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/home.js","sharedChunks":[{"src":"/__ruvyxa/client/shared.js"}],"hydration":"visible","hydrationLoader":"/__ruvyxa/client/hydration.js"}]}"#,
    )
    .unwrap();
    let assets = load_prerender_client_assets(&client_report).unwrap();

    let html = inject_prerender_client_assets(
        "<!doctype html><html><head></head><body><main>Home</main></body></html>",
        &assets,
        "/",
        "/",
        &BTreeMap::new(),
        None,
    );

    assert!(!html.contains("modulepreload"), "{html}");
    assert!(html.contains("hydration.js?strategy=visible&amp;src=/__ruvyxa/client/home.js"));
    assert!(!html.contains(r#"src="/__ruvyxa/client/home.js""#));
}

/// A pre-rendered document is served from disk with no renderer left to touch
/// it, so everything the live pipeline puts in the head has to be in it already
/// — the stylesheet and the links for what the project publishes.
///
/// The icon link is why: without it a browser falls back to requesting
/// `/favicon.ico`, and a project that publishes an icon under another name has
/// no such file. Every production page load logged a 404 that `ruvyxa dev`,
/// which renders through the pipeline that injects these, never showed.
///
/// A plugin's declared head is the same shape and was missing for the same
/// reason. `render_page_ssg` composes `defaults + plugin head + stylesheet`;
/// this composed `defaults + stylesheet`, so `fonts`, an analytics snippet, or
/// a site-verification tag rendered under `ruvyxa dev` and appeared in no baked
/// page at all — and static is what most pages are.
#[test]
fn prerender_html_includes_the_document_head_the_live_renderer_composes() {
    let html = inject_prerender_head(
        "<!doctype html><html><head><title>Docs</title></head><body><main>Guide</main></body></html>",
        &PrerenderHead {
            asset_links: Arc::from(r#"<link rel="icon" type="image/png" href="/ruvyxa.png">"#),
            plugin_head: Arc::from(r#"<link rel="stylesheet" href="/fonts/fonts.css">"#),
            // The finished tag, which is what a build now hands over: it links
            // the stylesheet it emitted rather than inlining the rule text, so
            // a baked page and a request-time render reference one file.
            styles: Arc::from(r#"<link rel="stylesheet" href="/__ruvyxa/client/styles.abc.css">"#),
            shell: CsrShell::default(),
        },
    );

    assert!(html.contains(r#"<link rel="stylesheet" href="/__ruvyxa/client/styles.abc.css">"#));
    assert!(html.contains(r#"<link rel="icon" type="image/png" href="/ruvyxa.png">"#));
    assert!(html.contains(r#"<link rel="stylesheet" href="/fonts/fonts.css">"#));
    assert!(
        html.contains(r#"<meta name="viewport" content="width=device-width, initial-scale=1">"#)
    );
    assert!(html.find("stylesheet").unwrap() < html.find("</head>").unwrap());
    assert!(html.find(r#"rel="icon""#).unwrap() < html.find("</head>").unwrap());
    assert!(html.find("/fonts/fonts.css").unwrap() < html.find("</head>").unwrap());
    assert!(html.contains("<main>Guide</main>"));
}

/// The head a build composes is read from the project's own configuration.
///
/// The test above holds the composition; this one holds the wiring, which is
/// the half that was actually missing. `config.plugins[].head` was parsed,
/// carried through config load, and read by exactly the two server hosts —
/// never by the build — so every field in it existed and none of it reached a
/// file. Going through `prerender_head` from a real loaded config is what makes
/// that reachable-but-unread state fail here instead of in production.
#[test]
fn a_plugin_head_declaration_reaches_the_pages_a_build_bakes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("app")).unwrap();
    std::fs::write(
        root.join("ruvyxa.config.ts"),
        r#"
import { config } from "ruvyxa/config"
import { definePlugin } from "ruvyxa/plugin"

export default config({
  plugins: [definePlugin({
name: "analytics",
head: [{ tag: "script", attrs: { src: "https://example.test/a.js", defer: true } }],
  })],
})
"#,
    )
    .unwrap();

    let config = load_project_config(root).unwrap();
    let head =
        crate::build::prerender_head(&config, &root.join("assets"), None, "", CsrShell::default());

    let rendered = inject_prerender_head(
        "<!doctype html><html><head><title>Docs</title></head><body><main>Guide</main></body></html>",
        &head,
    );
    assert!(
        rendered.contains(r#"src="https://example.test/a.js""#),
        "a rendered page must carry the plugin's tag: {rendered}"
    );
    assert!(rendered.find("example.test").unwrap() < rendered.find("</head>").unwrap());

    // And the shell, which is a separate template rather than an injection into
    // a rendered document, so the two can be missing it independently.
    let shell = csr_shell_html("/", &BTreeMap::new(), &head);
    assert!(
        shell.contains(r#"src="https://example.test/a.js""#),
        "a client-rendered shell must carry it too: {shell}"
    );
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
            dependency_hash: &config.build_dependency_hash,
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

/// `.env` is a build input, so editing one has to invalidate compiled bytes.
///
/// `import.meta.env` is substituted into every module the compiler emits, as a
/// frozen literal — that substitution is what makes a `RUVYXA_PUBLIC_*` value
/// readable in a browser at all. So the value is *in* the browser bundle, and
/// the caches that hold that bundle are keyed on this hash: the module compile
/// cache and its namespace, the artifact graph, the client route artifacts,
/// the shared chunk artifacts.
///
/// Keyed on the config alone, none of them noticed. Editing `.env` and
/// rebuilding produced a build whose pre-rendered HTML carried the new value —
/// `prerender_context_hash` has always keyed on the environment — and whose
/// browser bundle still carried the old one. One build, two answers for the
/// same variable in the same page, and the browser's is the one that wins the
/// moment hydration runs.
///
/// Asserted on the hash rather than on emitted bytes: `set_public_env` is a
/// `OnceLock`, so a single test process substitutes one environment for its
/// whole life. The hash is the mechanism that forces the recompile, and the
/// end-to-end behaviour was checked by building `examples/demo` twice.
#[test]
fn an_env_change_invalidates_the_bytes_it_is_compiled_into() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("app")).unwrap();
    std::fs::write(
        root.join("ruvyxa.config.mjs"),
        "export default {}
",
    )
    .unwrap();

    let hash_with = |contents: &str| {
        std::fs::write(root.join(".env"), contents).unwrap();
        load_project_config(root).unwrap().build_dependency_hash
    };

    let first = hash_with(
        "RUVYXA_PUBLIC_API_URL=https://staging.example.test
",
    );
    let again = hash_with(
        "RUVYXA_PUBLIC_API_URL=https://staging.example.test
",
    );
    assert_eq!(
        first, again,
        "an unchanged environment must not force a rebuild"
    );

    let changed = hash_with(
        "RUVYXA_PUBLIC_API_URL=https://api.example.test
",
    );
    assert_ne!(
        first, changed,
        "the compiled bytes carry this value, so the caches that hold them must miss"
    );

    // The config file never changed across any of those, which is the whole
    // point: the config-load cache answers from its stored output, and the
    // environment has to be read again on that path rather than folded in
    // once and remembered.
    let removed = {
        std::fs::remove_file(root.join(".env")).unwrap();
        load_project_config(root).unwrap().build_dependency_hash
    };
    assert_ne!(
        changed, removed,
        "removing a variable changes the emitted literal too"
    );
}

/// An unreadable `.env` is not the same as no `.env`.
///
/// Everything downstream keys on this hash -- the module compile cache and its
/// namespace, the artifact graph, the client route artifacts, the shared chunk
/// artifacts -- and all of them hold compiled bytes with `RUVYXA_PUBLIC_*`
/// values written into them as literals. Swallowing the read mapped "I could
/// not open the environment" onto the hash for "there is none", so the build
/// compiled without values the project had declared and then cached the result
/// under a key that could not tell the difference.
///
/// `project_env` fails only when a `.env` exists and cannot be read, which is
/// why absence still hashes as it always did.
#[test]
fn an_unreadable_env_is_reported_rather_than_hashed_as_absent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    std::fs::create_dir_all(root.join("app")).expect("app dir");

    // No `.env` at all: the documented case, and it still loads.
    let absent = crate::load_project_config(root).expect("a project with no .env still loads");
    assert!(!absent.build_dependency_hash.is_empty());

    // A `.env` that exists and reads: a different hash, because the
    // environment is part of the key.
    std::fs::write(root.join(".env"), b"RUVYXA_PUBLIC_A=1\n").expect("write .env");
    let present = crate::load_project_config(root).expect("a readable .env loads");
    assert_ne!(
        present.build_dependency_hash, absent.build_dependency_hash,
        "the environment is part of the key, or a bundle survives the value changing",
    );

    // A `.env` that exists and cannot be read is a fault, not an empty map.
    // A directory in its place is the portable way to make the read fail:
    // permission bits are not, on Windows.
    std::fs::remove_file(root.join(".env")).expect("remove .env");
    std::fs::create_dir(root.join(".env")).expect("a directory where the file was");
    let error = crate::load_project_config(root)
        .expect_err("an unreadable .env must not be swallowed into an empty environment");
    let rendered = format!("{error:#}");
    assert!(
        rendered.to_ascii_lowercase().contains("env"),
        "the failure has to name what could not be read: {rendered}",
    );
}

/// The two normalisations the `cli_args` module doc has always promised.
///
/// The doc positions itself as the contract a reader consults before adding a
/// spelling, and it named three: `--root=x`, an em-dashed `—root`, and
/// `test-parity`. Only the first existed, so a reader debugging a rejected
/// `test-parity` would have gone looking for a bug in `normalize_command_arg`
/// rather than adding the alias.
#[test]
fn the_documented_argument_spellings_are_the_ones_that_resolve() {
    use std::ffi::OsString;

    assert_eq!(
        crate::cli_args::canonical_command_name("test-parity"),
        Some("test:parity"),
    );
    // The canonical spelling still resolves, and an unknown one still does not
    // -- clap's own error is better than this module guessing.
    assert_eq!(
        crate::cli_args::canonical_command_name("test:parity"),
        Some("test:parity"),
    );
    assert_eq!(crate::cli_args::canonical_command_name("test_parity"), None);

    // Smart punctuation, both substitutions.
    for dash in ["\u{2014}", "\u{2013}"] {
        assert_eq!(
            crate::cli_args::normalized_option_arg(&OsString::from(format!("{dash}root"))),
            Some("--root".to_string()),
            "a {dash:?}-prefixed option must resolve",
        );
        assert_eq!(
            crate::cli_args::normalized_option_arg(&OsString::from(format!("{dash}root=app"))),
            Some("--root=app".to_string()),
        );
    }

    // A value that merely begins with a dash is not an option and must not be
    // rewritten: the gate is that the name has to resolve.
    assert_eq!(
        crate::cli_args::normalized_option_arg(&OsString::from("\u{2014}not-an-option")),
        None,
    );
    assert_eq!(
        crate::cli_args::normalized_option_arg(&OsString::from("\u{2014}")),
        None,
    );
}

/// `clean` reports what it did, and does what it reports.
///
/// The config cache and the generated route types live at a hardcoded
/// `.ruvyxa/...` whatever `outDir` says, so a project that builds elsewhere had
/// both left behind while the command printed `removed`. `clean` is the escape
/// hatch a user reaches for when a cache has gone wrong, so a clean that did
/// not happen is the defect rather than the leftover bytes.
#[test]
fn clean_removes_the_framework_directory_when_the_build_goes_elsewhere() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    std::fs::write(
        root.join("ruvyxa.config.ts"),
        "export default { outDir: 'dist' }\n",
    )
    .expect("write config");
    std::fs::create_dir_all(root.join("app")).expect("app dir");

    let framework = root.join(".ruvyxa");
    std::fs::create_dir_all(framework.join("cache")).expect("cache dir");
    std::fs::create_dir_all(framework.join("types")).expect("types dir");
    std::fs::write(framework.join("cache").join("config-load.json"), b"{}").expect("write cache");
    std::fs::write(framework.join("types").join("routes.d.ts"), b"// types").expect("write types");
    let out = root.join("dist");
    std::fs::create_dir_all(&out).expect("out dir");
    std::fs::write(out.join("build.json"), b"{}").expect("write build");

    crate::commands::clean(crate::ProjectArgs {
        root: root.to_path_buf(),
        runtime: None,
    })
    .expect("clean must succeed");

    assert!(!out.exists(), "the configured out directory is removed");
    assert!(
        !framework.exists(),
        "the framework directory holds the config cache and the route types, and \
         `clean` said it removed everything",
    );
}

/// The same command on a default-`outDir` project removes it exactly once.
///
/// `.ruvyxa` is both the build output and the framework directory there, and a
/// second `remove_dir_all` of a path that is already gone would be an error
/// rather than a no-op.
#[test]
fn clean_removes_the_default_out_directory_only_once() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    std::fs::create_dir_all(root.join("app")).expect("app dir");
    let framework = root.join(".ruvyxa");
    std::fs::create_dir_all(&framework).expect("framework dir");
    std::fs::write(framework.join("build.json"), b"{}").expect("write build");

    crate::commands::clean(crate::ProjectArgs {
        root: root.to_path_buf(),
        runtime: None,
    })
    .expect("clean must succeed on a default out directory");
    assert!(!framework.exists());
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
            dependency_hash: &first_config.build_dependency_hash,
            directory: &cache_dir,
        },
    )
    .unwrap();
    let first_file = first_manifest["routes"][0]["file"].as_str().unwrap();
    let first_output = std::fs::read_to_string(client_dir.join(first_file)).unwrap();

    write_plugin("SecondRun");
    let second_config = load_project_config(root).unwrap();
    assert_ne!(
        first_config.build_dependency_hash,
        second_config.build_dependency_hash
    );
    let second_manifest = emit_client_bundles(
        root,
        &app,
        &manifest,
        &client_dir,
        &second_config.build,
        &second_config.plugins,
        RuvyxaBuildCache {
            dependency_hash: &second_config.build_dependency_hash,
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
            dependency_hash: &config.build_dependency_hash,
            directory: &build_cache_dir(root, &config.cache),
        },
        &session,
        &crate::client_bundle::ServerComponentEntries::default(),
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
        transformed_modules: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
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
        transformed_modules: Arc::new(Mutex::new(std::collections::BTreeSet::new())),
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
    use ruvyxa_tui::tui_header_title;

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

    copy_project_sources(root, &server, std::slice::from_ref(&source)).unwrap();

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

    copy_project_sources(root, &server, &[scanned, source]).unwrap();

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

/// A process id no operating system Ruvyxa builds on will hand out: Linux caps
/// `pid_max` far below it, Darwin lower still, and a Windows process id is a
/// multiple of four. It also stays positive when read as a signed `pid_t`, so a
/// Unix probe cannot mistake it for `kill`'s "every process" wildcard.
const UNREACHABLE_PID: u32 = 0x7fff_ffff;

/// A process that is certainly running and is certainly not this one.
#[cfg(windows)]
const RUNNING_FOREIGN_PID: u32 = 4; // The NT "System" process.
#[cfg(not(windows))]
const RUNNING_FOREIGN_PID: u32 = 1; // init / launchd.

/// A commit is two moves, and its rollback arm runs only on a returned `Err`.
/// A process killed between the moves therefore leaves `dist/` with none of the
/// named outputs and the previous build inside `.build-rollback-*`, which
/// nothing ever looked at again. The next commit has to find it.
#[test]
fn an_interrupted_commit_is_recovered_by_the_next_build() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("dist");
    let staging = temp.path().join("staging");
    fs::create_dir_all(out_dir.join("server")).unwrap();
    fs::write(out_dir.join("server").join("index.mjs"), "previous").unwrap();
    fs::create_dir_all(staging.join("server")).unwrap();
    fs::write(staging.join("server").join("index.mjs"), "next").unwrap();

    // The first half of a commit, then a simulated kill: the outputs are in the
    // rollback directory and `dist/` is empty.
    let backup = create_build_temp_dir(&out_dir, ".build-rollback").unwrap();
    move_named_build_outputs(&out_dir, &backup).unwrap();
    assert!(!out_dir.join("server").exists());

    commit_staged_build_outputs(&staging, &out_dir).unwrap();

    assert_eq!(
        fs::read_to_string(out_dir.join("server").join("index.mjs")).unwrap(),
        "next"
    );
    assert!(
        !has_temp_build_dir(&out_dir, ".build-rollback"),
        "a stale rollback directory was left behind"
    );
}

/// Deleting a stranded rollback directory is what makes the loss permanent, so
/// the sweep restores from it first. The marker names the outputs it holds, so
/// recovery does not have to guess.
#[test]
fn recovery_restores_the_outputs_a_dead_build_stranded() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("dist");
    let stranded = out_dir.join(format!(".build-rollback-{UNREACHABLE_PID}-1"));
    fs::create_dir_all(stranded.join("server")).unwrap();
    fs::write(stranded.join("server").join("index.mjs"), "previous").unwrap();
    fs::write(stranded.join("manifest.json"), "{\"routes\":[]}").unwrap();
    fs::write(
        stranded.join(".ruvyxa-rollback.json"),
        format!("{{\"pid\":{UNREACHABLE_PID},\"outputs\":[\"server\",\"manifest.json\"]}}"),
    )
    .unwrap();

    recover_stranded_build_outputs(&out_dir).unwrap();

    assert_eq!(
        fs::read_to_string(out_dir.join("server").join("index.mjs")).unwrap(),
        "previous"
    );
    assert_eq!(
        fs::read_to_string(out_dir.join("manifest.json")).unwrap(),
        "{\"routes\":[]}"
    );
    assert!(!has_temp_build_dir(&out_dir, ".build-rollback"));
}

/// Starting the server recovers a build the previous run left half-committed.
///
/// The commit moves twice: the existing output into a rollback directory, then
/// the staged build into place. A process killed between them leaves the only
/// complete build in the rollback directory, and until now only the *next*
/// `ruvyxa build` swept it back. So `ruvyxa start` on the same machine refused
/// with RUV1015 — "Build output was not found" — while a complete build sat one
/// directory away.
///
/// A start is when it matters most: whatever crashed was probably a deploy
/// step, and the server is what runs next.
#[test]
fn starting_the_server_recovers_a_half_committed_build() {
    let temp = tempfile::tempdir().expect("temp dir");
    let root = temp.path();
    std::fs::create_dir_all(root.join("app")).expect("app dir");

    // What a commit killed between its two moves leaves behind: no `server/`
    // in the output, and the previous build parked in a rollback directory
    // whose owner is gone.
    let out_dir = root.join(".ruvyxa");
    let stranded = out_dir.join(format!(".build-rollback-{UNREACHABLE_PID}-1"));
    std::fs::create_dir_all(stranded.join("server").join("app")).expect("mkdir");
    std::fs::write(
        stranded.join("server").join("app").join("page.mjs"),
        "export default 1\n",
    )
    .expect("write");
    std::fs::write(
        stranded.join(".ruvyxa-rollback.json"),
        format!("{{\"pid\":{UNREACHABLE_PID},\"outputs\":[\"server\"]}}"),
    )
    .expect("write marker");

    assert!(
        !out_dir.join("server").join("app").exists(),
        "the test has to start from the half-committed state",
    );

    let args = crate::ServerArgs {
        root: root.to_path_buf(),
        host: None,
        port: None,
        runtime: None,
    };
    let config = crate::ProjectConfig::default();

    crate::runtime_config::ensure_build_output_exists(&args, &config)
        .expect("a recoverable build must not be reported as missing");

    assert!(
        out_dir.join("server").join("app").join("page.mjs").exists(),
        "the previous build was left in the rollback directory",
    );
}

/// The production shape of a dead owner: a process that really ran and really
/// exited, rather than an id that never named anything. On Windows those are
/// different answers — a process that has exited can still be opened while a
/// handle to it is held, and only its exit code says it is gone.
#[test]
fn recovery_reclaims_a_directory_whose_owner_has_exited() {
    // `--list` makes the test binary print its test names and exit, which is
    // the cheapest real child process available on every platform this runs on.
    // The handle is held until the end of the test, both to exercise the
    // exit-code path and to keep the id from being recycled under the assert.
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .arg("--list")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let exited_pid = child.id();
    child.wait().unwrap();

    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("dist");
    let stranded = out_dir.join(format!(".build-rollback-{exited_pid}-1"));
    fs::create_dir_all(stranded.join("client")).unwrap();
    fs::write(stranded.join("client").join("app.js"), "previous").unwrap();

    recover_stranded_build_outputs(&out_dir).unwrap();

    assert_eq!(
        fs::read_to_string(out_dir.join("client").join("app.js")).unwrap(),
        "previous"
    );
    assert!(!has_temp_build_dir(&out_dir, ".build-rollback"));
    drop(child);
}

/// Two `ruvyxa build` invocations against one `dist/` already race, and a sweep
/// keyed on age rather than on a dead process id would make that race
/// destructive: it would restore a running build's backup over `dist/` and
/// delete the tree it is still writing into.
#[test]
fn recovery_leaves_a_running_builds_directories_alone() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("dist");
    let foreign_rollback = out_dir.join(format!(".build-rollback-{RUNNING_FOREIGN_PID}-1"));
    let own_staging = out_dir.join(format!(".build-staging-{}-1", std::process::id()));
    fs::create_dir_all(foreign_rollback.join("server")).unwrap();
    fs::write(foreign_rollback.join("server").join("index.mjs"), "theirs").unwrap();
    fs::create_dir_all(own_staging.join("server")).unwrap();
    fs::write(own_staging.join("server").join("index.mjs"), "mine").unwrap();

    recover_stranded_build_outputs(&out_dir).unwrap();

    assert!(foreign_rollback.join("server").join("index.mjs").exists());
    assert!(own_staging.join("server").join("index.mjs").exists());
    assert!(
        !out_dir.join("server").exists(),
        "a running build's backup must not be restored over its own output"
    );
}

/// The staging half of the sweep: every killed build otherwise leaves a full
/// partial tree inside `dist/` that nothing removes.
#[test]
fn recovery_removes_a_dead_builds_staging_tree() {
    let temp = tempfile::tempdir().unwrap();
    let out_dir = temp.path().join("dist");
    let stranded = out_dir.join(format!(".build-staging-{UNREACHABLE_PID}-1"));
    fs::create_dir_all(stranded.join("server")).unwrap();
    fs::write(stranded.join("server").join("index.mjs"), "partial").unwrap();

    recover_stranded_build_outputs(&out_dir).unwrap();

    assert!(!has_temp_build_dir(&out_dir, ".build-staging"));
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
        template_chain: Vec::new(),
        slots: Vec::new(),
        intercepts: Vec::new(),
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
    use crate::runtime_config::{CONFIG_CACHE_TOOLCHAIN, ConfigLoadCache, config_cache_is_current};
    use ruvyxa_dev_server::JavaScriptRuntime;
    use std::collections::BTreeMap;

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    fs::write(root.join("ruvyxa.config.ts"), "export default {}").unwrap();
    let config_hash = blake3::hash(b"export default {}").to_hex().to_string();

    let base = |mutate: &dyn Fn(&mut ConfigLoadCache)| {
        let mut cache = ConfigLoadCache {
            toolchain: CONFIG_CACHE_TOOLCHAIN.to_string(),
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
        !is_current(&base(&|cache| cache.toolchain.push_str("-next"))),
        "a cache written by another Ruvyxa must be discarded, not reinterpreted"
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
    use crate::runtime_config::{CONFIG_CACHE_TOOLCHAIN, ConfigLoadCache, config_cache_is_current};
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
        toolchain: CONFIG_CACHE_TOOLCHAIN.to_string(),
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

/// Cache identity is derived from the inputs, never stamped by hand.
///
/// The plan key used to be the literal `route-v2-manifest-<emitChunkManifest>`:
/// a counter somebody had to remember to raise, and a key that named one of the
/// eight options the plan is actually built from. Both halves failed quietly —
/// a forgotten bump served a plan the current bundler would not produce, and a
/// `jsx` change reused a plan whose module set no longer matched.
#[test]
fn a_client_plan_key_follows_every_option_that_shapes_it() {
    use crate::client_bundle::client_route_plan_variant;

    let base = BuildConfigOptions::default();
    let baseline = client_route_plan_variant(&base).expect("baseline variant");

    // No `vN` stamp survives in the key: the options are the identity.
    for stamp in ["-v1", "-v2", "-v3", ":v1", ":v2", ":v3"] {
        assert!(
            !baseline.contains(stamp),
            "plan key still carries a hand-maintained stamp: {baseline}"
        );
    }

    // Every option that reaches the bundler has to move the key. `jsx` is the
    // one that proves it matters rather than merely differs: the automatic
    // runtime imports `react/jsx-runtime` and the classic one does not, so the
    // two plans describe different module sets.
    for (name, mutate) in [
        (
            "jsx",
            (|options: &mut BuildConfigOptions| options.jsx_runtime = Some("classic".to_string()))
                as fn(&mut BuildConfigOptions),
        ),
        ("minify", |options| options.minify = Some(false)),
        ("treeShake", |options| options.tree_shaking = Some(false)),
        ("map", |options| options.sourcemap = Some(true)),
        ("split", |options| {
            options.split_strategy = Some("single".to_string())
        }),
        ("emitChunkManifest", |options| {
            options.emit_chunk_manifest = Some(true)
        }),
    ] {
        let mut changed = BuildConfigOptions::default();
        mutate(&mut changed);
        assert_ne!(
            baseline,
            client_route_plan_variant(&changed).expect("variant"),
            "changing `{name}` must change the plan cache key"
        );
    }
}

/// The build only warns about on-demand images when it has something true to say.
///
/// `image.onDemand` was recorded as an unconditional gap between the two hosts,
/// and then two adapters started forwarding the request to their platform's own
/// optimizer. The capability became adapter-conditional and the report could not
/// say so, so `ruvyxa build` told a working Vercel deployment that its images
/// answered 404, `ruvyxa test:parity` agreed, and both language guides said the
/// same while the deployment matrix two chapters away already said
/// "adapter-dependent".
///
/// Asserted on the decision rather than on the contract, because the reader and
/// the contract are the same file: comparing them passes whatever the file says,
/// which is what the first draft of this test did. `tests/packages/ruvyxa/
/// serverless-shared-tables.test.mjs` holds the table to the adapter sources
/// that actually decide it, and `scripts/sync-adapters.mjs` fails when either
/// language guide drifts from it.
#[test]
fn the_on_demand_image_report_follows_the_adapter() {
    let quiet = ProjectConfig::default();
    assert!(!quiet.images.on_demand.enabled());
    assert!(
        !crate::build::should_report_native_only_images(&quiet, Some("node")),
        "a project that never enabled the option has nothing to be warned about"
    );

    let mut enabled = ProjectConfig::default();
    enabled.images.on_demand = crate::image_optimizer::OnDemandImageOptions::Enabled(true);

    assert!(
        crate::build::should_report_native_only_images(&enabled, Some("node")),
        "the node artifact carries no image pipeline, so /__ruvyxa/image answers 404 there"
    );
    for adapter in ["vercel", "cloudflare"] {
        assert!(
            !crate::build::should_report_native_only_images(&enabled, Some(adapter)),
            "{adapter} forwards to its platform optimizer, so warning tells the reader to \
             disable a feature that works"
        );
    }

    // Neither an object-form `adapter:` nor a third-party package gives a name
    // the table knows, and a wrong answer errs in one direction: saying nothing
    // costs one full-size download, while claiming a working deployment is
    // broken sends the reader to turn the feature off.
    assert!(!crate::build::should_report_native_only_images(
        &enabled, None
    ));
    assert!(!crate::build::should_report_native_only_images(
        &enabled,
        Some("@acme/ruvyxa-adapter-unknown")
    ));
}

/// The store a deployment reads documents from is named in the build output.
///
/// `cache.handler` changes where a deployed page comes from, and the alternative
/// to one line here is finding that out from behaviour: after this build a stale
/// document is the handler's to explain rather than the platform's.
///
/// The trimming is not cosmetic. Serde accepts whatever the config file holds,
/// and a key present but empty is a project that has not configured a handler —
/// reporting one would name a store no request will ever reach.
#[test]
fn a_configured_document_store_is_named() {
    let read = |value: Option<&str>| {
        let config: crate::config::ProjectConfig = serde_json::from_value(serde_json::json!({
            "cache": match value {
                Some(handler) => serde_json::json!({ "handler": handler }),
                None => serde_json::json!({}),
            }
        }))
        .expect("the fixture config must deserialize");
        crate::build::project_document_store(&config).map(str::to_string)
    };

    assert_eq!(
        read(Some("./cache-handler.mjs")),
        Some("./cache-handler.mjs".to_string())
    );
    assert_eq!(
        read(Some("  ./spaced.mjs  ")),
        Some("./spaced.mjs".to_string()),
        "the reported name is the one the runner will resolve"
    );
    assert_eq!(
        read(Some("   ")),
        None,
        "a blank handler is not a configured store"
    );
    assert_eq!(
        read(None),
        None,
        "a project that declares nothing reports nothing"
    );
}

/// Turning the in-memory tier off is reported, because it is not a small cache.
///
/// `0` means every `cache()` read reaches the shared store. Behind one that is
/// the right trade; in front of nothing it is a deployment that made every read
/// a miss and has no message saying so.
#[test]
fn a_disabled_local_cache_is_reported() {
    let read = |value: serde_json::Value| {
        let config: crate::config::ProjectConfig =
            serde_json::from_value(serde_json::json!({ "cache": value }))
                .expect("the fixture config must deserialize");
        crate::build::local_cache_is_disabled(&config)
    };

    assert!(read(serde_json::json!({ "maxEntries": 0 })));
    assert!(
        !read(serde_json::json!({ "maxBytes": 0 })),
        "maxBytes: 0 removes the memory ceiling and leaves the entry bound in charge,          which is a larger cache rather than none"
    );
    assert!(
        !read(serde_json::json!({ "maxEntries": 1 })),
        "a bound of one is a very small cache, not a disabled one"
    );
    assert!(
        !read(serde_json::json!({})),
        "a project that declares nothing keeps the default tier"
    );
}

/// A platform config the adapter declined to write is named, not swallowed.
///
/// `skipIfExists` defers to the file the project keeps under version control,
/// which is the right default and was the whole of the behaviour: the build
/// reported an artifact count and the `skipped: true` the runner had been
/// recording went unread. A `vercel.json` predating a framework change then
/// kept a deployment on the old routing with no evidence anywhere in the build.
#[test]
fn a_skipped_platform_config_is_reported() {
    use crate::config::AdapterArtifactReport;

    let report = |path: &str, skipped: Option<bool>| AdapterArtifactReport {
        kind: "file".to_string(),
        path: path.to_string(),
        scope: Some("project".to_string()),
        skipped,
    };

    let artifacts = vec![
        report("vercel.json", Some(true)),
        report(".vercel/output/config.json", None),
        report("netlify.toml", Some(false)),
    ];
    assert_eq!(
        crate::build::skipped_platform_config_paths(&artifacts),
        vec!["vercel.json"],
        "only an artifact the runner marked skipped is named"
    );

    assert!(
        crate::build::skipped_platform_config_paths(&[]).is_empty(),
        "a build that skipped nothing reports nothing"
    );
}

/// The build says so when `revalidatePath()` cannot reach a reader.
///
/// An ISR page is served from a cache, and which cache decides what
/// `revalidatePath()` can do. Where the store the function writes is the copy a
/// reader gets — a Worker's KV, a standalone server's filesystem — a forced
/// write is the whole revalidation. Where the platform caches the *response*
/// instead, the function has to ask the platform to drop that path, and two
/// adapters have no way to: Firebase Hosting and Amplify's CloudFront both cache
/// on the `s-maxage` the handler sends and give code inside no purge.
///
/// The forced write still succeeds there, and nothing a reader can see changes
/// until the window expires on its own. Reported rather than refused, exactly
/// like the image report beside it: such a deployment is still valid, and a
/// project may never call `revalidatePath()` at all.
#[test]
fn the_revalidation_report_follows_the_adapter() {
    assert!(
        !crate::build::should_report_stale_revalidation(false, Some("firebase")),
        "a project with no incrementally-regenerated route has nothing to be warned about"
    );

    for adapter in ["firebase", "aws"] {
        assert!(
            crate::build::should_report_stale_revalidation(true, Some(adapter)),
            "{adapter} caches the response and exposes no purge, so a forced write reaches \
             the store and not the reader"
        );
    }

    for adapter in [
        "vercel",
        "netlify",
        "cloudflare",
        "node",
        "bun",
        "deno",
        "railway",
        "render",
    ] {
        assert!(
            !crate::build::should_report_stale_revalidation(true, Some(adapter)),
            "{adapter} either writes the cache a reader is served from or implements the \
             platform's own purge"
        );
    }

    // The same rule as the image report: an unnamed or third-party adapter is
    // not in the table, and claiming a working deployment is broken is the
    // worse of the two wrong answers.
    assert!(!crate::build::should_report_stale_revalidation(true, None));
    assert!(!crate::build::should_report_stale_revalidation(
        true,
        Some("@acme/ruvyxa-adapter-unknown")
    ));
}

/// A route's `flight` export has to survive a build that runs from elsewhere.
///
/// `ClientBundle::entry` is the route file as the published manifest carries
/// it — project-relative, because a machine-specific path in `entry` would make
/// two machines emit different bytes. Reading it therefore resolves against the
/// process working directory, and every `ruvyxa build --root <elsewhere>` read
/// nothing at all. `unwrap_or_default()` turned that into "no `flight` export,
/// no `'use cache'`" for *every* route: the shipped route manifest said
/// `flight: false` throughout, so the browser router never asked for a payload
/// any route produced and fell back to a document load, and RUV1842 could not
/// fire. A build run from inside the project directory was correct, which is how
/// it stayed invisible — and no fixture in this repository exported `flight`,
/// so nothing looked.
///
/// The temporary root here is never the test process's working directory, which
/// is what makes this the failing case rather than the passing one.
#[test]
fn a_flight_export_is_recorded_when_the_build_runs_from_another_directory() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    let client_dir = root.join(".ruvyxa").join("client");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::create_dir_all(&client_dir).unwrap();
    std::fs::write(root.join("ruvyxa.config.mjs"), "export default {}\n").unwrap();
    std::fs::write(
        app.join("page.tsx"),
        "export function flight() {\n  return { ok: true }\n}\n\
         export default function Page() {\n  return <main>hello</main>\n}\n",
    )
    .unwrap();

    assert_ne!(
        ruvyxa_diagnostics::normalized_canonical_path(root),
        ruvyxa_diagnostics::normalized_canonical_path(
            &std::env::current_dir().expect("a working directory")
        ),
        "this test only bites while the project root is not the working directory"
    );

    let config = load_project_config(root).unwrap();
    let mut manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();

    // `ruvyxa build --root <dir>` discovers through a relative app directory, so
    // every `RouteEntry::file` it produces is project-relative — and that is the
    // path the writer has to resolve. Discovering through an absolute temporary
    // directory produces absolute files, which read correctly against any
    // working directory and so cannot fail. Rewriting them to the shape the CLI
    // actually builds is what makes this the failing case.
    let canonical_root = ruvyxa_diagnostics::normalized_canonical_path(root);
    for route in &mut manifest.routes {
        let relative = ruvyxa_diagnostics::normalized_canonical_path(&route.file);
        route.file = relative
            .strip_prefix(&canonical_root)
            .expect("a discovered route lives under the project root")
            .to_path_buf();
        assert!(route.file.is_relative());
    }

    let client_manifest = emit_client_bundles(
        root,
        &app,
        &manifest,
        &client_dir,
        &config.build,
        &config.plugins,
        RuvyxaBuildCache {
            dependency_hash: &config.build_dependency_hash,
            directory: &build_cache_dir(root, &config.cache),
        },
    )
    .unwrap();

    assert_eq!(
        client_manifest["routes"][0]["flight"],
        serde_json::Value::Bool(true),
        "the shipped manifest must tell the browser router this route has a payload: {client_manifest}"
    );
}

/// A route manifest that cannot be parsed must not be replaced by an empty one.
///
/// `write_style_asset` reads `route-manifest.json` to add the stylesheet URL,
/// then writes the whole document back. Parsing it with a default of
/// `{"routes": []}` meant an unparseable manifest was silently *overwritten*
/// with one naming no routes at all -- and that file is what every host reads to
/// find a route's scripts: the Rust server, the generated standalone server, and
/// each adapter's function bundle. The build still succeeded, so the first
/// symptom was a deployed site whose client router knew no routes and answered
/// every navigation with a full document load.
///
/// It is reachable without any corruption on disk: a build interrupted mid-write
/// and two builds sharing one output directory both leave a partial file, and
/// this repository has already had to isolate concurrent build outputs once.
#[test]
fn an_unparseable_route_manifest_fails_the_build_instead_of_emptying_itself() {
    let temp = tempfile::tempdir().unwrap();
    let client_dir = temp.path().join("client");
    std::fs::create_dir_all(&client_dir).unwrap();
    let manifest_path = client_dir.join("route-manifest.json");
    let truncated = r#"{"routes":[{"path":"/","src":"/__ruvyxa/client/entry.js"}"#;
    std::fs::write(&manifest_path, truncated).unwrap();

    let error = write_style_asset(&client_dir, "body{color:red}")
        .expect_err("an unreadable manifest is a build failure, not an empty route table");
    assert!(
        error.to_string().contains("route-manifest.json"),
        "the failure has to name the file a reader must repair: {error}"
    );

    assert_eq!(
        std::fs::read_to_string(&manifest_path).unwrap(),
        truncated,
        "the damaged manifest must be left for inspection, never replaced with an empty one"
    );
}

// ─── The config surface: one object, three descriptions ──────────────────────

/// The key planted to make a struct name the fields it accepts.
///
/// `deny_unknown_fields` reports an unknown key by listing every known one, so
/// this is how the Rust field set is read rather than transcribed. It is
/// deliberately unwritable as a config option: if a field ever were named this,
/// the probe would deserialize successfully and the test fails loudly rather
/// than comparing an empty list.
const CONFIG_SURFACE_PROBE_KEY: &str = "__ruvyxa_config_surface_probe__";

/// Deserialize a probe document into the struct the fixture names.
///
/// The match is the only hand-written part of the probe, and it maps a fixture
/// `rustType` to a type — never to a field list. Four of the five entries exist
/// because the section sits behind an `#[serde(untagged)]` wrapper
/// (`OnDemandImageOptions`, `SitemapSetting`, `RobotsSetting`, `OneOrManyRules`)
/// and untagged deserialization buffers the content and reports only
/// "data did not match any variant", discarding the inner field list.
fn deserialize_config_probe(rust_type: &str, value: serde_json::Value) -> Option<String> {
    fn outcome<T: serde::de::DeserializeOwned>(value: serde_json::Value) -> Option<String> {
        serde_json::from_value::<T>(value)
            .err()
            .map(|e| e.to_string())
    }
    match rust_type {
        "ProjectConfig" => outcome::<ProjectConfig>(value),
        "OnDemandImageConfigOptions" => {
            outcome::<crate::image_optimizer::OnDemandImageConfigOptions>(value)
        }
        "SitemapGenerationOptions" => {
            outcome::<crate::site_discovery::SitemapGenerationOptions>(value)
        }
        "RobotsGenerationOptions" => {
            outcome::<crate::site_discovery::RobotsGenerationOptions>(value)
        }
        "RobotsRuleOptions" => outcome::<crate::site_discovery::RobotsRuleOptions>(value),
        other => panic!(
            "tests/fixtures/config-surface-conformance.json names `{other}`, which this probe \
             cannot deserialize. Add it to `deserialize_config_probe` — never transcribe its \
             field list into the fixture by hand."
        ),
    }
}

/// Wrap the probe key in the nesting the fixture's `rustPath` describes.
///
/// A `[]` suffix means the segment is a sequence, and the probe goes into
/// element 0 — that is the shape `entries[]` and `videos[]` are read in.
fn config_probe_document(path: &[String]) -> serde_json::Value {
    let mut value = serde_json::json!({ CONFIG_SURFACE_PROBE_KEY: true });
    for segment in path.iter().rev() {
        let mut map = serde_json::Map::new();
        match segment.strip_suffix("[]") {
            Some(name) => {
                map.insert(name.to_string(), serde_json::Value::Array(vec![value]));
            }
            None => {
                map.insert(segment.clone(), value);
            }
        }
        value = serde_json::Value::Object(map);
    }
    serde_json::Value::Object(match value {
        serde_json::Value::Object(map) => map,
        other => panic!("a probe document is always an object, got {other}"),
    })
}

/// Read the field names out of a `deny_unknown_fields` error.
///
/// serde writes one of three shapes — "there are no fields", "expected `a`",
/// and "expected one of `a`, `b`" — and every field name in all three is
/// delimited by backticks, which no Rust identifier and no `#[serde(rename)]`
/// in this crate contains.
fn config_fields_from_error(message: &str) -> Vec<String> {
    let listed = message
        .split_once(", expected")
        .map(|(_, rest)| rest)
        .unwrap_or("");
    listed
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// `ruvyxa.config` has three descriptions of one object, and only two were held.
///
/// `ProjectConfig` here decides what the compiler reads, `CONFIG_KEY_SCHEMA` in
/// `packages/ruvyxa/runtime/config-schema.mjs` decides what the config renderer
/// accepts, and `RuvyxaConfig` in `@ruvyxa/core` decides what TypeScript
/// accepts. `tests/packages/core/config-schema.test.ts` held the second against
/// the third and nothing at all compared the first — so `image.maxWidth`, which
/// Rust has always read as `pub max_width: u32` and the public type has always
/// declared, was missing from the schema and from the hand-written literal that
/// test compares against. Both gated descriptions agreed with each other and
/// disagreed with Rust, so the gate saw nothing, and `assertKnownKeys` threw
/// `RUV1602 unknown config.image field: maxWidth` before any command could run.
///
/// This is the missing third edge. `tests/fixtures/config-surface-conformance.json`
/// is the shared table: this test asserts it equals what serde accepts, and
/// `config-schema.test.ts` asserts `CONFIG_KEY_SCHEMA` equals it in both
/// directions. Neither language can grow, rename, or drop a config key alone.
///
/// A *new nested section* is caught by the same pair without needing its own
/// entry first: the struct that contains it gains a field, so this test fails on
/// the parent's list until the fixture names it, and the schema test then fails
/// until `CONFIG_KEY_SCHEMA` does.
#[test]
fn config_surface_matches_the_rust_config() {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Section {
        rust_type: Option<String>,
        #[serde(default)]
        rust_path: Vec<String>,
        probe: String,
        fields: Vec<String>,
        #[serde(default)]
        reason: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct Fixture {
        sections: std::collections::BTreeMap<String, Section>,
    }

    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../tests/fixtures/config-surface-conformance.json"
    ))
    .expect("tests/fixtures/config-surface-conformance.json");
    assert!(
        fixture.sections.contains_key("config"),
        "the fixture must describe the config root"
    );

    // `probe: "none"` is the one place a field list is written by hand, so the
    // set of sections allowed to use it is fixed here rather than left to the
    // fixture. Otherwise the way to silence a divergence this test finds is to
    // stop probing the section — which looks like data and reads as a decision.
    // Both entries are blocks Rust forwards whole and never types; that claim is
    // asserted by `the_unprobed_config_sections_are_the_ones_rust_forwards_whole`.
    let unprobed = fixture
        .sections
        .iter()
        .filter(|(_, section)| section.probe == "none")
        .map(|(path, _)| path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        unprobed,
        vec!["config.content", "config.content.engine", "config.markdown"],
        "only the config blocks Rust reads as opaque JSON may go unprobed"
    );

    for (path, section) in &fixture.sections {
        let mut expected = section.fields.clone();
        expected.sort();
        assert_eq!(
            expected, section.fields,
            "{path}: the fixture's field list is written in code-unit order, so both \
             languages read the same table"
        );

        // A section whose parent does not declare it describes an object no
        // config can reach, which is how a stale entry survives a rename.
        if let Some((parent, own)) = path.rsplit_once('.') {
            let own = own.strip_suffix("[]").unwrap_or(own);
            let parent = fixture
                .sections
                .get(parent)
                .unwrap_or_else(|| panic!("{path}: no fixture section describes `{parent}`"));
            assert!(
                parent.fields.iter().any(|field| field == own),
                "{path}: `{parent_path}` does not declare `{own}`, so nothing can reach it",
                parent_path = path.rsplit_once('.').expect("checked above").0
            );
        }

        match section.probe.as_str() {
            "serde" => {
                let rust_type = section
                    .rust_type
                    .as_deref()
                    .unwrap_or_else(|| panic!("{path}: a serde probe needs a `rustType`"));
                let document = config_probe_document(&section.rust_path);
                let error = deserialize_config_probe(rust_type, document).unwrap_or_else(|| {
                    panic!(
                        "{path}: `{rust_type}` accepted `{CONFIG_SURFACE_PROBE_KEY}`. Either it \
                         lost `deny_unknown_fields` — in which case a misspelled config key is \
                         now silently ignored — or a real field is named that."
                    )
                });
                let mut actual = config_fields_from_error(&error);
                actual.sort();
                assert_eq!(
                    actual, section.fields,
                    "{path}: `{rust_type}` and the fixture describe different fields. \
                     A field only Rust accepts is refused by RUV1602 before any command runs; \
                     a field only the fixture (and so the schema) has is accepted and then \
                     ignored. serde reported: {error}"
                );
            }
            "none" => {
                assert!(
                    section.rust_type.is_none(),
                    "{path}: an unprobed section must not claim a Rust type"
                );
                assert!(
                    !section.reason.is_empty(),
                    "{path}: a section Rust does not describe has to say why, or the next \
                     reader cannot tell it from an omission"
                );
            }
            other => panic!("{path}: unknown probe `{other}`"),
        }
    }
}

/// The two blocks Rust deliberately does not describe are the two it forwards.
///
/// `probe: "none"` in the fixture is the only place a field list is written by
/// hand, so it is exactly where a wrong entry would go unnoticed. It is correct
/// only while Rust really does read the block as one opaque value: the moment
/// either grows a typed struct, the field list stops being the renderer's alone
/// and has to be probed like every other section.
#[test]
fn the_unprobed_config_sections_are_the_ones_rust_forwards_whole() {
    let config: ProjectConfig = serde_json::from_value(serde_json::json!({
        "markdown": true,
        "content": { "engine": { "anything": ["at", "all"] } },
    }))
    .expect("Rust accepts both blocks without knowing their shape");

    assert_eq!(
        config.markdown_enabled,
        Some(true),
        "`markdown` reaches Rust as the single boolean the renderer collapses it to"
    );
    assert_eq!(
        config._content,
        Some(serde_json::json!({ "engine": { "anything": ["at", "all"] } })),
        "`content` reaches Rust as opaque JSON, so its keys are the renderer's to police"
    );
}

/// The reproduction path from the finding, end to end through the renderer.
///
/// `image.maxWidth` was documented, typed in `RuvyxaConfig`, and read by Rust as
/// `pub max_width: u32` — and `CONFIG_KEY_SCHEMA` did not list it, so
/// `assertKnownKeys` threw `RUV1602 unknown config.image field: maxWidth` while
/// the config was still being rendered. Every command loads the config first, so
/// a project that set the documented option could not run any of them.
///
/// The unit test below deserializes `ProjectConfig` directly and so cannot see
/// that: the renderer is the half that refused. This one writes the config a
/// project would write and runs the whole load.
#[test]
fn a_rendered_config_carries_image_max_width_to_the_optimizer() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("app")).unwrap();
    std::fs::write(
        root.join("ruvyxa.config.ts"),
        "import { config } from \"ruvyxa/config\"\n\
         \n\
         export default config({ image: { maxWidth: 1920 } })\n",
    )
    .unwrap();

    let config = load_project_config(root)
        .expect("a config setting the documented `image.maxWidth` must load");
    assert_eq!(
        config.images.max_width, 1920,
        "the rendered config has to carry the value the project wrote, not the default"
    );
}

/// `image.maxWidth` reaches the field for every value the option documents.
///
/// The finding was not that the number was wrong — Rust has always read this
/// key — but that no config carrying it ever got as far as being deserialized.
/// `0` is the documented "publish the source's own resolution" escape hatch and
/// is the case a projection that drops falsy values silently turns back into
/// the 3840 default, so it is the one asserted.
#[test]
fn the_configured_image_max_width_reaches_the_optimizer() {
    let default: ProjectConfig = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(default.images.max_width, 3840);

    for width in [0_u32, 1920, 6000] {
        let config: ProjectConfig =
            serde_json::from_value(serde_json::json!({ "image": { "maxWidth": width } }))
                .expect("`image.maxWidth` is a config option, not an unknown field");
        assert_eq!(
            config.images.max_width, width,
            "a project that writes `image.maxWidth: {width}` must get {width}"
        );
        config
            .validate_paths()
            .unwrap_or_else(|error| panic!("`image.maxWidth: {width}` must validate: {error}"));
    }
}

// ─── CLI build pipeline: report placement, worker lifetime, artifact reads ────

/// The client build report is not a browser asset, and `client/` is public by
/// contract.
///
/// It carries the absolute source paths of the build machine, the module graph
/// of every shared chunk and route, the bundler cache location, the configured
/// plugin list, and per-route byte counts — and it was written into the one
/// directory the native server maps every flat `/__ruvyxa/client/<name>`
/// request into and every static deployment copies wholesale to a CDN. The lean
/// `route-manifest.json` beside it exists precisely so none of that has to
/// ship.
///
/// Excluding it from the copy instead would have left three hosts to keep in
/// step; a file outside the published directory cannot be published by any of
/// them.
#[test]
fn the_client_build_report_is_written_outside_the_public_client_directory() {
    let temp = tempfile::tempdir().unwrap();
    let staging = temp.path();
    let client_dir = staging.join("client");
    fs::create_dir_all(&client_dir).unwrap();

    let client_manifest = json!({
        "routes": [{
            "path": "/",
            "src": "/__ruvyxa/client/home.abc.js",
            "modules": [staging.join("app").join("page.tsx").to_string_lossy()],
            "sharedChunks": [{ "src": "/__ruvyxa/client/shared.def.js" }],
        }],
        "cache": { "directory": staging.join("cache").to_string_lossy() },
    });

    let written = write_client_build_report(staging, &client_manifest).unwrap();

    assert_eq!(written, staging.join("client-report.json"));
    assert!(
        !client_dir.join("manifest.json").exists(),
        "the build report must not be emitted into the directory every host publishes"
    );
    assert_eq!(
        fs::read_dir(&client_dir).unwrap().count(),
        0,
        "nothing about this report belongs under a public URL"
    );
    let report = fs::read_to_string(&written).unwrap();
    assert!(
        report.contains("page.tsx"),
        "the report still carries absolute build-machine paths — which is why it moved: {report}"
    );

    // The pre-renderer reads it from its new home.
    let assets = load_prerender_client_assets(&written).unwrap();
    assert_eq!(assets["/"].src, "/__ruvyxa/client/home.abc.js");
    assert_eq!(
        assets["/"].preloads,
        vec!["/__ruvyxa/client/shared.def.js".to_string()]
    );

    // And the commit carries it into the output directory, or the deployed
    // function that reads it from there finds nothing.
    assert!(
        BUILD_OUTPUT_FILES.contains(&CLIENT_BUILD_REPORT_FILE),
        "a build output nobody commits is a build output nobody has"
    );
}

/// A warm build must read each cached artifact once.
///
/// The pre-scan that decided whether to start a Node process answered its
/// question by doing the work: it called `load_prerender_artifact` for every
/// job — a whole-file read and a JSON parse whose `html` field is the entire
/// rendered document — and discarded the answer, and the render then loaded it
/// again. `any()` short-circuits on the first *miss*, so a cold build paid one
/// extra load and a fully warm build paid one per page: the entire cost landed
/// on exactly the case the pre-scan existed to make fast.
///
/// The property the pre-scan protected is asserted here too: a build whose
/// every render is a cache hit starts no Node process at all.
#[tokio::test]
async fn a_fully_warm_prerender_reads_each_artifact_once_and_starts_no_worker() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("about")).unwrap();
    let home_file = app_dir.join("page.tsx");
    let about_file = app_dir.join("about").join("page.tsx");
    fs::write(&home_file, "export default () => 'home'").unwrap();
    fs::write(&about_file, "export default () => 'about'").unwrap();

    let pages = [("/", home_file.clone()), ("/about", about_file.clone())];
    let manifest = ruvyxa_graph::RouteManifest {
        app_dir: PathBuf::from("app"),
        routes: pages
            .iter()
            .map(|(path, file)| ruvyxa_graph::RouteEntry {
                id: format!("app{path}/page"),
                path: (*path).to_string(),
                kind: ruvyxa_graph::RouteKind::Page,
                file: file.clone(),
                layout_chain: Vec::new(),
                template_chain: Vec::new(),
                slots: Vec::new(),
                intercepts: Vec::new(),
                server_modules: Vec::new(),
                client_modules: Vec::new(),
                runtime: ruvyxa_graph::RuntimeTarget::Node,
                render: ruvyxa_graph::RenderMeta {
                    strategy: RenderStrategy::Ssg,
                    ..ruvyxa_graph::RenderMeta::default()
                },
            })
            .collect(),
        i18n: None,
    };

    let build = BuildConfigOptions::default();
    let runtime = JavaScriptRuntime::Node;
    let cache_dir = root.join("cache");
    let prerender_dir = root.join("prerender");
    let client_report = root.join(CLIENT_BUILD_REPORT_FILE);
    fs::write(&client_report, r#"{"routes":[]}"#).unwrap();
    let head = PrerenderHead {
        asset_links: "".into(),
        styles: "".into(),
        plugin_head: "".into(),
        shell: CsrShell::default(),
    };

    // The same cache identity the phase derives, so the artifacts stored here
    // are the ones it validates: anything else is a miss, and a miss needs a
    // Node process this test deliberately does not give it.
    let worker_env = build_worker_env(root, &build, runtime).unwrap();
    let artifact_cache = PrerenderArtifactCache {
        directory: cache_dir.clone(),
        dependency_hash: "dependency-v1".to_string(),
        render_context_hash: prerender_context_hash(
            root,
            &head,
            &load_prerender_client_assets(&client_report).unwrap(),
            &build,
            &worker_env,
        ),
        fingerprints: Arc::new(ArtifactFingerprintCache::default()),
        enabled: true,
    };
    for (path, file) in &pages {
        store_prerender_artifact(
            &artifact_cache,
            &PrerenderJob {
                route_path: (*path).to_string(),
                render_path: (*path).to_string(),
                params: RouteParams::new(),
                strategy: RenderStrategy::Ssg,
                revalidate: None,
                kind: PrerenderJobKind::Render {
                    route_file: file.clone(),
                    mode: "full",
                    server_components: false,
                },
            },
            "renderer-v1",
            std::slice::from_ref(file),
            &format!("<main>{path}</main>"),
        );
    }

    PRERENDER_ARTIFACT_READS.store(0, std::sync::atomic::Ordering::Relaxed);
    let prerendered = prerender_static_routes(
        root,
        &app_dir,
        &manifest,
        &prerender_dir,
        &client_report,
        head.clone(),
        &build,
        RuvyxaBuildCache {
            dependency_hash: "dependency-v1",
            directory: &cache_dir,
        },
        runtime,
        false,
        None,
    )
    .await
    .expect("a fully cached pre-render needs no worker and cannot fail");

    assert_eq!(prerendered.len(), 2);
    assert!(
        prerendered.iter().all(|route| route.artifact_cache_hit),
        "the test's own cache identity does not match the phase's: {prerendered:?}"
    );
    assert_eq!(
        PRERENDER_ARTIFACT_READS.load(std::sync::atomic::Ordering::Relaxed),
        pages.len(),
        "a warm build must read each cached artifact once, not twice"
    );
    assert_eq!(
        fs::read_to_string(prerender_dir.join("index.html")).unwrap(),
        "<main>/</main>"
    );
}

/// A client report that is present but unusable stops the build; a report that
/// is simply absent does not.
///
/// The three answers are not one answer. An empty asset map makes
/// `inject_prerender_client_assets` hand back the document unchanged, so a
/// damaged report produced pages with no bootstrap block and no
/// `<script type="module">` at all — on a build that reported success. Nothing
/// downstream can see it either: `output_audit` looks for references that do
/// not resolve, and a document that references *nothing* has none. The deployed
/// site then renders correctly and is completely non-interactive.
///
/// `write_style_asset` was hardened against the same shape with the same
/// answer, and `prebuilt_client_assets` in `html_document.rs` and
/// `read_cache_observation` in `bench.rs` already separate absent from
/// unreadable for this very file. This was the reader that did not.
#[test]
fn an_unusable_client_report_fails_the_prerender_phase_instead_of_dropping_hydration() {
    let temp = tempfile::tempdir().unwrap();
    let client_report = temp.path().join(CLIENT_BUILD_REPORT_FILE);

    // A build interrupted mid-write, or two builds sharing an output directory.
    fs::write(&client_report, r#"{"routes":[{"path":"/","src":"/a.j"#).unwrap();
    let error = load_prerender_client_assets(&client_report)
        .expect_err("a truncated client report is not a project without client bundles");
    let message = format!("{error:#}");
    assert!(
        message.contains(CLIENT_BUILD_REPORT_FILE),
        "the error must name the file to delete and rebuild: {message}"
    );

    // Valid JSON that is not this document is the same class of answer: the
    // routes table is what the map is built from, so a file without one cannot
    // be read as "no routes ship a bundle".
    fs::write(&client_report, r#"{"cacheHits":3}"#).unwrap();
    assert!(
        load_prerender_client_assets(&client_report).is_err(),
        "a report with no routes table is unusable, not empty"
    );

    // The one legal absence: a build that emitted no client bundle at all.
    fs::remove_file(&client_report).unwrap();
    assert!(
        load_prerender_client_assets(&client_report)
            .expect("a missing report is a legal state")
            .is_empty()
    );
}

/// The cell is what `shutdown()` reaches, and a pool nobody needed was never
/// started.
///
/// `NodeWorkerPool` has no `Drop` impl, and its `retiring` field exists because
/// a worker that crossed its render budget is no longer in `workers`: its
/// `Child` is owned by a detached drain task that only `shutdown()` joins. The
/// cell is therefore the single owner — an un-started cell has nothing to
/// close, and an adopted pool is closed by the phase that adopted it rather
/// than by the caller that started it.
#[tokio::test]
async fn an_unstarted_worker_pool_has_nothing_to_shut_down() {
    let pool = LazyPrerenderWorkerPool::new(
        Path::new("/nonexistent"),
        BTreeMap::new(),
        1,
        JavaScriptRuntime::Node,
    );

    assert!(!pool.started());
    pool.shutdown().await;
    assert!(!pool.started(), "shutting down must not start anything");
}

/// A server-components pass whose every route is answered from the cache starts
/// no worker, so there is no pool to leak on the way out.
///
/// The three exits that used to skip `shutdown()` are all on the far side of
/// this early return; the return itself is what keeps a warm build off Node
/// entirely, and nothing else covers it.
#[tokio::test]
async fn a_fully_cached_server_component_pass_starts_no_worker() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app_dir = root.join("app");
    fs::create_dir_all(app_dir.join("live")).unwrap();
    let page = app_dir.join("live").join("page.tsx");
    fs::write(&page, "export const serverComponents = true").unwrap();

    let manifest = ruvyxa_graph::RouteManifest {
        app_dir: PathBuf::from("app"),
        routes: vec![ruvyxa_graph::RouteEntry {
            id: "app/live/page".to_string(),
            path: "/live".to_string(),
            kind: ruvyxa_graph::RouteKind::Page,
            file: page.clone(),
            layout_chain: Vec::new(),
            template_chain: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
            server_modules: Vec::new(),
            client_modules: Vec::new(),
            runtime: ruvyxa_graph::RuntimeTarget::Node,
            render: ruvyxa_graph::RenderMeta {
                strategy: RenderStrategy::Ssr,
                server_components: true,
                ..ruvyxa_graph::RenderMeta::default()
            },
        }],
        i18n: None,
    };

    let cache = ServerComponentEntryCache {
        directory: root.join("cache"),
        dependency_hash: "dependency-v1".to_string(),
        context_hash: "worker-v1".to_string(),
        fingerprints: Arc::new(ArtifactFingerprintCache::default()),
    };
    store_server_component_entry(
        &cache,
        "/live",
        std::slice::from_ref(&page),
        "export default 1",
        &[],
    );

    let entries = collect_server_component_entries(
        root,
        &app_dir,
        &manifest,
        &BuildConfigOptions::default(),
        JavaScriptRuntime::Node,
        Some(&cache),
    )
    .await
    .expect("a cached entry must not need a worker");

    assert_eq!(
        entries.entries.get("/live").map(String::as_str),
        Some("export default 1")
    );
}
