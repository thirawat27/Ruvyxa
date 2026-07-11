//! Native Markdown and MDX-to-module compilation.
//!
//! Content files are lowered to ordinary React modules before they enter the
//! existing TypeScript/JSX compiler. This keeps resolution, boundary checks,
//! linking, source hashing, and compile caching identical for every page type.

use std::path::Path;

use markdown::mdast::{AttributeContent, AttributeValue, Node};
use markdown::ParseOptions;
use serde_json::{json, Map, Value};

/// Compile a `.md` or `.mdx` document into a React ESM page module.
pub fn compile_content_module(source: &str, path: &Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (frontmatter, body) = split_frontmatter(source)?;
    let frontmatter_json = parse_frontmatter(frontmatter.as_deref());

    match extension.as_str() {
        "md" => compile_markdown(&body, &frontmatter_json),
        "mdx" => compile_mdx(&body, &frontmatter_json),
        _ => Err(format!(
            "RUV1310: unsupported content extension for {}",
            path.display()
        )),
    }
}

fn compile_markdown(body: &str, frontmatter: &Value) -> Result<String, String> {
    let tree = markdown::to_mdast(body, &ParseOptions::gfm())
        .map_err(|error| format!("RUV1310: Markdown parse error: {error}"))?;
    let mut headings = Vec::new();
    collect_ast_headings(&tree, &mut headings);
    let children = match &tree {
        Node::Root(root) => render_children(&root.children),
        _ => render_node(&tree),
    };
    Ok(module_source(
        frontmatter,
        &headings,
        "md",
        "",
        &format!(
            "React.createElement(\"article\", {{ className: \"ruvyxa-content\", \"data-content-format\": \"md\" }}, {children})"
        ),
    ))
}

fn compile_mdx(body: &str, frontmatter: &Value) -> Result<String, String> {
    let (esm, markdown_body) = extract_mdx_esm(body);
    let tree = markdown::to_mdast(&markdown_body, &ParseOptions::mdx())
        .map_err(|error| format!("RUV1311: MDX parse error: {error}"))?;
    let mut headings = Vec::new();
    collect_ast_headings(&tree, &mut headings);
    let children = match &tree {
        Node::Root(root) => render_children(&root.children),
        _ => render_node(&tree),
    };
    let render = format!(
        "React.createElement(\"article\", {{ className: \"ruvyxa-content\", \"data-content-format\": \"mdx\" }}, {children})"
    );
    Ok(module_source(frontmatter, &headings, "mdx", &esm, &render))
}

fn module_source(
    frontmatter: &Value,
    headings: &[Value],
    format: &str,
    esm: &str,
    render_expression: &str,
) -> String {
    let frontmatter_export = content_export(esm, "frontmatter", &frontmatter.to_string());
    let meta_export = content_export(esm, "meta", "frontmatter");
    let headings_export = content_export(
        esm,
        "headings",
        &Value::Array(headings.to_vec()).to_string(),
    );
    let format_export = content_export(esm, "contentFormat", &js_string(format));
    format!(
        "import React from \"react\";\n{esm}\n{frontmatter_export}{meta_export}{headings_export}{format_export}export default function RuvyxaContentPage() {{ return {render_expression}; }}\n",
    )
}

fn content_export(esm: &str, name: &str, value: &str) -> String {
    let declarations = [
        format!("export const {name}"),
        format!("export let {name}"),
        format!("export var {name}"),
    ];
    if declarations
        .iter()
        .any(|declaration| esm.contains(declaration))
    {
        String::new()
    } else {
        format!("export const {name} = {value};\n")
    }
}

fn split_frontmatter(source: &str) -> Result<(Option<String>, String), String> {
    let normalized = source.strip_prefix('\u{feff}').unwrap_or(source);
    if !normalized.starts_with("---\n") && !normalized.starts_with("---\r\n") {
        return Ok((None, normalized.to_string()));
    }

    let mut offset = 0usize;
    for (index, line) in normalized.split_inclusive('\n').enumerate() {
        offset += line.len();
        if index > 0 && matches!(line.trim(), "---" | "...") {
            let first_line_end = normalized.find('\n').map_or(3, |value| value + 1);
            let closing_start = offset - line.len();
            let frontmatter = normalized[first_line_end..closing_start].trim().to_string();
            return Ok((Some(frontmatter), normalized[offset..].to_string()));
        }
    }

    Err("RUV1312: frontmatter starts with '---' but has no closing delimiter".to_string())
}

fn parse_frontmatter(source: Option<&str>) -> Value {
    let mut output = Map::new();
    for line in source.unwrap_or_default().lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        output.insert(key.to_string(), parse_frontmatter_value(value.trim()));
    }
    Value::Object(output)
}

