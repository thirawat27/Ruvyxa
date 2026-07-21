//! Deterministic CSS Modules naming and Sass compilation shared with style collection.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// CSS produced for a module together with its JavaScript-facing class map.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CssModule {
    pub css: String,
    pub classes: BTreeMap<String, String>,
}

/// Return whether a stylesheet follows the `.module.css` / `.module.scss` convention.
pub fn is_css_module_path(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let name = name.to_ascii_lowercase();
    name.ends_with(".module.css")
        || name.ends_with(".module.scss")
        || name.ends_with(".module.sass")
}

/// Return whether a path contains Sass syntax that must be compiled before collection.
pub fn is_sass_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("scss") || extension.eq_ignore_ascii_case("sass")
        })
}

/// Compile a Sass file with project-root and `node_modules` load paths.
pub fn compile_sass_file(path: &Path, project_root: &Path) -> Result<String, String> {
    let options = grass::Options::default()
        .style(grass::OutputStyle::Expanded)
        .load_path(project_root)
        .load_path(project_root.join("node_modules"));
    grass::from_path(path, &options).map_err(|error| error.to_string())
}

/// Compile and scope a CSS Module from disk.
pub fn compile_css_module(path: &Path, project_root: &Path) -> Result<CssModule, String> {
    let css = if is_sass_path(path) {
        compile_sass_file(path, project_root)
            .map_err(|error| format!("RUV1402 Sass compilation failed: {error}"))?
    } else {
        std::fs::read_to_string(path).map_err(|error| error.to_string())?
    };
    Ok(scope_css_module(&css, path, project_root))
}

/// Scope local class selectors using a stable project-relative path and class-name hash.
///
/// The transformer operates on compiled CSS, where Sass nesting has already been flattened. It
/// only rewrites selector preludes and leaves declaration values, comments, and strings intact.
pub fn scope_css_module(css: &str, path: &Path, project_root: &Path) -> CssModule {
    let mut output = String::with_capacity(css.len());
    let mut classes = BTreeMap::new();
    let mut scoped_names = BTreeMap::new();
    let chars = css.chars().collect::<Vec<_>>();
    let mut index = 0;
    let mut quote = None;
    let mut in_comment = false;
    let mut escape = false;
    let mut block_allows_rules = vec![true];
    let mut rule_local_classes = vec![Vec::<String>::new()];
    let mut prelude = String::new();
    let mut prelude_locals = Vec::<String>::new();

    while index < chars.len() {
        let ch = chars[index];
        let next = chars.get(index + 1).copied();

        if in_comment {
            output.push(ch);
            if ch == '*' && next == Some('/') {
                output.push('/');
                index += 2;
                in_comment = false;
            } else {
                index += 1;
            }
            continue;
        }

        if let Some(active_quote) = quote {
            output.push(ch);
            if escape {
                escape = false;
            } else if ch == '\\' {
                escape = true;
            } else if ch == active_quote {
                quote = None;
            }
            index += 1;
            continue;
        }

        if ch == '/' && next == Some('*') {
            output.push_str("/*");
            index += 2;
            in_comment = true;
            continue;
        }
        if ch == '\'' || ch == '"' {
            output.push(ch);
            prelude.push(ch);
            quote = Some(ch);
            index += 1;
            continue;
        }

        let selector_context = block_allows_rules.last().copied().unwrap_or(true)
            || statement_opens_nested_rule(&chars, index);
        if selector_context
            && chars[index..].starts_with(&[':', 'g', 'l', 'o', 'b', 'a', 'l', '('])
            && let Some((global, end)) = global_selector_contents(&chars, index + 8)
        {
            output.push_str(&global);
            prelude.push_str(&global);
            index = end;
            continue;
        }
        if selector_context && ch == '.' && next.is_some_and(is_class_start) {
            let mut end = index + 1;
            while end < chars.len() && is_class_continue(chars[end]) {
                end += 1;
            }
            let local = chars[index + 1..end].iter().collect::<String>();
            let scoped = scoped_names
                .entry(local.clone())
                .or_insert_with(|| scoped_class_name(path, project_root, &local));
            classes
                .entry(local.clone())
                .or_insert_with(|| scoped.clone());
            output.push('.');
            output.push_str(scoped);
            prelude.push('.');
            prelude.push_str(scoped);
            if !prelude_locals.contains(&local) {
                prelude_locals.push(local);
            }
            index = end;
            continue;
        }

        if !selector_context
            && prelude.trim().is_empty()
            && let Some((end, composed)) = local_composition(&chars, index)
            && let Some(owners) = rule_local_classes
                .iter()
                .rev()
                .find(|owners| !owners.is_empty())
                .cloned()
        {
            let composed = composed
                .iter()
                .map(|local| {
                    let scoped = scoped_names
                        .entry(local.clone())
                        .or_insert_with(|| scoped_class_name(path, project_root, local))
                        .clone();
                    classes
                        .entry(local.clone())
                        .or_insert_with(|| scoped.clone());
                    scoped
                })
                .collect::<Vec<_>>();
            for owner in owners {
                let owner_scoped = scoped_names
                    .entry(owner.clone())
                    .or_insert_with(|| scoped_class_name(path, project_root, &owner))
                    .clone();
                let exported = classes.entry(owner).or_insert(owner_scoped);
                for scoped in &composed {
                    if !exported.split_whitespace().any(|class| class == scoped) {
                        exported.push(' ');
                        exported.push_str(scoped);
                    }
                }
            }
            index = end;
            prelude.clear();
            continue;
        }

        output.push(ch);
        match ch {
            '{' => {
                let container = is_container_at_rule(&prelude);
                block_allows_rules.push(container);
                rule_local_classes.push(if container {
                    Vec::new()
                } else {
                    std::mem::take(&mut prelude_locals)
                });
                prelude.clear();
            }
            '}' => {
                if block_allows_rules.len() > 1 {
                    block_allows_rules.pop();
                }
                if rule_local_classes.len() > 1 {
                    rule_local_classes.pop();
                }
                prelude.clear();
                prelude_locals.clear();
            }
            ';' => {
                prelude.clear();
                prelude_locals.clear();
            }
            _ => prelude.push(ch),
        }
        index += 1;
    }

    CssModule {
        css: output,
        classes,
    }
}

