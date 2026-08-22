//! Standing in for a `'use server'` module while a browser bundle is built.
//!
//! React's server functions are the mirror image of its client components. A
//! `'use client'` module belongs to the browser and the server graph holds a
//! reference to it; a `'use server'` module belongs to the server and every
//! *client* graph holds a reference to it — a function that posts its arguments
//! and resolves to what the real one returned.
//!
//! Development compiles those browser graphs in the Node worker, which performs
//! the substitution itself. `ruvyxa build` compiles them here, with the Rust
//! bundler, because that is where `NODE_ENV` folding, tree-shaking, and the
//! chunk budget live — and this bundler has no notion of a server function. So
//! it does not learn one. The worker, which already decided what a reference
//! looks like, hands over the finished source per file and this hook returns it
//! in place of what is on disk.
//!
//! That keeps one implementation of the reference shape. A second one here
//! would have to agree with the first about the id, the package the proxy is
//! made from, and the runtime it calls into, and nothing would notice the day
//! it stopped.
//!
//! Without the substitution the bundler walks the real file, sees a module in
//! the action lane inside a client bundle, and fails the build with `RUV1820` —
//! which is the correct answer for every route that has no way to call a server
//! function, and the wrong one for a server-components route.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ruvyxa_bundler::hooks::{BuildHookContext, BuildHooks, TransformOutput};
use ruvyxa_bundler::{BundleTarget, Result};

/// Sources to serve in place of the `'use server'` files a build reaches.
#[derive(Debug, Clone, Default)]
pub(crate) struct ServerReferenceSources {
    by_file: BTreeMap<PathBuf, String>,
}

impl ServerReferenceSources {
    /// Build a substitution table from what the worker reported per route.
    ///
    /// Keyed by path because that is what the bundler asks with. Two routes
    /// reaching one actions file report the same id and the same source, so a
    /// later insert overwrites an identical value rather than losing anything.
    pub(crate) fn new(
        references: impl IntoIterator<Item = ruvyxa_dev_server::ServerReferenceSource>,
    ) -> Self {
        Self {
            by_file: references
                .into_iter()
                .map(|reference| (normalize(&reference.file), reference.source))
                .collect(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.by_file.is_empty()
    }
}

impl BuildHooks for ServerReferenceSources {
    fn host_name(&self) -> &str {
        "ruvyxa:server-references"
    }

    /// Answer only for browser bundles.
    ///
    /// The `react-server` graph compiles these modules for real — that is where
    /// the functions actually run — and an SSR bundle for an ordinary route has
    /// no way to call one, so a `'use server'` module reaching it is still the
    /// mistake `RUV1820` describes.
    fn load(&self, id: &Path, context: &BuildHookContext) -> Result<Option<TransformOutput>> {
        if context.target != BundleTarget::Client {
            return Ok(None);
        }
        Ok(self
            .by_file
            .get(&normalize(id))
            .map(|source| TransformOutput {
                code: source.clone(),
                map: None,
            }))
    }
}

/// One spelling for a path, so a lookup cannot miss on punctuation alone.
///
/// The worker reports what its own resolver produced and the bundler asks with
/// what its resolver produced. Both are absolute and both point at the same
/// file, and only on Windows can they differ in how they are written: in the
/// separator, and in the extended-length `\\?\` prefix that `canonicalize`
/// adds to a root and every path derived from it. Node never produces that
/// prefix, so a build given a canonicalized root asked with it and was
/// answered by nothing — and a miss here is invisible until the build fails
/// somewhere else with `RUV1820`, naming an import the project is right to
/// have.
fn normalize(path: &Path) -> PathBuf {
    let plain = ruvyxa_diagnostics::without_verbatim_prefix(path);
    PathBuf::from(plain.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(file: &str) -> ruvyxa_dev_server::ServerReferenceSource {
        ruvyxa_dev_server::ServerReferenceSource {
            id: "ruv:s_0123456789abcdef".to_string(),
            file: PathBuf::from(file),
            source: "module.exports = {}".to_string(),
        }
    }

    fn context(target: BundleTarget) -> BuildHookContext {
        BuildHookContext {
            project_root: PathBuf::from("C:/app"),
            importer: None,
            target,
        }
    }

    #[test]
    fn substitutes_a_reported_module_in_a_browser_bundle() {
        let sources = ServerReferenceSources::new([reference("C:/app/app/actions.ts")]);
        let loaded = sources
            .load(
                Path::new("C:/app/app/actions.ts"),
                &context(BundleTarget::Client),
            )
            .unwrap();
        assert_eq!(
            loaded.map(|output| output.code).as_deref(),
            Some("module.exports = {}")
        );
    }

    /// The separator a path arrives with must not decide whether it is found.
    #[test]
    fn matches_across_path_separators() {
        let sources = ServerReferenceSources::new([reference("C:/app/app/actions.ts")]);
        let loaded = sources
            .load(
                Path::new(r"C:\app\app\actions.ts"),
                &context(BundleTarget::Client),
            )
            .unwrap();
        assert!(loaded.is_some());
    }

    /// A canonicalized root must not hide the substitution from the bundle it
    /// was collected for.
    ///
    /// `std::fs::canonicalize` writes an extended-length `\\?\` prefix on
    /// Windows, and a root that carries one hands it to every module path
    /// derived from it. Node's resolver never produces one, so the reported
    /// file and the asked-for file describe the same bytes in two spellings.
    /// The build that hit this asked with the prefix, was answered by nothing,
    /// and walked the real `'use server'` module into a browser bundle — where
    /// it is refused as `RUV1820`, naming an import that is not the mistake.
    #[cfg(windows)]
    #[test]
    fn matches_across_the_windows_extended_length_prefix() {
        let sources = ServerReferenceSources::new([reference(r"D:\app\app\actions.ts")]);
        let loaded = sources
            .load(
                Path::new(r"\\?\D:\app\app\actions.ts"),
                &context(BundleTarget::Client),
            )
            .unwrap();
        assert!(
            loaded.is_some(),
            "a verbatim-prefixed path must find what an ordinary one reported"
        );
    }

    /// The server graph runs this code rather than referencing it, and an
    /// ordinary SSR bundle has no business holding either.
    #[test]
    fn leaves_every_other_target_alone() {
        let sources = ServerReferenceSources::new([reference("C:/app/app/actions.ts")]);
        for target in [
            BundleTarget::Ssr,
            BundleTarget::Edge,
            BundleTarget::ReactServer,
        ] {
            let loaded = sources
                .load(Path::new("C:/app/app/actions.ts"), &context(target))
                .unwrap();
            assert!(loaded.is_none(), "{target:?}");
        }
    }

    #[test]
    fn ignores_a_file_no_route_reported() {
        let sources = ServerReferenceSources::new([reference("C:/app/app/actions.ts")]);
        let loaded = sources
            .load(
                Path::new("C:/app/app/other.ts"),
                &context(BundleTarget::Client),
            )
            .unwrap();
        assert!(loaded.is_none());
    }
}
