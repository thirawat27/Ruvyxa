//! Build hooks injected by Ruvyxa's TypeScript plugin host.
//!
//! Executable plugin callbacks remain in the selected JavaScript runtime. This
//! module is only the internal, synchronous boundary used by the native resolver
//! and compiler to request hook results from that host.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{BundleError, BundleTarget, Result};

/// Context sent with native build-hook invocations.
#[derive(Debug, Clone)]
pub struct BuildHookContext {
    pub project_root: PathBuf,
    pub importer: Option<PathBuf>,
    pub target: BundleTarget,
}

/// Source returned by a TypeScript transform hook.
#[derive(Debug, Clone)]
pub struct TransformOutput {
    pub code: String,
    pub map: Option<String>,
}

impl TransformOutput {
    pub fn code(code: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            map: None,
        }
    }
}

/// Internal host boundary consumed by the native bundler.
pub trait BuildHooks: Send + Sync {
    fn host_name(&self) -> &str;

    fn resolve_id(
        &self,
        _specifier: &str,
        _importer: Option<&Path>,
        _context: &BuildHookContext,
    ) -> Result<Option<PathBuf>> {
        Ok(None)
    }

    fn transform(
        &self,
        _code: &str,
        _id: &Path,
        _context: &BuildHookContext,
    ) -> Result<Option<TransformOutput>> {
        Ok(None)
    }
}

/// Ordered build-hook hosts. Ruvyxa currently installs at most one TypeScript host.
#[derive(Clone, Default)]
pub struct BuildHookPipeline {
    hosts: Arc<Vec<Arc<dyn BuildHooks>>>,
}

impl std::fmt::Debug for BuildHookPipeline {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BuildHookPipeline")
            .field("host_count", &self.hosts.len())
            .finish()
    }
}

impl BuildHookPipeline {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(hosts: Vec<Arc<dyn BuildHooks>>) -> Self {
        Self {
            hosts: Arc::new(hosts),
        }
    }

    pub fn host_count(&self) -> usize {
        self.hosts.len()
    }

    pub fn resolve_id(
        &self,
        specifier: &str,
        importer: Option<&Path>,
        context: &BuildHookContext,
    ) -> Result<Option<PathBuf>> {
        for host in self.hosts.iter() {
            if let Some(path) = host
                .resolve_id(specifier, importer, context)
                .map_err(|error| {
                    BundleError::Compiler(format!(
                        "build hook host `{}` resolve_id failed: {error}",
                        host.host_name()
                    ))
                })?
            {
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    pub fn transform_with_map(
        &self,
        code: &str,
        id: &Path,
        context: &BuildHookContext,
    ) -> Result<TransformOutput> {
        let mut current = code.to_string();
        let mut map = None;
        for host in self.hosts.iter() {
            if let Some(result) = host.transform(&current, id, context).map_err(|error| {
                BundleError::Compiler(format!(
                    "build hook host `{}` transform failed: {error}",
                    host.host_name()
                ))
            })? {
                current = result.code;
                if result.map.is_some() {
                    map = result.map;
                }
            }
        }
        Ok(TransformOutput { code: current, map })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct BannerHooks;

    impl BuildHooks for BannerHooks {
        fn host_name(&self) -> &str {
            "banner"
        }

        fn transform(
            &self,
            code: &str,
            _id: &Path,
            _context: &BuildHookContext,
        ) -> Result<Option<TransformOutput>> {
            Ok(Some(TransformOutput::code(format!("/* banner */\n{code}"))))
        }
    }

    struct SourceMapHooks;

    impl BuildHooks for SourceMapHooks {
        fn host_name(&self) -> &str {
            "source-map"
        }

        fn transform(
            &self,
            code: &str,
            _id: &Path,
            _context: &BuildHookContext,
        ) -> Result<Option<TransformOutput>> {
            Ok(Some(TransformOutput {
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
        let pipeline = BuildHookPipeline::new(vec![Arc::new(BannerHooks)]);
        let context = BuildHookContext {
            project_root: PathBuf::from("/app"),
            importer: None,
            target: BundleTarget::Client,
        };

        let output = pipeline
            .transform_with_map(
                "export const answer = 42;",
                Path::new("/app/page.ts"),
                &context,
            )
            .unwrap();

        assert!(output.code.starts_with("/* banner */"));
    }

    #[test]
    fn pipeline_preserves_transform_source_map() {
        let pipeline = BuildHookPipeline::new(vec![Arc::new(SourceMapHooks)]);
        let context = BuildHookContext {
            project_root: PathBuf::from("/app"),
            importer: None,
            target: BundleTarget::Client,
        };

        let output = pipeline
            .transform_with_map(
                "export const answer = 42;",
                Path::new("/app/page.ts"),
                &context,
            )
            .unwrap();

        assert!(output.map.unwrap().contains("input.ts"));
    }
}
