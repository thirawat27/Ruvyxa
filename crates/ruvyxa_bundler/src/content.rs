//! Native Markdown and MDX-to-module compilation.
//!
//! Content files are lowered to ordinary React modules before they enter the
//! existing TypeScript/JSX compiler. This keeps resolution, boundary checks,
//! linking, source hashing, and compile caching identical for every page type.

use std::collections::BTreeMap;
use std::path::Path;

use markdown::mdast::{AlignKind, AttributeContent, AttributeValue, Node};
use markdown::{Constructs, MdxSignal, ParseOptions};
use oxc::allocator::Allocator;
use oxc::parser::Parser;
use oxc::span::SourceType;
use serde_json::{Value, json};

/// Compile a `.md` or `.mdx` document into a React ESM page module.
pub fn compile_content_module(source: &str, path: &Path) -> Result<String, String> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (frontmatter, body) = split_frontmatter(source)?;
    let frontmatter_json = parse_frontmatter(frontmatter.as_deref())?;

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
    let headings = collect_ast_headings(&tree);
    let definitions = collect_definitions(&tree);
    let mut slugger = HeadingSlugger::default();
    let children = match &tree {
        Node::Root(root) => render_children(&root.children, &definitions, &mut slugger, false),
        _ => render_node(&tree, &definitions, &mut slugger, false),
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
    let tree = markdown::to_mdast(body, &mdx_parse_options())
        .map_err(|error| format!("RUV1311: MDX parse error: {error}"))?;
    let esm = collect_mdx_esm(&tree);
    let headings = collect_ast_headings(&tree);
    let definitions = collect_definitions(&tree);
    let mut slugger = HeadingSlugger::default();
    let children = match &tree {
        Node::Root(root) => render_children(&root.children, &definitions, &mut slugger, true),
        _ => render_node(&tree, &definitions, &mut slugger, true),
    };
    let render = format!(
        "React.createElement(\"article\", {{ className: \"ruvyxa-content\", \"data-content-format\": \"mdx\" }}, {children})"
    );
    Ok(module_source(frontmatter, &headings, "mdx", &esm, &render))
}

fn mdx_parse_options() -> ParseOptions {
    let mut constructs = Constructs::gfm();
    constructs.autolink = false;
    constructs.code_indented = false;
    constructs.html_flow = false;
    constructs.html_text = false;
    constructs.mdx_esm = true;
    constructs.mdx_expression_flow = true;
    constructs.mdx_expression_text = true;
    constructs.mdx_jsx_flow = true;
    constructs.mdx_jsx_text = true;
    ParseOptions {
        constructs,
        mdx_esm_parse: Some(Box::new(parse_mdx_esm)),
        ..ParseOptions::default()
    }
}

fn parse_mdx_esm(source: &str) -> MdxSignal {
    let allocator = Allocator::default();
    let source_type = SourceType::mjs().with_typescript(true).with_jsx(true);
    let parsed = Parser::new(&allocator, source, source_type).parse();
    if parsed.diagnostics.is_empty() {
        MdxSignal::Ok
    } else {
        MdxSignal::Eof(
            "incomplete or invalid JavaScript module syntax".to_string(),
            Box::new("ruvyxa".to_string()),
            Box::new("mdx-esm".to_string()),
        )
    }
}

