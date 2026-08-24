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

    /// Supply source for a resolved module before the filesystem is read.
    fn load(&self, _id: &Path, _context: &BuildHookContext) -> Result<Option<TransformOutput>> {
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

    /// Compile raw `.md`/`.mdx` through the configured JavaScript MDX host.
    /// Returning `None` keeps the native Rust content compiler as the fallback.
    fn compile_content(
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
                Self::validate_resolved_id(&path, specifier, host.host_name())?;
                return Ok(Some(path));
            }
        }
        Ok(None)
    }

    /// Refuse a resolved id this bundler cannot open, and say what it needed.
    ///
    /// `build.onResolve` answers with a **path**, and the loader hook then
    /// supplies its contents — the file itself need not exist. The two spellings
    /// every other ecosystem uses for a virtual module are not paths, and both
    /// were passed straight to the filesystem: `'\0virtual:x'` reached Windows
    /// as `strings passed to WinAPI cannot contain NULs`, and `'virtual:x'` as
    /// `The system cannot find the file specified`, each naming a plugin
    /// nowhere in the message.
    fn validate_resolved_id(path: &Path, specifier: &str, host: &str) -> Result<()> {
        let text = path.to_string_lossy();
        // A NUL can never be opened on any platform this runs on; the host
        // above applies the rest of the rule, where the plugin's own string is
        // still in hand.
        if !text.contains('\0') {
            return Ok(());
        }
        Err(BundleError::Compiler(format!(
            "plugin host `{host}` resolved `{specifier}` to `{text}`, which is not a file path. \
             A resolve hook answers with a path — the file itself may be virtual, and a load hook \
             can supply its contents — so return something like \
             `${{root}}/virtual-{specifier}.ts` rather than a bare or NUL-prefixed id."
        )))
    }

    pub fn load(&self, id: &Path, context: &BuildHookContext) -> Result<Option<TransformOutput>> {
        for host in self.hosts.iter() {
            if let Some(source) = host.load(id, context).map_err(|error| {
                BundleError::Compiler(format!(
                    "build hook host `{}` load failed: {error}",
                    host.host_name()
                ))
            })? {
                return Ok(Some(source));
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

    pub fn compile_content(
        &self,
        code: &str,
        id: &Path,
        context: &BuildHookContext,
    ) -> Result<Option<TransformOutput>> {
        for host in self.hosts.iter() {
            if let Some(result) = host.compile_content(code, id, context).map_err(|error| {
                BundleError::Compiler(format!(
                    "build hook host `{}` content compilation failed: {error}",
                    host.host_name()
                ))
            })? {
                return Ok(Some(result));
            }
        }
        Ok(None)
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