fn parse_frontmatter_value(value: &str) -> Value {
    let unquoted = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        });
    if let Some(value) = unquoted {
        return Value::String(value.to_string());
    }
    if value.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if value.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if value.eq_ignore_ascii_case("null") || value == "~" {
        return Value::Null;
    }
    if let Ok(number) = value.parse::<i64>() {
        return Value::Number(number.into());
    }
    if let Ok(number) = value.parse::<f64>() {
        if let Some(number) = serde_json::Number::from_f64(number) {
            return Value::Number(number);
        }
    }
    if let Some(items) = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
    {
        return Value::Array(
            items
                .split(',')
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(parse_frontmatter_value)
                .collect(),
        );
    }
    Value::String(value.to_string())
}

fn extract_mdx_esm(source: &str) -> (String, String) {
    let mut esm = String::new();
    let mut body = String::new();
    let mut in_statement = false;
    let mut fence: Option<char> = None;

    for line in source.lines() {
        let trimmed = line.trim_start();
        let fence_marker = trimmed
            .chars()
            .next()
            .filter(|character| matches!(character, '`' | '~'))
            .filter(|character| trimmed.chars().take_while(|next| next == character).count() >= 3);
        if let Some(marker) = fence_marker {
            if fence == Some(marker) {
                fence = None;
            } else if fence.is_none() {
                fence = Some(marker);
            }
            body.push_str(line);
            body.push('\n');
            continue;
        }
        if fence.is_some() {
            body.push_str(line);
            body.push('\n');
            continue;
        }

        let is_top_level = trimmed.len() == line.len();
        let starts_esm =
            is_top_level && (trimmed.starts_with("import ") || trimmed.starts_with("export "));
        if starts_esm || in_statement {
            esm.push_str(line);
            esm.push('\n');
            in_statement = !(trimmed.ends_with(';')
                || (!trimmed.ends_with(',') && !trimmed.ends_with('{') && !trimmed.ends_with('(')));
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    (esm, body)
}

fn render_children(children: &[Node]) -> String {
    if children.is_empty() {
        return "null".to_string();
    }
    children
        .iter()
        .map(render_node)
        .collect::<Vec<_>>()
        .join(", ")
}

fn element(tag: &str, props: &str, children: &[Node]) -> String {
    let children = render_children(children);
    format!("React.createElement({tag}, {props}, {children})")
}

fn render_node(node: &Node) -> String {
    match node {
        Node::Root(value) => element("React.Fragment", "null", &value.children),
        Node::Paragraph(value) => element("\"p\"", "null", &value.children),
        Node::Heading(value) => {
            let text = plain_text(&value.children);
            let props = format!("{{ id: {} }}", js_string(&slugify(&text)));
            element(&format!("\"h{}\"", value.depth), &props, &value.children)
        }
        Node::Blockquote(value) => element("\"blockquote\"", "null", &value.children),
        Node::List(value) => {
            let tag = if value.ordered { "\"ol\"" } else { "\"ul\"" };
            let props = value
                .start
                .map(|start| format!("{{ start: {start} }}"))
                .unwrap_or_else(|| "null".to_string());
            element(tag, &props, &value.children)
        }
        Node::ListItem(value) => {
            let checkbox = value.checked.map(|checked| {
                format!(
                    "React.createElement(\"input\", {{ type: \"checkbox\", checked: {checked}, disabled: true, readOnly: true }})"
                )
            });
            let mut children = value.children.iter().map(render_node).collect::<Vec<_>>();
            if let Some(checkbox) = checkbox {
                children.insert(0, checkbox);
            }
            format!("React.createElement(\"li\", null, {})", children.join(", "))
        }
        Node::Emphasis(value) => element("\"em\"", "null", &value.children),
        Node::Strong(value) => element("\"strong\"", "null", &value.children),
        Node::Delete(value) => element("\"del\"", "null", &value.children),
        Node::Link(value) => {
            let mut props = format!("{{ href: {}", js_string(&value.url));
            if let Some(title) = &value.title {
                props.push_str(&format!(", title: {}", js_string(title)));
            }
            props.push_str(" }");
            element("\"a\"", &props, &value.children)
        }
        Node::Image(value) => {
            let title = value
                .title
                .as_ref()
                .map(|title| format!(", title: {}", js_string(title)))
                .unwrap_or_default();
            format!(
                "React.createElement(\"img\", {{ src: {}, alt: {}, loading: \"lazy\", decoding: \"async\"{title} }})",
                js_string(&value.url),
                js_string(&value.alt)
            )
        }
        Node::Text(value) => js_string(&value.value),
        Node::InlineCode(value) => format!(
            "React.createElement(\"code\", null, {})",
            js_string(&value.value)
        ),
        Node::Code(value) => {
            let class_name = value
                .lang
                .as_ref()
                .map(|language| {
                    format!(
                        "{{ className: {} }}",
                        js_string(&format!("language-{language}"))
                    )
                })
                .unwrap_or_else(|| "null".to_string());
            format!(
                "React.createElement(\"pre\", null, React.createElement(\"code\", {class_name}, {}))",
                js_string(&value.value)
            )
        }
        Node::Break(_) => "React.createElement(\"br\", null)".to_string(),
        Node::ThematicBreak(_) => "React.createElement(\"hr\", null)".to_string(),
        Node::Table(value) => element("\"table\"", "null", &value.children),
        Node::TableRow(value) => element("\"tr\"", "null", &value.children),
        Node::TableCell(value) => element("\"td\"", "null", &value.children),
        Node::Html(value) => format!(
            "React.createElement(\"span\", {{ dangerouslySetInnerHTML: {{ __html: {} }} }})",
            js_string(&value.value)
        ),
        Node::MdxFlowExpression(value) => expression(&value.value),
        Node::MdxTextExpression(value) => expression(&value.value),
        Node::MdxJsxFlowElement(value) => {
            render_mdx_element(value.name.as_deref(), &value.attributes, &value.children)
        }
        Node::MdxJsxTextElement(value) => {
            render_mdx_element(value.name.as_deref(), &value.attributes, &value.children)
        }
        Node::InlineMath(value) => format!(
            "React.createElement(\"span\", {{ className: \"math math-inline\" }}, {})",
            js_string(&value.value)
        ),
        Node::Math(value) => format!(
            "React.createElement(\"div\", {{ className: \"math math-display\" }}, {})",
            js_string(&value.value)
        ),
        Node::FootnoteReference(value) => format!(
            "React.createElement(\"sup\", {{ id: {} }}, {})",
            js_string(&format!("fnref-{}", value.identifier)),
            js_string(value.label.as_deref().unwrap_or(&value.identifier))
        ),
        Node::FootnoteDefinition(value) => {
            let props = format!(
                "{{ id: {} }}",
                js_string(&format!("fn-{}", value.identifier))
            );
            element("\"aside\"", &props, &value.children)
        }
        Node::LinkReference(value) => element("React.Fragment", "null", &value.children),
        Node::ImageReference(value) => js_string(&value.alt),
        Node::MdxjsEsm(_) | Node::Yaml(_) | Node::Toml(_) | Node::Definition(_) => {
            "null".to_string()
        }
    }
}

fn render_mdx_element(
    name: Option<&str>,
    attributes: &[AttributeContent],
    children: &[Node],
) -> String {
    let tag = match name {
        None => "React.Fragment".to_string(),
        Some(name)
            if name
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_lowercase()) =>
        {
            js_string(name)
        }
        Some(name) => name.to_string(),
    };
    let properties = attributes
        .iter()
        .map(|attribute| match attribute {
            AttributeContent::Expression(value) => format!("...({})", value.value),
            AttributeContent::Property(value) => {
                let property_value = match &value.value {
                    None => "true".to_string(),
                    Some(AttributeValue::Literal(value)) => js_string(value),
                    Some(AttributeValue::Expression(value)) => expression(&value.value),
                };
                format!("{}: {property_value}", js_string(&value.name))
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    element(&tag, &format!("{{ {properties} }}"), children)
}

fn expression(value: &str) -> String {
    if value.trim().is_empty() {
        "null".to_string()
    } else {
        format!("({value})")
    }
}

fn plain_text(nodes: &[Node]) -> String {
    nodes
        .iter()
        .map(|node| match node {
            Node::Text(value) => value.value.clone(),
            Node::InlineCode(value) => value.value.clone(),
            _ => node
                .children()
                .map_or_else(String::new, |nodes| plain_text(nodes)),
        })
        .collect()
}

fn collect_ast_headings(node: &Node, output: &mut Vec<Value>) {
    if let Node::Heading(heading) = node {
        let text = plain_text(&heading.children);
        output.push(json!({
            "depth": heading.depth,
            "slug": slugify(&text),
            "text": text,
        }));
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_ast_headings(child, output);
        }
    }
}

fn slugify(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in value.chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('-');
            }
            output.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    output
}

fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compiles_markdown_with_frontmatter_and_safe_image_defaults() {
        let source =
            "---\ntitle: Hello\ndraft: false\ntags: [rust, web]\n---\n# Hello\n\n![Alt](/hero.png)";
        let module = compile_content_module(source, Path::new("page.md")).unwrap();
        assert!(module.contains("\"title\":\"Hello\""));
        assert!(module.contains("\"draft\":false"));
        assert!(module.contains("loading: \"lazy\""));
        assert!(module.contains("export const headings"));
    }

    #[test]
    fn compiles_mdx_components_expressions_and_esm() {
        let source =
            "import Card from './Card'\n\n# Hello {name}\n\n<Card tone=\"info\">**Fast**</Card>";
        let module = compile_content_module(source, Path::new("page.mdx")).unwrap();
        assert!(module.contains("import Card from './Card'"));
        assert!(module.contains("React.createElement(Card"));
        assert!(module.contains("(name)"));
        assert!(module.contains("React.createElement(\"strong\""));
    }

    #[test]
    fn rejects_unclosed_frontmatter() {
        let error = compile_content_module("---\ntitle: Broken", Path::new("page.md"))
            .expect_err("frontmatter should fail");
        assert!(error.contains("RUV1312"));
    }
}
