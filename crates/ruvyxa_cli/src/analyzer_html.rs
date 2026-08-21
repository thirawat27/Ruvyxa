//! Interactive bundle analysis built from the same client bundler as production builds.
//!
//! The report is self-contained so it can be archived by CI or opened locally without a
//! development server. Bundling happens in an isolated system-temporary directory; only the
//! requested HTML report is retained.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use ruvyxa_graph::{RouteManifest, ValidationReport};

use crate::*;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BundleAnalysis {
    client_manifest: serde_json::Value,
    module_bytes: BTreeMap<String, u64>,
}

/// Compile the current client graph without mutating the project's build output.
pub(crate) fn analyze_client_bundle(
    root: &Path,
    config: &ProjectConfig,
    manifest: &RouteManifest,
) -> anyhow::Result<BundleAnalysis> {
    let temporary = AnalyzerTemporaryDirectory::create()?;
    let client_dir = temporary.path.join("client");
    fs::create_dir_all(&client_dir)?;

    // An analyzer needs the module list even when normal production builds keep the optional
    // chunk manifest disabled. Copy the build contract and change only that reporting flag.
    let build = BuildConfigOptions {
        minify: config.build.minify,
        sourcemap: config.build.sourcemap,
        tree_shaking: config.build.tree_shaking,
        split_strategy: config.build.split_strategy.clone(),
        parallelism: config.build.parallelism,
        jsx_runtime: config.build.jsx_runtime.clone(),
        // Carried, not defaulted: the analyzer reports bundle sizes, and since
        // `build.target` reaches the transform it also reaches those bytes.
        es_target: config.build.es_target.clone(),
        emit_chunk_manifest: Some(true),
        prebundle_dependencies: config.build.prebundle_dependencies,
        prerender_cache: config.build.prerender_cache,
    };
    let plugin_session = TypeScriptPluginBuildSession::new(
        root,
        &config.plugins,
        config.javascript_runtime(),
        config.markdown_enabled(),
        config.react_compiler.unwrap_or(false),
    )?;
    let client_manifest = emit_client_bundles_with_session(
        root,
        &root.join(config.app_dir()),
        manifest,
        &client_dir,
        &build,
        &config.plugins,
        RuvyxaBuildCache {
            dependency_hash: &config.config_dependency_hash,
            directory: &build_cache_dir(root, &config.cache),
        },
        &plugin_session,
    )?;

    let mut module_bytes = BTreeMap::new();
    collect_module_paths(&client_manifest, &mut |module| {
        let path = Path::new(module);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        if let Ok(metadata) = fs::metadata(path) {
            module_bytes.insert(module.to_string(), metadata.len());
        }
    });

    Ok(BundleAnalysis {
        client_manifest,
        module_bytes,
    })
}

fn collect_module_paths(value: &serde_json::Value, visit: &mut impl FnMut(&str)) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                collect_module_paths(value, visit);
            }
        }
        serde_json::Value::Object(object) => {
            if let Some(modules) = object.get("modules").and_then(serde_json::Value::as_array) {
                for module in modules.iter().filter_map(serde_json::Value::as_str) {
                    visit(module);
                }
            }
            for value in object.values() {
                collect_module_paths(value, visit);
            }
        }
        _ => {}
    }
}

pub(crate) fn render_analyzer_html(
    root: &Path,
    manifest: &RouteManifest,
    validation: &ValidationReport,
    bundle: &BundleAnalysis,
) -> anyhow::Result<String> {
    let payload = serde_json::json!({
        "root": root.display().to_string(),
        "routes": manifest.routes,
        "validation": validation,
        "bundle": bundle,
    });
    // A literal `<` could close the data script. JSON escapes retain the exact value while making
    // the payload inert even when a project path or diagnostic contains HTML-looking text.
    let payload = serde_json::to_string(&payload)?
        .replace('<', "\\u003c")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    Ok(include_str!("../templates/analyze.html").replace("__RUVYXA_ANALYSIS_DATA__", &payload))
}

struct AnalyzerTemporaryDirectory {
    path: PathBuf,
}

impl AnalyzerTemporaryDirectory {
    fn create() -> anyhow::Result<Self> {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("ruvyxa-analyze-{}-{nonce}", std::process::id()));
        fs::create_dir(&path).with_context(|| {
            format!(
                "failed to create analyzer temporary directory {}",
                path.display()
            )
        })?;
        Ok(Self { path })
    }
}

impl Drop for AnalyzerTemporaryDirectory {
    fn drop(&mut self) {
        // `path` is constructed directly below the OS temp directory with a process-unique name.
        // Never broaden this cleanup to a parent or user-controlled path.
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_report_keeps_project_data_in_an_inert_json_script() {
        let manifest = RouteManifest {
            app_dir: PathBuf::from("app"),
            routes: Vec::new(),
            i18n: None,
        };
        let validation = ValidationReport {
            routes: 0,
            page_routes: 0,
            api_routes: 0,
            client_modules: 0,
            server_modules: 0,
            diagnostics: Vec::new(),
        };
        let bundle = BundleAnalysis {
            client_manifest: serde_json::json!({"routes": []}),
            module_bytes: BTreeMap::new(),
        };

        let html = render_analyzer_html(
            Path::new("</script><script>alert(1)</script>"),
            &manifest,
            &validation,
            &bundle,
        )
        .unwrap();
        assert!(html.contains("Ruvyxa Bundle Analyzer"));
        assert!(!html.contains("</script><script>alert(1)</script>"));
        assert!(html.contains("\\u003c/script>"));
    }
}
