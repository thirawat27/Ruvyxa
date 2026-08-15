//! Canonical server/client/shared/action module references.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::compiler::CompiledModule;
use crate::{BundleError, Result};

pub const REFERENCE_MANIFEST_CONTRACT: &str = "ruvyxa.references";
pub const REFERENCE_MANIFEST_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ModuleLane {
    Shared,
    Server,
    Client,
    Action,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModuleReference {
    pub id: String,
    pub module: String,
    pub lane: ModuleLane,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReferenceManifest {
    pub contract: String,
    pub schema_version: u32,
    pub artifact_version: String,
    pub modules: Vec<ModuleReference>,
}

pub(crate) fn build_reference_manifest(
    modules: &[CompiledModule],
    project_root: &Path,
) -> Result<ReferenceManifest> {
    let mut lanes = modules
        .iter()
        .filter(|module| !module.is_external)
        .map(|module| (module.path.clone(), declared_lane(module)))
        .collect::<BTreeMap<_, _>>();

    // A client boundary owns its shared dependency closure. Explicit server or
    // action modules terminate that closure and are rejected below.
    let mut pending = lanes
        .iter()
        .filter_map(|(path, lane)| (*lane == ModuleLane::Client).then_some(path.clone()))
        .collect::<VecDeque<_>>();
    while let Some(path) = pending.pop_front() {
        let Some(module) = modules.iter().find(|module| module.path == path) else {
            continue;
        };
        for dependency in module.deps.iter() {
            if lanes.get(dependency) == Some(&ModuleLane::Shared) {
                lanes.insert(dependency.clone(), ModuleLane::Client);
                pending.push_back(dependency.clone());
            }
        }
    }

    for module in modules.iter().filter(|module| !module.is_external) {
        let importer_lane = lanes
            .get(&module.path)
            .copied()
            .unwrap_or(ModuleLane::Shared);
        for dependency in module.deps.iter() {
            let Some(dependency_lane) = lanes.get(dependency).copied() else {
                continue;
            };
            let invalid = matches!(
                (importer_lane, dependency_lane),
                (ModuleLane::Client, ModuleLane::Server | ModuleLane::Action)
                    | (ModuleLane::Action, ModuleLane::Client)
            );
            if invalid {
                return Err(BundleError::Compiler(format!(
                    "RUV1820 invalid {} to {} module crossing: {} imports {}",
                    lane_name(importer_lane),
                    lane_name(dependency_lane),
                    display_module(&module.path, project_root),
                    display_module(dependency, project_root),
                )));
            }
        }
    }

    let mut references = modules
        .iter()
        .filter(|module| !module.is_external)
        .map(|module| {
            let module_path = display_module(&module.path, project_root);
            let lane = lanes
                .get(&module.path)
                .copied()
                .unwrap_or(ModuleLane::Shared);
            let mut dependencies = module
                .deps
                .iter()
                .filter(|path| lanes.contains_key(*path))
                .map(|path| reference_id(path, project_root))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            dependencies.sort();
            ModuleReference {
                id: reference_id(&module.path, project_root),
                module: module_path,
                lane,
                dependencies,
            }
        })
        .collect::<Vec<_>>();
    references.sort_by(|left, right| left.id.cmp(&right.id));

    let encoded = serde_json::to_vec(&references)
        .map_err(|error| BundleError::Compiler(format!("reference manifest: {error}")))?;
    let artifact_version = blake3::hash(&encoded).to_hex()[..16].to_string();
    Ok(ReferenceManifest {
        contract: REFERENCE_MANIFEST_CONTRACT.to_string(),
        schema_version: REFERENCE_MANIFEST_SCHEMA_VERSION,
        artifact_version,
        modules: references,
    })
}

fn declared_lane(module: &CompiledModule) -> ModuleLane {
    match directive(&module.js) {
        Some("use client") => ModuleLane::Client,
        Some("use server") => ModuleLane::Action,
        _ => match module.path.file_stem().and_then(|name| name.to_str()) {
            Some("client") => ModuleLane::Client,
            Some("action" | "actions") => ModuleLane::Action,
            Some("server") => ModuleLane::Server,
            _ => ModuleLane::Shared,
        },
    }
}

fn directive(source: &str) -> Option<&str> {
    let source = skip_leading_trivia(source.trim_start_matches('\u{feff}'));
    for directive in ["use client", "use server", "use cache"] {
        for quote in ['\'', '"'] {
            let candidate = format!("{quote}{directive}{quote}");
            if source.starts_with(&candidate) {
                return Some(directive);
            }
        }
    }
    None
}

/// Whether the module's leading directive prologue declares `expected`.
///
/// The scan accepts a BOM, whitespace, and leading comments but never matches
/// text inside a later function or string. Build manifests use this to expose
/// cache policy without maintaining a second directive parser.
pub fn has_module_directive(source: &str, expected: &str) -> bool {
    directive(source) == Some(expected)
}

/// Byte offset just past the module's directive prologue.
///
/// Generated top-level statements must be inserted here. Not at the very start,
/// because `'use client'` is only a directive while it is the first statement in
/// the module — anything placed above it silently demotes it to a plain string
/// expression and the whole server/client boundary check stops seeing it. Not at
/// the end either, because the linker rewrites imports into `const` bindings at
/// their original position rather than hoisting them, so a trailing import is in
/// the temporal dead zone for every earlier use.
pub fn directive_prologue_end(source: &str) -> usize {
    let mut offset = source.len() - source.trim_start_matches('\u{feff}').len();
    loop {
        let rest = &source[offset..];
        let trimmed = skip_leading_trivia(rest);
        let after_trivia = offset + (rest.len() - trimmed.len());
        let Some(quote) = trimmed.chars().next().filter(|c| *c == '\'' || *c == '"') else {
            return offset;
        };
        let body_start = after_trivia + quote.len_utf8();
        let Some(end) = source[body_start..].find(quote) else {
            return offset;
        };
        // A raw newline means this was never a directive string.
        if source[body_start..body_start + end].contains('\n') {
            return offset;
        }
        let mut next = body_start + end + quote.len_utf8();
        let tail = &source[next..];
        let spaces = tail.len() - tail.trim_start_matches([' ', '\t']).len();
        if source[next + spaces..].starts_with(';') {
            next += spaces + 1;
        }
        offset = next;
    }
}

fn skip_leading_trivia(mut source: &str) -> &str {
    loop {
        source = source.trim_start();
        if let Some(comment) = source.strip_prefix("//") {
            source = comment.find(['\r', '\n']).map_or("", |end| &comment[end..]);
            continue;
        }
        if let Some(comment) = source.strip_prefix("/*") {
            let Some(end) = comment.find("*/") else {
                return "";
            };
            source = &comment[end + 2..];
            continue;
        }
        return source;
    }
}

fn reference_id(path: &Path, project_root: &Path) -> String {
    let module = display_module(path, project_root);
    format!("m_{}", &blake3::hash(module.as_bytes()).to_hex()[..16])
}

fn display_module(path: &Path, project_root: &Path) -> String {
    path.strip_prefix(project_root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn lane_name(lane: ModuleLane) -> &'static str {
    match lane {
        ModuleLane::Shared => "shared",
        ModuleLane::Server => "server",
        ModuleLane::Client => "client",
        ModuleLane::Action => "action",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compiler::CompiledModule;
    use std::path::PathBuf;

    fn module(path: &str, source: &str, deps: Vec<&str>) -> CompiledModule {
        CompiledModule::new(
            PathBuf::from(path),
            source.to_string(),
            deps.into_iter().map(PathBuf::from).collect(),
            BTreeMap::new(),
            false,
            false,
        )
    }

    #[test]
    fn client_lane_owns_shared_closure_and_manifest_is_stable() {
        let modules = vec![
            module(
                "C:/app/client.ts",
                "'use client'; export {}",
                vec!["C:/app/shared.ts"],
            ),
            module("C:/app/shared.ts", "export const value = 1", vec![]),
        ];
        let first = build_reference_manifest(&modules, Path::new("C:/app")).unwrap();
        let second = build_reference_manifest(&modules, Path::new("C:/app")).unwrap();
        assert_eq!(first, second);
        assert!(
            first
                .modules
                .iter()
                .all(|module| module.lane == ModuleLane::Client)
        );
    }

    #[test]
    fn client_to_action_crossing_fails_closed() {
        let modules = vec![
            module(
                "C:/app/client.ts",
                "'use client'; export {}",
                vec!["C:/app/action.ts"],
            ),
            module("C:/app/action.ts", "'use server'; export {}", vec![]),
        ];
        let error = build_reference_manifest(&modules, Path::new("C:/app")).unwrap_err();
        assert!(error.to_string().contains("RUV1820"));
    }

    #[test]
    fn directives_allow_a_bom_whitespace_and_leading_comments() {
        assert_eq!(
            directive("\u{feff} /* license */\n// boundary\n'use client';"),
            Some("use client")
        );
        assert_eq!(directive("/* unterminated"), None);
    }
}