fn collect_mdx_esm(tree: &Node) -> String {
    match tree {
        Node::Root(root) => root
            .children
            .iter()
            .filter_map(|node| match node {
                Node::MdxjsEsm(value) => Some(value.value.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
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
    let page_parameters = if format == "mdx" {
        "({ components = {} } = {})"
    } else {
        "()"
    };
    format!(
        "import React from \"react\";\n{esm}\n{frontmatter_export}{meta_export}{headings_export}{format_export}export default function RuvyxaContentPage{page_parameters} {{ return {render_expression}; }}\n",
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
            // Preserve the final line ending: YAML block scalar chomping depends
            // on whether content ends with a newline before the delimiter.
            let frontmatter = normalized[first_line_end..closing_start].to_string();
            return Ok((Some(frontmatter), normalized[offset..].to_string()));
        }
    }

    Err("RUV1312: frontmatter starts with '---' but has no closing delimiter".to_string())
}

fn parse_frontmatter(source: Option<&str>) -> Result<Value, String> {
    let Some(source) = source else {
        return Ok(json!({}));
    };
    if source.trim().is_empty() {
        return Ok(json!({}));
    }
    let value = serde_yaml_ng::from_str::<Value>(source)
        .map_err(|error| format!("RUV1312: invalid YAML frontmatter: {error}"))?;
    if !value.is_object() {
        return Err("RUV1312: frontmatter must be a YAML mapping".to_string());
    }
    Ok(value)
}

#[derive(Clone)]
struct ResourceDefinition {
    url: String,
    title: Option<String>,
}

#[derive(Default)]
struct HeadingSlugger {
    occurrences: BTreeMap<String, usize>,
}

impl HeadingSlugger {
    fn next(&mut self, text: &str) -> String {
        let base = slugify(text);
        let occurrence = self.occurrences.entry(base.clone()).or_default();
        let slug = if *occurrence == 0 {
            base
        } else {
            format!("{base}-{occurrence}")
        };
        *occurrence += 1;
        slug
    }
}

fn collect_definitions(node: &Node) -> BTreeMap<String, ResourceDefinition> {
    let mut definitions = BTreeMap::new();
    collect_definitions_into(node, &mut definitions);
    definitions
}

fn collect_definitions_into(node: &Node, definitions: &mut BTreeMap<String, ResourceDefinition>) {
    if let Node::Definition(value) = node {
        definitions
            .entry(value.identifier.clone())
            .or_insert_with(|| ResourceDefinition {
                url: value.url.clone(),
                title: value.title.clone(),
            });
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_definitions_into(child, definitions);
        }
    }
}

fn render_children(
    children: &[Node],
    definitions: &BTreeMap<String, ResourceDefinition>,
    slugger: &mut HeadingSlugger,
    mdx: bool,
) -> String {
    if children.is_empty() {
        return "null".to_string();
    }
    children
        .iter()
        .filter(|node| {
            !matches!(
                node,
                Node::MdxjsEsm(_) | Node::Yaml(_) | Node::Toml(_) | Node::Definition(_)
            )
        })
        .map(|node| render_node(node, definitions, slugger, mdx))
        .collect::<Vec<_>>()
        .join(", ")
}

fn element(
    tag: &str,
    props: &str,
    children: &[Node],
    definitions: &BTreeMap<String, ResourceDefinition>,
    slugger: &mut HeadingSlugger,
    mdx: bool,
) -> String {
    let children = render_children(children, definitions, slugger, mdx);
    format!("React.createElement({tag}, {props}, {children})")
}

fn intrinsic_tag(tag: &str, mdx: bool) -> String {
    let literal = js_string(tag);
    if mdx {
        format!("(components[{literal}] || {literal})")
    } else {
        literal
    }
}

fn render_node(
    node: &Node,
    definitions: &BTreeMap<String, ResourceDefinition>,
    slugger: &mut HeadingSlugger,
    mdx: bool,
) -> String {
    match node {
        Node::Root(value) => element(
            "React.Fragment",
            "null",
            &value.children,
            definitions,
            slugger,
            mdx,
        ),
        Node::Paragraph(value) => element(
            &intrinsic_tag("p", mdx),
            "null",
            &value.children,
            definitions,
            slugger,
            mdx,
        ),
        Node::Heading(value) => {
            let text = plain_text(&value.children);
            let props = format!("{{ id: {} }}", js_string(&slugger.next(&text)));
            element(
                &intrinsic_tag(&format!("h{}", value.depth), mdx),
                &props,
                &value.children,
                definitions,
                slugger,
                mdx,
            )
        }
        Node::Blockquote(value) => element(
            &intrinsic_tag("blockquote", mdx),
            "null",
            &value.children,
            definitions,
            slugger,
            mdx,
        ),
        Node::List(value) => {
            let tag = if value.ordered { "ol" } else { "ul" };
            let contains_tasks = value.children.iter().any(|child| {
                matches!(child, Node::ListItem(item) if item.checked.is_some())
            });
            let mut props = Vec::new();
            if let Some(start) = value.start {
                props.push(format!("start: {start}"));
            }
            if contains_tasks {
                props.push("className: \"contains-task-list\"".to_string());
            }
            let props = if props.is_empty() {
                "null".to_string()
            } else {
                format!("{{ {} }}", props.join(", "))
            };
            element(
                &intrinsic_tag(tag, mdx),
                &props,
                &value.children,
                definitions,
                slugger,
                mdx,
            )
        }
        Node::ListItem(value) => {
            let checkbox = value.checked.map(|checked| {
                format!(
                    "React.createElement(\"input\", {{ type: \"checkbox\", checked: {checked}, disabled: true, readOnly: true }})"
                )
            });
            let mut children = value
                .children
                .iter()
                .map(|node| render_node(node, definitions, slugger, mdx))
                .collect::<Vec<_>>();
            if let Some(checkbox) = checkbox {
                children.insert(0, checkbox);
            }
            let props = if value.checked.is_some() {
                "{ className: \"task-list-item\" }"
            } else {
                "null"
            };
            format!(
                "React.createElement({}, {props}, {})",
                intrinsic_tag("li", mdx),
                children.join(", ")
            )
        }
        Node::Emphasis(value) => element(
            &intrinsic_tag("em", mdx),
            "null",
            &value.children,
            definitions,
            slugger,
            mdx,
        ),
        Node::Strong(value) => element(
            &intrinsic_tag("strong", mdx),
            "null",
            &value.children,
            definitions,
            slugger,
            mdx,
        ),
        Node::Delete(value) => element(
            &intrinsic_tag("del", mdx),
            "null",
            &value.children,
            definitions,
            slugger,
            mdx,
        ),
        Node::Link(value) => {
            let mut props = format!("{{ href: {}", js_string(&value.url));
            if let Some(title) = &value.title {
                props.push_str(&format!(", title: {}", js_string(title)));
            }
            props.push_str(" }");
            element(
                &intrinsic_tag("a", mdx),
                &props,
                &value.children,
                definitions,
                slugger,
                mdx,
            )
        }
        Node::Image(value) => {
            let title = value
                .title
                .as_ref()
                .map(|title| format!(", title: {}", js_string(title)))
                .unwrap_or_default();
            format!(
                "React.createElement({}, {{ src: {}, alt: {}, loading: \"lazy\", decoding: \"async\"{title} }})",
                intrinsic_tag("img", mdx),
                js_string(&value.url),
                js_string(&value.alt)
            )
        }
        Node::Text(value) => js_string(&value.value),
        Node::InlineCode(value) => format!(
            "React.createElement({}, null, {})",
            intrinsic_tag("code", mdx),
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
                "React.createElement({}, null, React.createElement({}, {class_name}, {}))",
                intrinsic_tag("pre", mdx),
                intrinsic_tag("code", mdx),
                js_string(&value.value)
            )
        }
        Node::Break(_) => format!("React.createElement({}, null)", intrinsic_tag("br", mdx)),
        Node::ThematicBreak(_) => {
            format!("React.createElement({}, null)", intrinsic_tag("hr", mdx))
        }
        Node::Table(value) => render_table(value, definitions, slugger, mdx),
        Node::TableRow(value) => element(
            &intrinsic_tag("tr", mdx),
            "null",
            &value.children,
            definitions,
            slugger,
            mdx,
        ),
        Node::TableCell(value) => element(
            &intrinsic_tag("td", mdx),
            "null",
            &value.children,
            definitions,
            slugger,
            mdx,
        ),
        Node::Html(value) => format!(
            "React.createElement(\"span\", {{ dangerouslySetInnerHTML: {{ __html: {} }} }})",
            js_string(&value.value)
        ),
        Node::MdxFlowExpression(value) => expression(&value.value),
        Node::MdxTextExpression(value) => expression(&value.value),
        Node::MdxJsxFlowElement(value) => {
            render_mdx_element(
                value.name.as_deref(),
                &value.attributes,
                &value.children,
                definitions,
                slugger,
            )
        }
        Node::MdxJsxTextElement(value) => {
            render_mdx_element(
                value.name.as_deref(),
                &value.attributes,
                &value.children,
                definitions,
                slugger,
            )
        }
        Node::InlineMath(value) => format!(
            "React.createElement(\"span\", {{ className: \"math math-inline\" }}, {})",
            js_string(&value.value)
        ),
        Node::Math(value) => format!(
            "React.createElement(\"div\", {{ className: \"math math-display\" }}, {})",
            js_string(&value.value)
        ),
        Node::FootnoteReference(value) => {
            let identifier = &value.identifier;
            format!(
                "React.createElement({}, {{ id: {}, role: \"doc-noteref\" }}, React.createElement({}, {{ href: {} }}, {}))",
                intrinsic_tag("sup", mdx),
                js_string(&format!("fnref-{identifier}")),
                intrinsic_tag("a", mdx),
                js_string(&format!("#fn-{identifier}")),
                js_string(value.label.as_deref().unwrap_or(identifier))
            )
        }
        Node::FootnoteDefinition(value) => {
            let props = format!(
                "{{ id: {}, role: \"doc-footnote\" }}",
                js_string(&format!("fn-{}", value.identifier))
            );
            let content = render_children(&value.children, definitions, slugger, mdx);
            format!(
                "React.createElement({}, {props}, {content}, React.createElement({}, {{ href: {}, \"aria-label\": \"Back to content\" }}, \"↩\"))",
                intrinsic_tag("aside", mdx),
                intrinsic_tag("a", mdx),
                js_string(&format!("#fnref-{}", value.identifier)),
            )
        }
        Node::LinkReference(value) => {
            if let Some(definition) = definitions.get(&value.identifier) {
                let mut props = format!("{{ href: {}", js_string(&definition.url));
                if let Some(title) = &definition.title {
                    props.push_str(&format!(", title: {}", js_string(title)));
                }
                props.push_str(" }");
                element(
                    &intrinsic_tag("a", mdx),
                    &props,
                    &value.children,
                    definitions,
                    slugger,
                    mdx,
                )
            } else {
                element(
                    "React.Fragment",
                    "null",
                    &value.children,
                    definitions,
                    slugger,
                    mdx,
                )
            }
        }
        Node::ImageReference(value) => definitions.get(&value.identifier).map_or_else(
            || js_string(&value.alt),
            |definition| {
                let title = definition
                    .title
                    .as_ref()
                    .map(|title| format!(", title: {}", js_string(title)))
                    .unwrap_or_default();
                format!(
                    "React.createElement({}, {{ src: {}, alt: {}, loading: \"lazy\", decoding: \"async\"{title} }})",
                    intrinsic_tag("img", mdx),
                    js_string(&definition.url),
                    js_string(&value.alt),
                )
            },
        ),
        Node::MdxjsEsm(_) | Node::Yaml(_) | Node::Toml(_) | Node::Definition(_) => {
            "null".to_string()
        }
    }
}

fn render_table(
    table: &markdown::mdast::Table,
    definitions: &BTreeMap<String, ResourceDefinition>,
    slugger: &mut HeadingSlugger,
    mdx: bool,
) -> String {
    let mut rows = table.children.iter();
    let header = rows.next().map_or_else(
        || "null".to_string(),
        |row| render_table_row(row, true, &table.align, definitions, slugger, mdx),
    );
    let body_rows = rows
        .map(|row| render_table_row(row, false, &table.align, definitions, slugger, mdx))
        .collect::<Vec<_>>();
    let body = if body_rows.is_empty() {
        "null".to_string()
    } else {
        format!(
            "React.createElement({}, null, {})",
            intrinsic_tag("tbody", mdx),
            body_rows.join(", ")
        )
    };
    format!(
        "React.createElement({}, null, React.createElement({}, null, {header}), {body})",
        intrinsic_tag("table", mdx),
        intrinsic_tag("thead", mdx),
    )
}

fn render_table_row(
    row: &Node,
    header: bool,
    alignments: &[AlignKind],
    definitions: &BTreeMap<String, ResourceDefinition>,
    slugger: &mut HeadingSlugger,
    mdx: bool,
) -> String {
    let Node::TableRow(row) = row else {
        return render_node(row, definitions, slugger, mdx);
    };
    let cell_tag = if header { "th" } else { "td" };
    let cells = row
        .children
        .iter()
        .enumerate()
        .map(|(index, cell)| {
            let alignment = alignments.get(index).copied().unwrap_or(AlignKind::None);
            let props = match alignment {
                AlignKind::Left => "{ style: { textAlign: \"left\" } }",
                AlignKind::Right => "{ style: { textAlign: \"right\" } }",
                AlignKind::Center => "{ style: { textAlign: \"center\" } }",
                AlignKind::None => "null",
            };
            match cell {
                Node::TableCell(cell) => element(
                    &intrinsic_tag(cell_tag, mdx),
                    props,
                    &cell.children,
                    definitions,
                    slugger,
                    mdx,
                ),
                _ => render_node(cell, definitions, slugger, mdx),
            }
        })
        .collect::<Vec<_>>();
    format!(
        "React.createElement({}, null, {})",
        intrinsic_tag("tr", mdx),
        cells.join(", ")
    )
}

fn render_mdx_element(
    name: Option<&str>,
    attributes: &[AttributeContent],
    children: &[Node],
    definitions: &BTreeMap<String, ResourceDefinition>,
    slugger: &mut HeadingSlugger,
) -> String {
    let tag = match name {
        None => "React.Fragment".to_string(),
        Some(name) if name.contains('.') => name.to_string(),
        Some(name)
            if name
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_lowercase()) =>
        {
            intrinsic_tag(name, true)
        }
        Some(name) => name.to_string(),
    };
    let properties = attributes
        .iter()
        .map(|attribute| match attribute {
            AttributeContent::Expression(value) => {
                let expression = value.value.trim();
                let expression = expression.strip_prefix("...").unwrap_or(expression);
                format!("...({expression})")
            }
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
    let properties = if properties.is_empty() {
        "null".to_string()
    } else {
        format!("{{ {properties} }}")
    };
    element(&tag, &properties, children, definitions, slugger, true)
}

fn expression(value: &str) -> String {
    if value.trim().is_empty() || is_comment_only(value) {
        "null".to_string()
    } else {
        format!("({value})")
    }
}

fn is_comment_only(mut value: &str) -> bool {
    loop {
        value = value.trim();
        if value.is_empty() || value.starts_with("//") {
            return true;
        }
        let Some(comment) = value.strip_prefix("/*") else {
            return false;
        };
        let Some(end) = comment.find("*/") else {
            return false;
        };
        value = &comment[end + 2..];
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

fn collect_ast_headings(node: &Node) -> Vec<Value> {
    let mut output = Vec::new();
    let mut slugger = HeadingSlugger::default();
    collect_ast_headings_into(node, &mut output, &mut slugger);
    output
}

fn collect_ast_headings_into(node: &Node, output: &mut Vec<Value>, slugger: &mut HeadingSlugger) {
    if let Node::Heading(heading) = node {
        let text = plain_text(&heading.children);
        output.push(json!({
            "depth": heading.depth,
            "slug": slugger.next(&text),
            "text": text,
        }));
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_ast_headings_into(child, output, slugger);
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
        assert!(module.contains("components[\"strong\"] || \"strong\""));
        assert!(module.contains("RuvyxaContentPage({ components = {} } = {})"));
    }

    #[test]
    fn compiles_mdx_with_multiline_esm_and_gfm_semantics() {
        let source = r#"import {
  Card
} from './Card.js'

export const data = {
  tone: 'info'
}

# Repeat
# Repeat

| Left | Right |
| :--- | ----: |
| one | two |

- [x] ~~shipped~~

https://example.com

<Card.Header {...data}>Hello</Card.Header>

{/* hidden */}
"#;
        let module = compile_content_module(source, Path::new("page.mdx")).unwrap();

        assert!(module.contains("import {\n  Card\n} from './Card.js'"));
        assert!(module.contains("export const data = {"));
        assert!(module.contains("components[\"th\"] || \"th\""));
        assert!(module.contains("textAlign: \"left\""));
        assert!(module.contains("textAlign: \"right\""));
        assert!(module.contains("contains-task-list"));
        assert!(module.contains("components[\"del\"] || \"del\""));
        assert!(module.contains("https://example.com"));
        assert!(module.contains("React.createElement(Card.Header"));
        assert!(module.contains("...(data)"), "{module}");
        assert!(module.contains("\"slug\":\"repeat\""));
        assert!(module.contains("\"slug\":\"repeat-1\""));
        assert!(module.contains("id: \"repeat-1\""));
        assert!(!module.contains("(/* hidden */)"));
        crate::compiler::transform_with_options(&module, false, crate::JsxRuntime::Automatic)
            .expect("generated MDX module should be valid JavaScript");
    }

    #[test]
    fn parses_nested_yaml_frontmatter_and_markdown_references() {
        let source = r#"---
title: "Ruvyxa: Content"
author:
  name: Ada
tags:
  - rust
  - mdx
summary: |
  First line.
  Second line.
---
# Links

[Framework][site]

![Logo][logo]

[site]: https://ruvyxa.example "Ruvyxa"
[logo]: /logo.png "Logo"
"#;
        let module = compile_content_module(source, Path::new("page.md")).unwrap();

        assert!(module.contains("\"author\":{\"name\":\"Ada\"}"));
        assert!(module.contains("\"tags\":[\"rust\",\"mdx\"]"));
        assert!(module.contains("First line.\\nSecond line.\\n"), "{module}");
        assert!(module.contains("href: \"https://ruvyxa.example\""));
        assert!(module.contains("src: \"/logo.png\""));
        assert!(module.contains("title: \"Logo\""));
    }

    #[test]
    fn rejects_invalid_or_non_mapping_yaml_frontmatter() {
        let invalid =
            compile_content_module("---\nauthor: [broken\n---\n# Page", Path::new("page.md"))
                .expect_err("invalid YAML should fail");
        assert!(invalid.contains("RUV1312"));

        let scalar = compile_content_module("---\nhello\n---\n# Page", Path::new("page.md"))
            .expect_err("frontmatter should be a mapping");
        assert!(scalar.contains("must be a YAML mapping"));
    }

    #[test]
    fn rejects_unclosed_frontmatter() {
        let error = compile_content_module("---\ntitle: Broken", Path::new("page.md"))
            .expect_err("frontmatter should fail");
        assert!(error.contains("RUV1312"));
    }
}