fn statement_opens_nested_rule(chars: &[char], start: usize) -> bool {
    let mut quote = None;
    let mut escape = false;
    let mut index = start;
    while index < chars.len() {
        let character = chars[index];
        if let Some(active_quote) = quote {
            if escape {
                escape = false;
            } else if character == '\\' {
                escape = true;
            } else if character == active_quote {
                quote = None;
            }
        } else if matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if character == '{' {
            return true;
        } else if matches!(character, ';' | '}') {
            return false;
        }
        index += 1;
    }
    false
}

fn global_selector_contents(chars: &[char], content_start: usize) -> Option<(String, usize)> {
    let mut depth = 1usize;
    let mut index = content_start;
    let mut content = String::new();
    while index < chars.len() {
        match chars[index] {
            '(' => {
                depth += 1;
                content.push('(');
            }
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((content, index + 1));
                }
                content.push(')');
            }
            character => content.push(character),
        }
        index += 1;
    }
    None
}

fn local_composition(chars: &[char], start: usize) -> Option<(usize, Vec<String>)> {
    const KEYWORD: [char; 8] = ['c', 'o', 'm', 'p', 'o', 's', 'e', 's'];
    if !chars[start..].starts_with(&KEYWORD) {
        return None;
    }
    let mut index = start + KEYWORD.len();
    if chars
        .get(index)
        .is_some_and(|character| is_class_continue(*character))
    {
        return None;
    }
    while chars
        .get(index)
        .is_some_and(|character| character.is_whitespace())
    {
        index += 1;
    }
    if chars.get(index) != Some(&':') {
        return None;
    }
    index += 1;
    let value_start = index;
    while chars.get(index).is_some_and(|character| *character != ';') {
        index += 1;
    }
    if chars.get(index) != Some(&';') {
        return None;
    }
    let value = chars[value_start..index].iter().collect::<String>();
    let names = value.split_whitespace().collect::<Vec<_>>();
    if names.is_empty()
        || names.contains(&"from")
        || names
            .iter()
            .any(|name| !name.chars().all(is_class_continue))
    {
        return None;
    }
    Some((index + 1, names.into_iter().map(str::to_string).collect()))
}

/// Serialize a CSS Module as an ESM default export for the linker.
pub fn css_module_javascript(module: &CssModule) -> Result<String, serde_json::Error> {
    serde_json::to_string(&module.classes).map(|classes| format!("export default {classes};"))
}

