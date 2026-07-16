use ruvyxa_bundler::ast::{ImportKind, parse_module};
use ruvyxa_bundler::compiler::transform;

const MODULE_EDGES: &str = include_str!("fixtures/parser/module-edges.ts");
const ADVANCED_TYPESCRIPT: &str = include_str!("fixtures/parser/advanced-typescript.ts");
const ADVANCED_JSX: &str = include_str!("fixtures/parser/advanced-jsx.tsx");

#[test]
fn advanced_typescript_constructs_compile_individually() {
    for (name, source) in [
        (
            "interface",
            "interface Config { port: number }\nconst port = 1",
        ),
        ("type", "type Resource = { close(): void }\nconst value = 1"),
        ("enum", "const enum Mode { Development, Production = 5 }"),
        (
            "multiline-enum",
            "const enum Mode {\n  Development,\n  Production = 5,\n}\nexport { Mode }",
        ),
        ("decorator", "@sealed\nclass Service {}"),
        (
            "implements",
            "class Service implements Disposable { value = 1 }",
        ),
        (
            "satisfies",
            "const config = { port: 3000 } satisfies Config",
        ),
        (
            "typed-class-field",
            "class Service { readonly config: Config = { port: 3000 } satisfies Config }",
        ),
        ("computed-method", "class Service { [Symbol.dispose]() {} }"),
        (
            "declare",
            "declare function openResource(): Resource\nconst value = 1",
        ),
        (
            "using",
            "async function load() { await using resource = openResource(); return resource as Resource }",
        ),
    ] {
        transform(source, false).unwrap_or_else(|error| panic!("{name}: {error}"));
    }
}

#[test]
fn parses_multiline_module_edges_without_string_comment_or_member_false_positives() {
    let ast = parse_module(MODULE_EDGES);
    let imports = ast
        .imports
        .iter()
        .map(|edge| (edge.specifier.as_str(), edge.kind))
        .collect::<Vec<_>>();

    assert_eq!(
        imports,
        vec![
            ("./user.js", ImportKind::Static),
            ("./side-effect.js", ImportKind::SideEffect),
            ("./helper.js", ImportKind::ReExport),
            ("./lazy.js", ImportKind::Dynamic),
            ("./data.cjs", ImportKind::Require),
        ]
    );
    assert!(ast.exports.contains(&"loadLazy".to_string()));
    assert!(ast.exports.contains(&"loadData".to_string()));
}

#[test]
fn transforms_decorators_satisfies_implements_enums_and_using_fixture() {
    let ast = parse_module(ADVANCED_TYPESCRIPT);
    assert!(ast.has_decorators);
    assert!(ast.has_typescript);
    assert!(ast.has_enums);

    let output = transform(ADVANCED_TYPESCRIPT, false).expect("advanced TS fixture should compile");
    assert!(!output.contains("interface Config"), "{output}");
    assert!(!output.contains("type Resource"), "{output}");
    assert!(!output.contains("@sealed"), "{output}");
    assert!(!output.contains("implements Disposable"), "{output}");
    assert!(!output.contains("satisfies Config"), "{output}");
    assert!(output.contains("const Mode"), "{output}");
    assert!(output.contains("await using resource"), "{output}");
}

#[test]
fn transforms_fragments_member_tags_namespaced_tags_and_spread_props() {
    let ast = parse_module(ADVANCED_JSX);
    assert!(ast.has_jsx);
    assert!(ast.has_typescript);
    assert!(ast.exports.contains(&"Page".to_string()));

    let output = transform(ADVANCED_JSX, true).expect("advanced JSX fixture should compile");
    assert!(
        output.contains("React.createElement(React.Fragment"),
        "{output}"
    );
    assert!(output.contains("React.createElement(UI.Card"), "{output}");
    assert!(
        output.contains("React.createElement(\"svg:path\""),
        "{output}"
    );
    assert!(output.contains("Object.assign"), "{output}");
}
