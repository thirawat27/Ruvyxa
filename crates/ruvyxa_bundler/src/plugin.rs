//! Native bundler plugin pipeline.
//!
//! Plugins are synchronous Rust hooks for the native pipeline. JavaScript config
//! plugin functions remain typed at the package layer, but the Rust bundler now
//! has a first-class pipeline that adapters or embedded callers can populate
//! without forking resolver or compiler code.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{BundleError, BundleTarget, Result};

/// Context provided to plugin hooks.
#[derive(Debug, Clone)]
pub struct PluginContext {
    pub project_root: PathBuf,
    pub importer: Option<PathBuf>,
    pub target: BundleTarget,
}

/// Result from a transform hook.
#[derive(Debug, Clone)]
pub struct TransformResult {
    pub code: String,
    pub map: Option<String>,
}

impl TransformResult {
    pub fn code(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            map: None,
        }
    }
}

/// Native plugin hook contract.
pub trait NativeBundlerPlugin: Send + Sync {
    fn name(&self) -> &str;

    fn resolve_id(
        &self,
        _specifier: &str,
        _importer: Option<&Path>,
        _ctx: &PluginContext,
    ) -> Result<Option<PathBuf>> {
        Ok(None)
    }

    fn transform(
        &self,
        _code: &str,
        _id: &Path,
        _ctx: &PluginContext,
    ) -> Result<Option<TransformResult>> {
        Ok(None)
    }
}

/// Ordered plugin collection.
#[derive(Clone, Default)]
pub struct PluginPipeline {
    plugins: Arc<Vec<Arc<dyn NativeBundlerPlugin>>>,
}

impl std::fmt::Debug for PluginPipeline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginPipeline")
            .field("plugin_count", &self.plugins.len())
            .finish()
    }
}

impl PluginPipeline {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(plugins: Vec<Arc<dyn NativeBundlerPlugin>>) -> Self {
        Self {
            plugins: Arc::new(plugins),
        }
    }

    pub fn plugin_count(&self) -> usize {
        self.plugins.len()
    }

    pub fn plugin_names(&self) -> Vec<String> {
        self.plugins
            .iter()
            .map(|plugin| plugin.name().to_string())
            .collect()
    }

    pub fn resolve_id(
        &self,
        specifier: &str,
        importer: Option<&Path>,
        ctx: &PluginContext,
    ) -> Result<Option<PathBuf>> {
        for plugin in self.plugins.iter() {
            if let Some(path) = plugin.resolve_id(specifier, importer, ctx).map_err(|err| {
                BundleError::Compiler(format!(
                    "plugin `{}` resolve_id failed: {err}",
                    plugin.name()
                ))
            })? {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    pub fn transform(&self, code: &str, id: &Path, ctx: &PluginContext) -> Result<String> {
        Ok(self.transform_with_map(code, id, ctx)?.code)
    }

    /// Apply all transform hooks while preserving the most recent source map.
    pub fn transform_with_map(
        &self,
        code: &str,
        id: &Path,
        ctx: &PluginContext,
    ) -> Result<TransformResult> {
        let mut current = code.to_string();
        let mut map = None;
        for plugin in self.plugins.iter() {
            if let Some(result) = plugin.transform(&current, id, ctx).map_err(|err| {
                BundleError::Compiler(format!(
                    "plugin `{}` transform failed: {err}",
                    plugin.name()
                ))
            })? {
                current = result.code;
                if result.map.is_some() {
                    map = result.map;
                }
            }
        }
        Ok(TransformResult { code: current, map })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BannerPlugin;

    impl NativeBundlerPlugin for BannerPlugin {
        fn name(&self) -> &str {
            "banner"
        }

        fn transform(
            &self,
            code: &str,
            _id: &Path,
            _ctx: &PluginContext,
        ) -> Result<Option<TransformResult>> {
            Ok(Some(TransformResult::code(format!("/* banner */\n{code}"))))
        }
    }

    struct MapPlugin;

    impl NativeBundlerPlugin for MapPlugin {
        fn name(&self) -> &str {
            "source-map"
        }

        fn transform(
            &self,
            code: &str,
            _id: &Path,
            _ctx: &PluginContext,
        ) -> Result<Option<TransformResult>> {
            Ok(Some(TransformResult {
                code: code.to_string(),
                map: Some(
                    r#"{"version":3,"sources":["input.ts"],"names":[],"mappings":"AAAA"}"#
                        .to_string(),
                ),
            }))
        }
    }

    #[test]
    fn pipeline_applies_transform_hooks_in_order() {
        let pipeline = PluginPipeline::new(vec![Arc::new(BannerPlugin)]);
        let ctx = PluginContext {
            project_root: PathBuf::from("/app"),
            importer: None,
            target: BundleTarget::Client,
        };

        let out = pipeline
            .transform("export const answer = 42;", Path::new("/app/page.ts"), &ctx)
            .unwrap();

        assert!(out.starts_with("/* banner */"));
        assert_eq!(pipeline.plugin_names(), vec!["banner"]);
    }

    #[test]
    fn pipeline_preserves_transform_source_map() {
        let pipeline = PluginPipeline::new(vec![Arc::new(MapPlugin)]);
        let ctx = PluginContext {
            project_root: PathBuf::from("/app"),
            importer: None,
            target: BundleTarget::Client,
        };

        let out = pipeline
            .transform_with_map("export const answer = 42;", Path::new("/app/page.ts"), &ctx)
            .unwrap();

        assert!(out.map.unwrap().contains("input.ts"));
    }
}