fn scoped_class_name(path: &Path, project_root: &Path, local: &str) -> String {
    let relative = normalized_relative_path(path, project_root);
    let digest = fnv1a_64(format!("{relative}:{local}").as_bytes());
    let stem = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("style")
        .trim_end_matches(".module")
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect::<String>();
    format!("{stem}_{local}__{digest:016x}")
}

fn fnv1a_64(input: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in input {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

pub(crate) fn normalized_relative_path(path: &Path, project_root: &Path) -> String {
    let path = path.canonicalize().unwrap_or_else(|_| PathBuf::from(path));
    let root = project_root
        .canonicalize()
        .unwrap_or_else(|_| PathBuf::from(project_root));
    path.strip_prefix(&root)
        .unwrap_or(&path)
        .display()
        .to_string()
        .replace('\\', "/")
        .to_ascii_lowercase()
}

fn is_class_start(ch: char) -> bool {
    ch.is_ascii_alphabetic() || ch == '_' || ch == '-'
}

fn is_class_continue(ch: char) -> bool {
    ch.is_ascii_alphanumeric() || ch == '_' || ch == '-'
}

fn is_container_at_rule(prelude: &str) -> bool {
    let prelude = prelude.trim_start().to_ascii_lowercase();
    [
        "@media",
        "@supports",
        "@layer",
        "@container",
        "@document",
        "@scope",
        "@keyframes",
        "@-webkit-keyframes",
    ]
    .iter()
    .any(|prefix| prelude.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_selectors_but_not_declaration_values_or_strings() {
        let root = Path::new("/project");
        let path = root.join("components/card.module.css");
        let output = scope_css_module(
            ".card, .card-active { color: var(--theme.card); content: \".card\"; }",
            &path,
            root,
        );

        assert_eq!(output.classes.len(), 2);
        assert!(output.css.contains(".card_card__"));
        assert!(output.css.contains(".card_card-active__"));
        assert!(output.css.contains("var(--theme.card)"));
        assert!(output.css.contains("\".card\""));
    }

    #[test]
    fn scopes_rules_inside_container_at_rules() {
        let root = Path::new("/project");
        let path = root.join("app/page.module.css");
        let output = scope_css_module(
            "@media (min-width: 40rem) { .page { display: grid; } }",
            &path,
            root,
        );

        assert!(output.css.contains(".page_page__"));
        assert_eq!(output.classes.keys().collect::<Vec<_>>(), vec!["page"]);
    }

    #[test]
    fn generated_names_change_with_the_project_relative_path() {
        let root = Path::new("/project");
        let first = scope_css_module(".button {}", &root.join("a/card.module.css"), root);
        let second = scope_css_module(".button {}", &root.join("b/card.module.css"), root);

        assert_ne!(first.classes["button"], second.classes["button"]);
        assert_eq!(
            first.classes["button"],
            scope_css_module(".button {}", &root.join("a/card.module.css"), root).classes["button"]
        );
    }

    #[test]
    fn class_name_contract_has_a_cross_runtime_golden_value() {
        let root = Path::new("/project");
        let output = scope_css_module(".card {}", &root.join("styles/card.module.css"), root);
        assert_eq!(output.classes["card"], "card_card__feff5ad3a1e67b7b");
    }

    #[test]
    fn supports_nested_global_and_local_composition_contracts() {
        let root = Path::new("/project");
        let path = root.join("styles/card.module.css");
        let output = scope_css_module(
            r#"
.base { color: navy; }
.card {
  composes: base;
  & .title { color: white; }
  :global(.theme-dark) .icon { color: black; }
}
"#,
            &path,
            root,
        );

        let card_classes = output.classes["card"]
            .split_whitespace()
            .collect::<Vec<_>>();
        assert_eq!(card_classes.len(), 2);
        assert_eq!(card_classes[1], output.classes["base"]);
        assert!(output.classes.contains_key("title"));
        assert!(output.classes.contains_key("icon"));
        assert!(!output.classes.contains_key("theme-dark"));
        assert!(output.css.contains(".theme-dark"));
        assert!(!output.css.contains(":global("));
        assert!(!output.css.contains("composes:"));
        assert!(
            output
                .css
                .contains(&format!(".{}", output.classes["title"]))
        );
    }
}
