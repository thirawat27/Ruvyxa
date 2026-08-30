use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use ruvyxa_diagnostics::normalized_canonical_path;

use super::*;
// The crate root re-exports the public surface; these reach the crate-private
// helpers each module keeps to itself, which is most of what is asserted here.
use crate::discovery::*;
use crate::exports::*;
use crate::graph::*;
use crate::parallel::*;
use crate::render::*;
use crate::validate::*;

fn hydration_of(source: &str) -> HydrationMode {
    parse_hydration_mode(source, &code_without_strings_and_comments(source))
}

/// The runtime a source declares, read the way the route walk reads it.
fn runtime_of(source: &str) -> Option<RuntimeTarget> {
    export_const_value(
        source,
        &code_without_strings_and_comments(source),
        "runtime",
    )
    .map_or(Some(RuntimeTarget::Node), runtime_target_from_value)
}

fn edge_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/edge-runtime-conformance.json"
    ))
    .expect("the edge runtime fixture parses")
}

/// Every spelling of `export const runtime` the shared table lists.
///
/// The declaration decides where a route is allowed to run and what it may
/// import, so a spelling read differently here than by the manifest readers
/// moves a route with nothing said.
#[test]
fn route_runtime_declarations_match_the_shared_conformance_table() {
    let fixture = edge_fixture();
    let declaration = &fixture["declaration"];
    assert_eq!(declaration["export"], "runtime");

    let cases = declaration["values"].as_array().expect("values");
    assert!(!cases.is_empty(), "the table must carry cases");
    for case in cases {
        let source = case["source"].as_str().expect("source");
        let expected = case["runtime"].as_str().expect("runtime");
        let actual = match runtime_of(source) {
            Some(RuntimeTarget::Edge) => "edge",
            Some(RuntimeTarget::Node) => "node",
            Some(RuntimeTarget::Static) => "static",
            None => "rejected",
        };
        assert_eq!(actual, expected, "{source}");
    }

    for source in declaration["rejected"].as_array().expect("rejected") {
        let source = source.as_str().expect("rejected source");
        assert_eq!(
            runtime_of(source),
            None,
            "{source} names no runtime this framework has, and defaulting it \
             to Node would place the route somewhere the author did not ask for"
        );
    }
}

/// The built-in list this crate refuses is the list the table publishes.
#[test]
fn edge_unavailable_builtins_match_the_shared_conformance_table() {
    let fixture = edge_fixture();

    let expected: Vec<&str> = fixture["unavailableOnEdge"]
        .as_array()
        .expect("unavailableOnEdge")
        .iter()
        .map(|name| name.as_str().expect("name"))
        .collect();
    assert_eq!(EDGE_UNAVAILABLE_BUILTINS, expected.as_slice());

    // Bare, prefixed, and sub-path spellings are one answer.
    assert_eq!(edge_forbidden_builtin("fs"), Some("fs"));
    assert_eq!(edge_forbidden_builtin("node:fs"), Some("fs"));
    assert_eq!(edge_forbidden_builtin("node:fs/promises"), Some("fs"));
    assert_eq!(edge_forbidden_builtin("fs/promises"), Some("fs"));

    // Everything the table calls available must pass, under both spellings:
    // a false refusal costs more than a missing one, because the missing one
    // still fails at deploy on the platform that knows its own surface.
    for name in fixture["availableOnEdge"]
        .as_array()
        .expect("availableOnEdge")
    {
        let name = name.as_str().expect("name");
        assert_eq!(edge_forbidden_builtin(name), None, "{name}");
        assert_eq!(
            edge_forbidden_builtin(&format!("node:{name}")),
            None,
            "{name}"
        );
    }

    // A package whose name merely starts with a built-in's is not that
    // built-in: `os-locale` and `fs-extra` are ordinary dependencies.
    for specifier in ["os-locale", "fs-extra", "net-utils", "@scope/vm"] {
        assert_eq!(edge_forbidden_builtin(specifier), None, "{specifier}");
    }
}

/// A route export only counts where it is real code.
///
/// These scanners read the raw source, so an `export const hydrate = false`
/// sitting in a block comment or quoted inside a documentation snippet
/// switched off the surrounding page's hydration. The same shape already
/// broke the linker once, which is why `masked_code` exists.
#[test]
fn a_route_export_inside_a_comment_or_literal_is_not_an_export() {
    assert_eq!(
        hydration_of("export const hydrate = false\n"),
        HydrationMode::None,
        "the real declaration must still be read"
    );
    assert_eq!(
        hydration_of("/*\nexport const hydrate = false\n*/\nexport default function P() {}\n"),
        HydrationMode::Load,
        "a commented-out opt-out must not disable hydration"
    );
    assert_eq!(
        hydration_of("const docs = `\nexport const hydrate = false\n`;\n"),
        HydrationMode::Load,
        "a code sample inside a template literal is text, not an export"
    );

    let quoted_ppr = "const docs = `export const ppr = true`;\n";
    assert!(
        !has_export_const_bool(
            quoted_ppr,
            &code_without_strings_and_comments(quoted_ppr),
            "ppr",
            true
        ),
        "a quoted opt-in must not switch the route to PPR"
    );
}

/// A TypeScript annotation between the name and `=` is ordinary TS, and
/// `has_export_function` beside these already tolerated one. These did not,
/// so an annotated opt-in was read as absent and the route silently fell
/// back to a different rendering strategy.
#[test]
fn an_annotated_route_export_is_still_the_export() {
    assert_eq!(
        hydration_of("export const hydrate: HydrationMode = false\n"),
        HydrationMode::None
    );
    assert_eq!(
        hydration_of("export const hydrate: 'idle' | 'visible' = 'idle'\n"),
        HydrationMode::Idle,
        "a union type contains no assignment; the value after it does"
    );

    let ppr = "export const ppr: boolean = true\n";
    assert!(has_export_const_bool(
        ppr,
        &code_without_strings_and_comments(ppr),
        "ppr",
        true
    ));

    let revalidate = "export const revalidate: number = 3600\n";
    assert_eq!(
        parse_export_const_number(
            revalidate,
            &code_without_strings_and_comments(revalidate),
            "revalidate"
        ),
        Some(3600)
    );

    // An arrow inside the annotation is not the assignment.
    let typed_arrow = "export const revalidate: (() => number) extends never ? 1 : number = 60\n";
    assert_eq!(
        parse_export_const_number(
            typed_arrow,
            &code_without_strings_and_comments(typed_arrow),
            "revalidate"
        ),
        Some(60)
    );
}

/// A longer identifier that merely starts with the same characters is a
/// different export, and a trailing comment is not part of the value.
#[test]
fn route_export_matching_stops_at_the_identifier_and_the_comment() {
    assert_eq!(
        hydration_of("export const hydrateAll = false\n"),
        HydrationMode::Load,
        "`hydrateAll` is not `hydrate`"
    );
    assert_eq!(
        hydration_of("export const hydrate = false // keep this page static\n"),
        HydrationMode::None
    );

    let commented = "export const revalidate = 120 // refresh every two minutes\n";
    assert_eq!(
        parse_export_const_number(
            commented,
            &code_without_strings_and_comments(commented),
            "revalidate"
        ),
        Some(120)
    );
}

/// Scanner tests assert on source text directly. Production reads both
/// facts off one cached `ModuleAst`; these shadow the module-level helpers
/// so the assertions stay about the scanner rather than the cache.
fn private_env_reads(source: &str) -> Vec<String> {
    let ast = ruvyxa_bundler::ast::parse_module(source);
    crate::graph::private_env_reads(&ast)
        .map(str::to_owned)
        .collect()
}

fn import_specifiers(source: &str) -> Vec<String> {
    ruvyxa_bundler::ast::parse_module(source).import_specifiers()
}

/// `check` must allow exactly what `build` allows. A local copy of the rule
/// had lost the `NODE_ENV` exemption, so the most ordinary line in a React
/// client component raised RUV1008 while the same file built cleanly.
#[test]
fn env_boundary_rule_matches_the_bundler_that_compiles_the_bundle() {
    assert!(
        private_env_reads("const dev = process.env.NODE_ENV !== 'production'").is_empty(),
        "NODE_ENV is substituted at build time and must not be reported as a leak"
    );
    assert!(
        private_env_reads("const url = process.env.RUVYXA_PUBLIC_API_URL").is_empty(),
        "RUVYXA_PUBLIC_* is public by contract"
    );
    assert_eq!(
        private_env_reads("const secret = process.env.DATABASE_URL"),
        vec!["DATABASE_URL".to_string()],
        "a genuinely private read must still be reported"
    );

    // Same names, judged by the bundler's own predicate: the two must agree
    // name for name, or `check` and `build` have drifted again.
    for name in [
        "NODE_ENV",
        "RUVYXA_PUBLIC_API_URL",
        "DATABASE_URL",
        "API_KEY",
    ] {
        let source = format!("const value = process.env.{name}");
        assert_eq!(
            private_env_reads(&source).is_empty(),
            !ruvyxa_bundler::boundary::env_read_is_private(name),
            "check and build disagree about `{name}`"
        );
    }
}

#[test]
fn discovers_static_nested_and_dynamic_pages() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("about")).unwrap();
    fs::create_dir_all(app.join("blog/[slug]")).unwrap();
    fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
    fs::write(
        app.join("about/page.tsx"),
        "export default function About() {}",
    )
    .unwrap();
    fs::write(
        app.join("blog/[slug]/page.tsx"),
        "export default function Post() {}",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let paths = manifest
        .routes
        .iter()
        .map(|route| route.path.as_str())
        .collect::<Vec<_>>();

    assert_eq!(paths, vec!["/", "/about", "/blog/[slug]"]);
}

#[test]
fn discovers_markdown_and_mdx_pages_without_default_export_diagnostics() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("docs")).unwrap();
    fs::write(app.join("page.md"), "# Home").unwrap();
    fs::write(
        app.join("docs/page.mdx"),
        "# Docs\n\n<strong>Built in</strong>",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();
    assert_eq!(manifest.routes.len(), 2);
    assert!(report.diagnostics.is_empty());
    assert!(
        manifest
            .routes
            .iter()
            .all(|route| route.render.strategy == RenderStrategy::Ssg)
    );
}

#[test]
fn markdown_code_examples_do_not_create_graph_edges() {
    // A fenced example is display text. It used to reach the edge walk
    // unmasked — every other reader masked it first — so a documented
    // `import './config'` pulled a real module into the page's client
    // graph and raised boundary diagnostics against code the page never
    // runs.
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("config.ts"),
        "export const url = process.env.DATABASE_URL;\n",
    )
    .unwrap();
    fs::write(
        app.join("page.md"),
        "# Guide\n\nConfigure the database:\n\n```ts\nimport './config';\n```\n",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();

    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    assert_eq!(
        report.client_modules, 1,
        "only the page itself is reachable"
    );
}

#[test]
fn supports_catch_all_optional_catch_all_and_route_groups() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("docs/[...slug]")).unwrap();
    fs::create_dir_all(app.join("shop/[[...category]]")).unwrap();
    fs::create_dir_all(app.join("(marketing)/pricing")).unwrap();
    fs::write(app.join("docs/[...slug]/page.tsx"), "").unwrap();
    fs::write(app.join("shop/[[...category]]/page.tsx"), "").unwrap();
    fs::write(app.join("(marketing)/pricing/page.tsx"), "").unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let paths = manifest
        .routes
        .iter()
        .map(|route| route.path.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        paths,
        vec!["/docs/[...slug]", "/pricing", "/shop/[[...category]]"]
    );
}

#[test]
fn rejects_non_next_optional_segments_and_non_terminal_catch_all() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("shop/[[category]]")).unwrap();
    fs::write(app.join("shop/[[category]]/page.tsx"), "").unwrap();

    let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
    assert!(error.to_string().contains("RUV1002"));

    fs::remove_dir_all(app.join("shop")).unwrap();
    fs::create_dir_all(app.join("docs/[...slug]/edit")).unwrap();
    fs::write(app.join("docs/[...slug]/edit/page.tsx"), "").unwrap();

    // A well-formed segment in the wrong place, which is a different
    // mistake with a different fix from a malformed one — and so a
    // different code.
    let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
    assert!(error.to_string().contains("RUV1017"));
}

fn route_pattern_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/route-pattern-conformance.json"
    ))
    .expect("the route pattern fixture parses")
}

/// Replay the shared dynamic-segment syntax table.
///
/// `tests/packages/core/route-pattern-conformance.test.ts` drives the same
/// file through `compilePattern`, which is the half that decides whether a
/// discovered segment ever captures anything. A name accepted here and not
/// recognised there is a route that exists in the manifest and matches no
/// URL, so the two halves are only correct together.
#[test]
fn replays_the_shared_route_pattern_conformance_table() {
    // Any folder does: it is carried only so the diagnostic can name it.
    let folder = Path::new("blog");

    for case in route_pattern_fixture()["segments"]
        .as_array()
        .expect("fixture declares segments")
    {
        let segment = case["segment"].as_str().expect("segment");
        let kind = case["kind"].as_str().expect("kind");
        let why = case["why"].as_str().expect("why");

        let accepted = route_segment(folder, segment, true);

        if kind == "rejected" {
            let error = accepted
                .err()
                .unwrap_or_else(|| panic!("`{segment}` must be refused: {why}"));
            let rendered = error.to_string();
            assert!(
                rendered.contains("RUV1002"),
                "`{segment}` must be refused as RUV1002, got: {rendered}"
            );
            assert!(
                rendered.contains("app/blog"),
                "the refusal of `{segment}` must name the folder to rename, got: {rendered}"
            );
            continue;
        }

        assert_eq!(
            accepted
                .unwrap_or_else(|error| panic!("`{segment}` must be accepted ({why}): {error}")),
            segment,
            "an accepted segment passes through unchanged"
        );

        // The three bracket forms are distinguished here by the only rule
        // that separates them in Rust: a catch-all consumes the rest of the
        // URL, so it cannot have a child segment.
        let not_last = route_segment(folder, segment, false);
        match kind {
            "static" | "dynamic" => assert!(
                not_last.is_ok(),
                "`{segment}` is not a catch-all and may carry a child segment"
            ),
            "catchAll" | "optionalCatchAll" => assert!(
                not_last.is_err(),
                "`{segment}` is a catch-all and must be the final URL segment"
            ),
            other => panic!("unknown kind `{other}` for `{segment}`"),
        }
    }
}

/// The defect the fixture was written from, end to end through discovery.
#[test]
fn rejects_a_dynamic_segment_the_route_matcher_would_read_as_a_literal() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("blog/[post-id]")).unwrap();
    fs::write(app.join("blog/[post-id]/page.tsx"), "").unwrap();

    let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
    let rendered = error.to_string();

    assert!(rendered.contains("RUV1002"), "got: {rendered}");
    assert!(
        rendered.contains("app/blog/[post-id]"),
        "the diagnostic must name the folder to rename, got: {rendered}"
    );
    assert!(
        rendered.contains("[postId]"),
        "the diagnostic must suggest a usable name, got: {rendered}"
    );
}

#[test]
fn private_folders_and_parallel_slots_do_not_create_routes() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("_private")).unwrap();
    fs::create_dir_all(app.join("@modal")).unwrap();
    fs::write(app.join("page.tsx"), "").unwrap();
    fs::write(app.join("_private/page.tsx"), "").unwrap();
    fs::write(app.join("@modal/page.tsx"), "").unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    assert_eq!(manifest.routes.len(), 1);
    assert_eq!(manifest.routes[0].path, "/");
}

/// Both hosts discover the same interceptions from the same tree.
///
/// The JavaScript half is
/// `tests/packages/ruvyxa/intercepting-routes-contract.test.mjs` over
/// `collectIntercepts` in `packages/ruvyxa/runtime/worker-pool.mjs`, which
/// is what `ruvyxa dev` builds its client entries from. An interception one
/// host composes and the other does not is a modal that opens in
/// production and does nothing locally.
#[test]
fn interception_discovery_matches_the_shared_conformance_table() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/intercepting-route-conformance.json"
    ))
    .unwrap();

    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        for file in case["tree"].as_array().unwrap() {
            let path = app.join(file.as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "export default function Fixture() {}").unwrap();
        }
        let route_dir = match case["routeDir"].as_str().unwrap() {
            "" => app.clone(),
            relative => app.join(relative),
        };

        let actual = route_intercepts(&app, &route_dir)
            .unwrap_or_else(|error| panic!("{name} failed discovery: {error}"))
            .into_iter()
            .map(|intercept| {
                serde_json::json!({
                    "level": intercept.level,
                    "name": intercept.name,
                    "target": intercept.target,
                    "file": intercept
                        .file
                        .strip_prefix(&app)
                        .unwrap_or(&intercept.file)
                        .display()
                        .to_string()
                        .replace('\\', "/"),
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(
            &serde_json::Value::Array(actual),
            &case["intercepts"],
            "{name} disagrees with the shared fixture"
        );
    }
}

/// Each marker resolves to the URL it actually covers.
///
/// The target comes from the *level* the slot sits on, not from the slot
/// folder, because a slot contributes no URL segment of its own. Getting
/// that wrong is invisible until a modal silently never opens.
#[test]
fn every_marker_resolves_to_the_url_it_covers() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    // The ordinary routes the interceptions stand in for.
    for real in ["photo", "feed/photo", "feed/albums/photo"] {
        fs::create_dir_all(app.join(real)).unwrap();
        fs::write(
            app.join(real).join("page.tsx"),
            "export default function Real() {}",
        )
        .unwrap();
    }
    fs::create_dir_all(app.join("feed/albums")).unwrap();
    fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
    fs::write(
        app.join("feed/albums/page.tsx"),
        "export default function Albums() {}",
    )
    .unwrap();
    fs::write(
        app.join("feed/layout.tsx"),
        "export default function L() {}",
    )
    .unwrap();

    // One slot on `app/feed/albums`, so every climb has somewhere to go.
    for (folder, _expected) in [
        ("(.)photo", "/feed/albums/photo"),
        ("(..)photo", "/feed/photo"),
        ("(..)(..)photo", "/photo"),
        ("(...)photo", "/photo"),
    ] {
        fs::create_dir_all(app.join("feed/albums/@modal").join(folder)).unwrap();
        fs::write(
            app.join("feed/albums/@modal").join(folder).join("page.tsx"),
            "export default function Modal() {}",
        )
        .unwrap();
    }
    fs::create_dir_all(app.join("feed/albums/photo")).unwrap();
    fs::write(
        app.join("feed/albums/photo/page.tsx"),
        "export default function Real() {}",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let albums = manifest
        .routes
        .iter()
        .find(|route| route.path == "/feed/albums")
        .expect("the level's own route is discovered");
    let mut targets = albums
        .intercepts
        .iter()
        .map(|intercept| (intercept.marker.as_str(), intercept.target.as_str()))
        .collect::<Vec<_>>();
    targets.sort_unstable();
    assert_eq!(
        targets,
        vec![
            ("(.)", "/feed/albums/photo"),
            ("(..)", "/feed/photo"),
            ("(..)(..)", "/photo"),
            ("(...)", "/photo"),
        ]
    );
    assert!(
        albums
            .intercepts
            .iter()
            .all(|intercept| intercept.name == "modal"),
        "every interception names the slot it renders into"
    );
}

/// An interception is carried by the routes that can show it, not by the
/// route it covers.
///
/// The intercepted route keeps its own entry untouched, which is what makes
/// a hard load render the real page instead of the overlay.
#[test]
fn an_interception_is_carried_by_the_routes_below_its_level() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("feed/@modal/(.)photo")).unwrap();
    fs::create_dir_all(app.join("feed/photo")).unwrap();
    fs::create_dir_all(app.join("elsewhere")).unwrap();
    fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
    fs::write(
        app.join("feed/layout.tsx"),
        "export default function L() {}",
    )
    .unwrap();
    fs::write(
        app.join("feed/page.tsx"),
        "export default function Feed() {}",
    )
    .unwrap();
    fs::write(
        app.join("feed/photo/page.tsx"),
        "export default function Photo() {}",
    )
    .unwrap();
    fs::write(
        app.join("feed/@modal/(.)photo/page.tsx"),
        "export default function Modal() {}",
    )
    .unwrap();
    fs::write(
        app.join("elsewhere/page.tsx"),
        "export default function Elsewhere() {}",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let by_path = |path: &str| {
        manifest
            .routes
            .iter()
            .find(|route| route.path == path)
            .unwrap_or_else(|| panic!("{path} must be discovered"))
    };

    assert_eq!(by_path("/feed").intercepts.len(), 1, "the level itself");
    assert_eq!(
        by_path("/feed/photo").intercepts.len(),
        1,
        "a route below the level composes the same layout"
    );
    assert!(
        by_path("/").intercepts.is_empty(),
        "a route above the level has no layout to render it into"
    );
    assert!(
        by_path("/elsewhere").intercepts.is_empty(),
        "a sibling route never composes that layout"
    );
}

/// An interception with no real route behind it fails the build.
#[test]
fn rejects_an_interception_whose_target_no_route_serves() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("feed/@modal/(.)phto")).unwrap();
    fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
    fs::write(
        app.join("feed/layout.tsx"),
        "export default function L() {}",
    )
    .unwrap();
    fs::write(
        app.join("feed/page.tsx"),
        "export default function Feed() {}",
    )
    .unwrap();
    fs::write(
        app.join("feed/@modal/(.)phto/page.tsx"),
        "export default function Modal() {}",
    )
    .unwrap();

    let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("RUV1006"), "{text}");
    assert!(text.contains("/feed/phto"), "{text}");
}

/// A marker cannot climb above the app root.
#[test]
fn rejects_an_interception_that_climbs_past_the_app_root() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("@modal/(..)photo")).unwrap();
    fs::create_dir_all(app.join("photo")).unwrap();
    fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
    fs::write(app.join("photo/page.tsx"), "export default function P() {}").unwrap();
    fs::write(
        app.join("@modal/(..)photo/page.tsx"),
        "export default function Modal() {}",
    )
    .unwrap();

    let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
    assert!(error.to_string().contains("RUV1018"), "{error}");
}

/// Every intercepting-route marker is refused, and refused as itself.
///
/// Before this, none of the four was stripped or reported: the route-group
/// branch needs a trailing `)`, so the folder became a literal URL segment
/// and published a page the author wrote as an interception.
#[test]
fn rejects_every_intercepting_route_marker() {
    for (folder, marker) in [
        ("(.)photo", "(.)"),
        ("(..)photo", "(..)"),
        ("(..)(..)photo", "(..)(..)"),
        ("(...)photo", "(...)"),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(app.join("feed").join(folder)).unwrap();
        fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
        fs::write(
            app.join("feed").join(folder).join("page.tsx"),
            "export default function Photo() {}",
        )
        .unwrap();

        let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
        let text = error.to_string();
        assert!(text.contains("RUV1005"), "{folder} was accepted: {text}");
        assert!(
            text.contains(marker),
            "{folder} was reported as some other convention: {text}"
        );
    }
}

/// A marker inside a parallel-route slot is an interception, not an error.
///
/// This is the shape the convention exists for — `@modal/(.)photo` is the
/// canonical Next.js modal — and it is the one place the folder has
/// somewhere to render into. It used to be rejected along with every other
/// marker, and before that it silently matched no URL and rendered nothing.
#[test]
fn an_intercepting_route_inside_a_parallel_slot_is_resolved() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("@modal/(.)photo")).unwrap();
    fs::create_dir_all(app.join("photo")).unwrap();
    fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
    fs::write(app.join("layout.tsx"), "export default function L() {}").unwrap();
    fs::write(
        app.join("photo/page.tsx"),
        "export default function Photo() {}",
    )
    .unwrap();
    fs::write(
        app.join("@modal/(.)photo/page.tsx"),
        "export default function Modal() {}",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let home = manifest
        .routes
        .iter()
        .find(|route| route.path == "/")
        .expect("the root route is discovered");
    assert_eq!(home.intercepts.len(), 1);
    assert_eq!(home.intercepts[0].target, "/photo");
    assert_eq!(home.intercepts[0].name, "modal");
    assert_eq!(home.intercepts[0].marker, "(.)");
    // The slot folder still contributes no URL of its own.
    let paths = manifest
        .routes
        .iter()
        .map(|route| route.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["/", "/photo"]);
}

/// The marker scan must not swallow the conventions beside it.
///
/// `(marketing)` opens with `(` and `@modal` is a slot; both were working
/// before the scan existed and a prefix test that is too loose would take
/// them away with no test noticing.
#[test]
fn route_groups_slots_and_private_folders_survive_the_intercept_scan() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("(marketing)/pricing")).unwrap();
    fs::create_dir_all(app.join("@modal")).unwrap();
    fs::create_dir_all(app.join("_drafts/(.)photo")).unwrap();
    fs::write(app.join("page.tsx"), "export default function Home() {}").unwrap();
    fs::write(
        app.join("(marketing)/pricing/page.tsx"),
        "export default function Pricing() {}",
    )
    .unwrap();
    fs::write(
        app.join("@modal/default.tsx"),
        "export default function M() {}",
    )
    .unwrap();
    fs::write(
        app.join("_drafts/(.)photo/page.tsx"),
        "export default function Draft() {}",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let paths = manifest
        .routes
        .iter()
        .map(|route| route.path.as_str())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec!["/", "/pricing"]);
}

#[test]
fn detects_duplicate_page_routes() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("pricing")).unwrap();
    fs::create_dir_all(app.join("(marketing)/pricing")).unwrap();
    fs::write(app.join("pricing/page.tsx"), "").unwrap();
    fs::write(app.join("(marketing)/pricing/page.tsx"), "").unwrap();

    let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
    assert!(error.to_string().contains("RUV1003"));
}

#[test]
fn detects_routes_with_equivalent_dynamic_shapes() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("blog/[slug]")).unwrap();
    fs::create_dir_all(app.join("blog/[id]")).unwrap();
    fs::write(app.join("blog/[slug]/page.tsx"), "").unwrap();
    fs::write(app.join("blog/[id]/page.tsx"), "").unwrap();

    let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
    assert!(error.to_string().contains("RUV1003"));
}

#[test]
fn rejects_page_and_route_handler_at_the_same_segment() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app/api");
    fs::create_dir_all(&app).unwrap();
    fs::write(app.join("page.tsx"), "").unwrap();
    fs::write(app.join("route.ts"), "").unwrap();

    let error = discover_routes(DiscoverOptions::new(temp.path().join("app"))).unwrap_err();
    assert!(error.to_string().contains("RUV1003"));
}

/// The opt-in is read the way every other route export is: from masked
/// source. Reading raw text would let a commented-out line, or the same
/// words inside a template literal, silently change a route's rendering
/// pipeline — which is how `hydrate` was misread twice before.
#[test]
fn reads_the_server_components_opt_in_from_code_only() {
    let cases = [
        ("export const serverComponents = true\n", true),
        ("export const serverComponents: boolean = true\n", true),
        ("export const serverComponents = false\n", false),
        ("// export const serverComponents = true\n", false),
        ("/*\nexport const serverComponents = true\n*/\n", false),
        (
            "const doc = `\nexport const serverComponents = true\n`;\n",
            false,
        ),
        ("export const serverComponentsAll = true\n", false),
        ("", false),
    ];

    for (prologue, expected) in cases {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            format!("{prologue}export default function Page() {{ return null }}\n"),
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        assert_eq!(
            manifest.routes[0].render.server_components, expected,
            "{prologue:?}"
        );
    }
}

/// `'use client'` is read by the bundler's scanner here too, not by a
/// second text search.
///
/// `str::trim_start` does not strip U+FEFF — it is `Cf`, not whitespace —
/// and `starts_with` cannot see past a leading comment, so the hand-rolled
/// check this replaced disagreed with `is_client_boundary` in the same file
/// and with both compilers. A page all of those call a client component and
/// this one called SSG is pre-rendered at build time, executing browser-only
/// code in the build's server renderer, and RUV1011 — gated on `Csr` —
/// silently never fires.
#[test]
fn reads_the_use_client_directive_through_the_shared_scanner() {
    let cases = [
        ("'use client'\n", RenderStrategy::Csr),
        ("\"use client\"\n", RenderStrategy::Csr),
        (
            "// eslint-disable-next-line no-restricted-syntax\n'use client'\n",
            RenderStrategy::Csr,
        ),
        (
            "/* the browser half of this route */\n\"use client\"\n",
            RenderStrategy::Csr,
        ),
        // A UTF-8 BOM, which Windows editors write and `trim_start` keeps.
        ("\u{feff}'use client'\n", RenderStrategy::Csr),
        // A comment is trivia; what it contains is not a directive.
        ("// 'use client'\n", RenderStrategy::Ssg),
    ];

    for (prologue, expected) in cases {
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(
            app.join("page.tsx"),
            format!("{prologue}export default function Page() {{ return null }}\n"),
        )
        .unwrap();

        let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
        assert_eq!(manifest.routes[0].render.strategy, expected, "{prologue:?}");
    }
}

/// The opt-in is orthogonal to the strategy, not a variant of it: a
/// server-components route can still revalidate on an interval.
#[test]
fn server_components_compose_with_a_rendering_strategy() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("page.tsx"),
        "export const serverComponents = true\nexport const revalidate = 60\nexport default function Page() { return null }\n",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    assert!(manifest.routes[0].render.server_components);
    assert_eq!(manifest.routes[0].render.strategy, RenderStrategy::Isr);
    assert_eq!(manifest.routes[0].render.revalidate, Some(60));
}

/// The pairing that produces a hydration mismatch nobody is told about, and
/// the two that do not.
///
/// A plugin transform is applied by the browser compile alone. Rendering
/// the same module on the server and then hydrating against the rewritten
/// version makes React throw the server markup away (#418) — which shows up
/// as a flicker, never as a failure. A `'use client'` route has no server
/// document to disagree with, and a route that ships no bundle never
/// hydrates, so neither is at risk.
#[test]
fn only_a_route_that_both_renders_and_hydrates_diverges_on_a_plugin_transform() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    for route in ["ssr", "csr", "static"] {
        fs::create_dir_all(app.join(route)).unwrap();
    }
    fs::write(
        app.join("marker.ts"),
        "export const MARKER = 'untouched'
",
    )
    .unwrap();
    fs::write(
        app.join("ssr/page.tsx"),
        "import { MARKER } from '../marker'
export default function Page() { return MARKER }
",
    )
    .unwrap();
    fs::write(
        app.join("csr/page.tsx"),
        "'use client'
import { MARKER } from '../marker'
export default function Page() { return MARKER }
",
    )
    .unwrap();
    fs::write(
        app.join("static/page.tsx"),
        "export const hydrate = false
import { MARKER } from '../marker'
export default function Page() { return MARKER }
",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let transformed = BTreeSet::from([normalized_canonical_path(&app.join("marker.ts"))]);
    let at_risk = hydrated_routes_reaching(&manifest, &transformed);

    assert_eq!(
        at_risk
            .iter()
            .map(|(route, _)| route.as_str())
            .collect::<Vec<_>>(),
        vec!["/ssr"],
        "only the route that renders on the server and hydrates can disagree with itself"
    );

    // The shared table, replayed against those three routes. It named the
    // rule and nothing checked it, which is the state a fixture exists to
    // avoid.
    //
    // The rule has two halves and they live apart on purpose. Whether the
    // plugin really produces different text for the two lanes is answered
    // by asking the plugin (`transform_differs_by_environment` in the CLI);
    // whether anything that renders on the server *and* hydrates can reach
    // the module is this function's half. A build warns only when both say
    // yes, which is what the `expect` column describes.
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/plugin-transform-lane-conformance.json");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("read {}: {error}", path.display()));
    let fixture: serde_json::Value =
        serde_json::from_str(&source).expect("the lane fixture parses");
    let cases = fixture["divergence"]["cases"]
        .as_array()
        .expect("divergence cases");
    assert!(!cases.is_empty(), "the fixture must carry cases");

    for case in cases {
        let renders = case["routeRenders"].as_bool().expect("routeRenders");
        let hydrates = case["routeHydrates"].as_bool().expect("routeHydrates");
        let client_only = case["clientOnly"].as_bool().expect("clientOnly");
        let why = case["why"].as_str().unwrap_or_default();
        let route = match (renders, hydrates) {
            (true, true) => "/ssr",
            (false, true) => "/csr",
            (true, false) => "/static",
            (false, false) => continue,
        };

        assert_eq!(
            at_risk.iter().any(|(found, _)| found == route),
            renders && hydrates,
            "{route}: the graph half is reachability alone — {why}"
        );
        assert_eq!(
            case["expect"].as_str().expect("expect") == "diverge",
            client_only && renders && hydrates,
            "{route}: a build warns only when both halves say yes — {why}"
        );
    }
}

/// The common case has to stay free: a build with no plugin transforms must
/// not walk the graph at all.
#[test]
fn no_transformed_modules_asks_no_questions() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("page.tsx"),
        "export default function Page() { return null }
",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    assert!(hydrated_routes_reaching(&manifest, &BTreeSet::new()).is_empty());
}

/// A bundle that hydrates nothing is invisible in every other report: it is
/// real, referenced, and correct, so no check flags it and the page just
/// downloads a few hundred kilobytes of React it never uses.
///
/// The signal has to be the boundary walk, not the route's declared client
/// modules: `client_modules` holds a sibling `client.tsx` by convention and
/// is empty for a route whose island is any other file, so a check written
/// against it would tell an interactive page to switch its JavaScript off.
#[test]
fn reports_a_server_components_route_whose_bundle_hydrates_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("static")).unwrap();
    fs::create_dir_all(app.join("island")).unwrap();
    fs::write(
        app.join("static/page.tsx"),
        "export const serverComponents = true
export default function Page() { return null }
",
    )
    .unwrap();
    fs::write(
        app.join("island/counter.tsx"),
        "'use client'
export default function Counter() { return null }
",
    )
    .unwrap();
    fs::write(
        app.join("island/page.tsx"),
        "export const serverComponents = true
import Counter from './counter'
export default function Page() { return Counter }
",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();

    assert_eq!(
        report.inert_hydration_routes,
        vec!["/static".to_string()],
        "only the route that reaches no client module ships a bundle for nothing"
    );
}

/// A `'use client'` page has no server half, so the export would do nothing
/// while reading as though it had moved the page's work off the browser.
#[test]
fn rejects_server_components_on_a_use_client_page() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("page.tsx"),
        "\"use client\"\nexport const serverComponents = true\nexport default function Page() { return null }\n",
    )
    .unwrap();

    let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("RUV1011"), "{text}");
    assert!(text.contains("use client"), "{text}");
}

/// Partial pre-rendering streams a shell through an entry the
/// server-components pipeline does not build.
#[test]
fn rejects_server_components_with_partial_prerendering() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("page.tsx"),
        "export const ppr = true\nexport const serverComponents = true\nexport default function Page() { return null }\n",
    )
    .unwrap();

    let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
    assert!(error.to_string().contains("RUV1019"), "{error}");
}

/// An interception is matched by the client router from a registry a
/// server-components browser entry never publishes, so the overlay would
/// simply never open.
#[test]
fn rejects_server_components_on_a_route_carrying_an_interception() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("@modal/(.)photo")).unwrap();
    fs::create_dir_all(app.join("photo")).unwrap();
    fs::write(app.join("layout.tsx"), "export default function L() {}").unwrap();
    fs::write(
        app.join("page.tsx"),
        "export const serverComponents = true\nexport default function Page() { return null }\n",
    )
    .unwrap();
    fs::write(app.join("photo/page.tsx"), "export default function P() {}").unwrap();
    fs::write(
        app.join("@modal/(.)photo/page.tsx"),
        "export default function M() {}",
    )
    .unwrap();

    let error = discover_routes(DiscoverOptions::new(&app)).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("RUV1020"), "{text}");
    assert!(text.contains("/photo"), "{text}");
}

#[test]
fn includes_action_files_as_server_modules() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("todos")).unwrap();
    fs::write(
        app.join("todos/page.tsx"),
        "export default function Todos() {}",
    )
    .unwrap();
    fs::write(app.join("todos/action.ts"), "export const createTodo = {}").unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let route = manifest
        .routes
        .iter()
        .find(|route| route.path == "/todos")
        .unwrap();

    assert_eq!(route.server_modules.len(), 1);
    assert!(route.server_modules[0].ends_with("action.ts"));
}

#[test]
fn classifies_static_pages_without_data_markers_as_ssg() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("static-page")).unwrap();
    fs::write(
        app.join("static-page/page.tsx"),
        r#"
                export default function StaticPage() {
                    return <code>.ruvyxa/prerender/static-page/index.html</code>;
                }
            "#,
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let route = manifest
        .routes
        .iter()
        .find(|route| route.path == "/static-page")
        .unwrap();

    assert_eq!(route.render.strategy, RenderStrategy::Ssg);
    assert!(!route.render.has_static_params);
    assert!(route.render.ships_client_bundle());
}

#[test]
fn hydrate_false_export_opts_pages_out_of_hydration() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("no-js")).unwrap();
    fs::write(
        app.join("no-js/page.tsx"),
        r#"
                export const hydrate = false
                export default function NoJsPage() {
                    return <h1>Content only</h1>;
                }
            "#,
    )
    .unwrap();
    fs::create_dir_all(app.join("csr-page")).unwrap();
    fs::write(
        app.join("csr-page/page.tsx"),
        r#""use client"
                export const hydrate = false
                export default function CsrPage() {
                    return <h1>Client page</h1>;
                }
            "#,
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let no_js = manifest
        .routes
        .iter()
        .find(|route| route.path == "/no-js")
        .unwrap();
    assert_eq!(no_js.render.strategy, RenderStrategy::Ssg);
    assert!(!no_js.render.ships_client_bundle());

    // 'use client' wins: CSR pages cannot opt out of client rendering.
    let csr = manifest
        .routes
        .iter()
        .find(|route| route.path == "/csr-page")
        .unwrap();
    assert_eq!(csr.render.strategy, RenderStrategy::Csr);
    assert!(csr.render.ships_client_bundle());
}

#[test]
fn hydration_string_exports_select_deferred_modes() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    for (segment, declaration) in [
        ("idle", "'idle' as const; // wait for idle"),
        ("visible", "'visible'"),
    ] {
        let route = app.join(segment);
        fs::create_dir_all(&route).unwrap();
        fs::write(
            route.join("page.tsx"),
            format!(
                "export const hydrate = {declaration};\nexport default function Page() {{ return <main>{segment}</main> }}"
            ),
        )
        .unwrap();
    }

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let idle = manifest
        .routes
        .iter()
        .find(|route| route.path == "/idle")
        .unwrap();
    let visible = manifest
        .routes
        .iter()
        .find(|route| route.path == "/visible")
        .unwrap();

    assert_eq!(idle.render.hydration, HydrationMode::Idle);
    assert_eq!(visible.render.hydration, HydrationMode::Visible);
    assert!(idle.render.ships_client_bundle() && visible.render.ships_client_bundle());
}

#[test]
fn classifies_static_params_shorthand_as_dynamic_ssg() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("articles/[slug]")).unwrap();
    fs::write(
        app.join("articles/[slug]/page.tsx"),
        "export const staticParams = ['one', 'two']; export default function Page() {}",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let route = manifest
        .routes
        .iter()
        .find(|route| route.path == "/articles/[slug]")
        .unwrap();

    assert_eq!(route.render.strategy, RenderStrategy::Ssg);
    assert!(route.render.has_static_params);
}

#[test]
fn does_not_treat_prefixed_static_params_names_as_exports() {
    assert!(!has_static_params_export(
        "export const staticParamsHelper = ['one'];"
    ));
    assert!(!has_static_params_export(
        "export function getStaticParamsHelper() {}"
    ));
}

#[test]
fn keeps_dynamic_and_data_fetching_pages_as_ssr_without_static_params() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("blog/[slug]")).unwrap();
    fs::create_dir_all(app.join("latest")).unwrap();
    fs::write(
        app.join("blog/[slug]/page.tsx"),
        "export default function Post() {}",
    )
    .unwrap();
    fs::write(
        app.join("latest/page.tsx"),
        r#"
                export default async function Latest() {
                    const response = await fetch("https://example.com/news");
                    return <main>{response.status}</main>;
                }
            "#,
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let dynamic = manifest
        .routes
        .iter()
        .find(|route| route.path == "/blog/[slug]")
        .unwrap();
    let latest = manifest
        .routes
        .iter()
        .find(|route| route.path == "/latest")
        .unwrap();

    assert_eq!(dynamic.render.strategy, RenderStrategy::Ssr);
    assert_eq!(latest.render.strategy, RenderStrategy::Ssr);
}

#[test]
fn keeps_pages_with_reachable_data_fetching_as_ssr() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("news")).unwrap();
    fs::write(
        app.join("news/page.tsx"),
        "import { load } from './data'; export default function Page() { return <main>{load}</main>; }",
    )
    .unwrap();
    fs::write(
        app.join("news/data.ts"),
        "export const load = fetch('https://example.com/news');",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    assert_eq!(manifest.routes[0].render.strategy, RenderStrategy::Ssr);
}

#[test]
fn keeps_pages_with_data_fetching_layouts_as_ssr() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("docs")).unwrap();
    fs::write(
        app.join("layout.tsx"),
        "export default function Layout({ children }) { headers(); return children; }",
    )
    .unwrap();
    fs::write(
        app.join("docs/page.tsx"),
        "export default function Page() { return <main>Docs</main>; }",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    assert_eq!(manifest.routes[0].render.strategy, RenderStrategy::Ssr);
}

/// A server component is server code, and validation has to know that.
///
/// The client compile of a server-components route stops at `'use client'`:
/// the page is serialised into a payload and never reaches a browser
/// bundle. Validating its whole graph as client code refused the two things
/// a server component exists to do — `import 'server-only'` and reading a
/// private `process.env` value — while the module they were refused in was
/// provably absent from every emitted chunk. The boundary below is the
/// dividing line, and what sits under it is still browser code.
#[test]
fn a_server_components_route_is_validated_on_the_server_side_of_its_boundary() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app").join("rsc");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("page.tsx"),
        r#"
                import "server-only";
                import Island from "./island";

                export const serverComponents = true;

                export default function Page() {
                    return <main>{process.env.DATABASE_URL}<Island /></main>;
                }
            "#,
    )
    .unwrap();
    fs::write(
        app.join("island.tsx"),
        r#"
                'use client'
                import { secret } from "./browser-secret";
                export default function Island() { return <button>{secret}</button> }
            "#,
    )
    .unwrap();
    fs::write(
        app.join("browser-secret.ts"),
        "export const secret = process.env.DATABASE_URL;\n",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(temp.path().join("app"))).unwrap();
    assert!(
        manifest.routes[0].render.server_components,
        "the fixture must opt into server components"
    );
    let report = validate_app(temp.path(), &manifest).unwrap();
    let codes = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    // The page's own `server-only` import and env read are correct here.
    assert!(
        !codes.contains(&"RUV1007"),
        "server-only is what a server component is for: {:?}",
        report.diagnostics
    );
    // The module under the client boundary is still browser code, and its
    // private env read is still a leak.
    assert_eq!(
        codes.iter().filter(|code| **code == "RUV1008").count(),
        1,
        "only the module below the client boundary leaks: {:?}",
        report.diagnostics
    );
}

#[test]
fn validates_client_and_server_boundaries() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let server = temp.path().join("server");
    fs::create_dir_all(&app).unwrap();
    fs::create_dir_all(&server).unwrap();
    fs::write(
        app.join("page.tsx"),
        r#"
                import secret from "../server/secret";

                export default function Home() {
                    return <main>{secret}</main>;
                }
            "#,
    )
    .unwrap();
    fs::write(
        server.join("secret.ts"),
        r#"
                import "server-only";

                export default process.env.DATABASE_URL;
            "#,
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();
    let codes = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RUV1007"));
    assert!(codes.contains(&"RUV1008"));
    assert!(codes.contains(&"RUV1010"));
}

#[test]
fn validates_implicit_mdx_component_providers_in_the_client_graph() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("docs")).unwrap();
    fs::write(app.join("docs/page.mdx"), "# Documentation").unwrap();
    fs::write(
        app.join("mdx-components.tsx"),
        r#"
                import "server-only";
                export function useMDXComponents(components) {
                    return { ...components, secret: process.env.DATABASE_URL };
                }
            "#,
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();
    let codes = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RUV1007"), "{codes:?}");
    assert!(codes.contains(&"RUV1008"), "{codes:?}");
}

/// Route validation used to test for the literal text `export default`, so
/// every other valid default-export form was reported as RUV1004 and a
/// commented-out one silently passed. It now shares the bundler's
/// comment-aware scanner.
#[test]
fn accepts_every_valid_default_export_form_and_still_catches_a_missing_one() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("aliased")).unwrap();
    fs::create_dir_all(app.join("reexported")).unwrap();
    fs::create_dir_all(app.join("commented")).unwrap();

    fs::write(
        app.join("page.tsx"),
        "export default function Home() { return <main /> }",
    )
    .unwrap();
    // Valid: a named binding aliased to `default`.
    fs::write(
        app.join("aliased/page.tsx"),
        "function Page() { return <main /> }\nexport { Page as default }",
    )
    .unwrap();
    // Valid: a namespace re-exported as `default`.
    fs::write(
        app.join("reexported/page.tsx"),
        "export * as default from \"../page\"",
    )
    .unwrap();
    // Invalid: the only occurrence is inside a comment.
    fs::write(
        app.join("commented/page.tsx"),
        "// export default function Page() {}\nexport const title = 'Missing'",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();
    let missing_default = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RUV1004")
        .count();

    assert_eq!(
        missing_default,
        1,
        "only the commented-out page lacks a default export, got: {:#?}",
        report
            .diagnostics
            .iter()
            .map(|diagnostic| (diagnostic.code, &diagnostic.title))
            .collect::<Vec<_>>()
    );
}

#[test]
fn validates_layouts_in_the_client_boundary_graph() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("layout.tsx"),
        r#"
                import "server-only";
                export default function Layout({ children }) {
                    return <main>{process.env.DATABASE_URL}{children}</main>;
                }
            "#,
    )
    .unwrap();
    fs::write(
        app.join("page.tsx"),
        "export default function Page() { return <p>Safe page</p>; }",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();
    let codes = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RUV1007"), "{codes:?}");
    assert!(codes.contains(&"RUV1008"), "{codes:?}");
}

/// The edge cache is shared across walks, so the second walk over a module
/// finds its edges already memoized. It must still return the full
/// reachable set — caching the edges, not the reachable set, is what keeps
/// a warm walk identical to a cold one.
#[test]
fn a_warm_edge_cache_returns_the_same_reachable_set() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("blog")).unwrap();
    fs::write(app.join("shared.ts"), "export const shared = 1;").unwrap();
    fs::write(
        app.join("layout.tsx"),
        "import { shared } from './shared'; export default function Layout() { return shared; }",
    )
    .unwrap();
    fs::write(
        app.join("page.tsx"),
        "import { shared } from './shared'; export default function Page() { return shared; }",
    )
    .unwrap();
    fs::write(
        app.join("blog/page.tsx"),
        "import { shared } from '../shared'; export default function Blog() { return shared; }",
    )
    .unwrap();

    let mut cache = ModuleCache::default();
    let cold = collect_relative_graph(&app.join("page.tsx"), &mut cache);
    // `shared.ts` is memoized by now; walking a second entry through it must
    // not short-circuit into a partial graph.
    let warm_blog = collect_relative_graph(&app.join("blog/page.tsx"), &mut cache);
    let warm_layout = collect_relative_graph(&app.join("layout.tsx"), &mut cache);
    // Repeating the first entry on a fully warm cache must be idempotent.
    let warm_repeat = collect_relative_graph(&app.join("page.tsx"), &mut cache);

    assert_eq!(cold, warm_repeat, "a warm walk must match the cold walk");
    let shared = normalized_canonical_path(&app.join("shared.ts"));
    for (label, graph) in [
        ("page", &cold),
        ("blog", &warm_blog),
        ("layout", &warm_layout),
    ] {
        assert_eq!(graph.len(), 2, "{label} graph: {graph:?}");
        assert!(
            graph.contains(&shared),
            "{label} graph lost the shared module"
        );
    }

    // Every entry above still resolves through one read of `shared.ts`.
    assert!(cache.edges.contains_key(&shared));
    assert!(cache.modules.contains_key(&shared));
}

#[test]
fn validates_dynamic_imports_and_requires_in_boundary_graphs() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("api")).unwrap();
    fs::write(
        app.join("page.tsx"),
        "export default async function Page() { return (await import('./secret')).default; }",
    )
    .unwrap();
    fs::write(
        app.join("secret.ts"),
        "import 'server-only'; export default 'secret';",
    )
    .unwrap();
    fs::write(
        app.join("api/route.ts"),
        "const browser = require('./browser'); export const GET = () => browser;",
    )
    .unwrap();
    fs::write(
        app.join("api/browser.ts"),
        "import 'client-only'; export default {}; ",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();
    let codes = report
        .diagnostics
        .iter()
        .map(|diagnostic| diagnostic.code)
        .collect::<Vec<_>>();

    assert!(codes.contains(&"RUV1007"), "{codes:?}");
    assert!(codes.contains(&"RUV1009"), "{codes:?}");
}

#[test]
fn ignores_doc_snippets_when_validating_client_env_and_imports() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("page.tsx"),
        r#"
                const docs = `
                  import secret from "../server/secret";
                  import "server-only";
                  process.env.DATABASE_URL;
                `;

                export default function Docs() {
                    return <main>{docs}</main>;
                }
            "#,
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();

    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

#[test]
fn regex_literals_do_not_blank_out_the_rest_of_a_module() {
    // A quote inside a regex character class used to open a string that ran
    // to end-of-file, blanking every later import and env read and silently
    // disabling the boundary rules for the module.
    let names =
        private_env_reads(r#"const re = /['"]/g; const secret = process.env.DATABASE_URL;"#);
    assert_eq!(names, vec!["DATABASE_URL"]);

    let specifiers = import_specifiers(
        r#"const re = /['"]/g;
import 'server-only';
"#,
    );
    assert!(
        specifiers.iter().any(|s| s == "server-only"),
        "{specifiers:?}"
    );
}

#[test]
fn division_is_not_mistaken_for_a_regex_literal() {
    let names =
        private_env_reads("const ratio = total / count; const secret = process.env.DATABASE_URL;");
    assert_eq!(names, vec!["DATABASE_URL"]);

    let names =
        private_env_reads("const ratio = (a + b) / 2 / 4; const secret = process.env.API_KEY;");
    assert_eq!(names, vec!["API_KEY"]);
}

#[test]
fn detects_literal_bracket_private_env_reads() {
    let names = private_env_reads(
        r#"const secret = process.env["DATABASE_URL"]; const docs = "process.env['EXAMPLE']";"#,
    );

    assert_eq!(names, vec!["DATABASE_URL"]);
}

#[test]
fn detects_private_env_reads_inside_template_interpolations() {
    let names = private_env_reads(
        "const label = `db: ${process.env.DATABASE_URL}`;\nconst doc = `plain process.env.IGNORED text`;",
    );

    assert_eq!(names, vec!["DATABASE_URL"]);

    let nested = private_env_reads(
        "const value = `outer ${cond ? `inner ${process.env.API_SECRET}` : \"\"}`;",
    );
    assert_eq!(nested, vec!["API_SECRET"]);
}

#[test]
fn detects_server_only_imports_inside_template_interpolations() {
    let specifiers = import_specifiers(
        "const loader = `${require(\"server-only\")}`;\nconst doc = `import \"ignored-in-text\";`;",
    );

    assert!(
        specifiers.iter().any(|s| s == "server-only"),
        "{specifiers:?}"
    );
    assert!(
        !specifiers.iter().any(|s| s == "ignored-in-text"),
        "{specifiers:?}"
    );
}

#[test]
fn bracket_env_reads_stay_index_accurate_after_multibyte_text() {
    // Thai comment before the read shifts byte offsets; blanking must be
    // byte-width preserving or the bracket lookup reads garbage.
    let names =
        private_env_reads("// คอมเมนต์ภาษาไทยก่อนหน้า\nconst secret = process.env[\"DATABASE_URL\"];");

    assert_eq!(names, vec!["DATABASE_URL"]);
}

#[test]
fn allows_server_as_a_url_route_segment() {
    let temp = tempfile::tempdir().unwrap();
    let app_server = temp.path().join("app/server");
    fs::create_dir_all(&app_server).unwrap();
    fs::write(
        app_server.join("page.tsx"),
        "export default function ServerDocs() { return <main /> }",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(temp.path().join("app"))).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();

    assert_eq!(manifest.routes[0].path, "/server");
    assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
}

#[test]
fn applies_global_isr_defaults_to_ssr_routes() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("page.tsx"),
        "export default async function Page() { return <main>{await fetch('https://example.com')}</main> }",
    )
    .unwrap();

    let manifest = discover_routes(
        DiscoverOptions::new(&app).with_rendering_defaults(Some(RenderStrategy::Isr), Some(90)),
    )
    .unwrap();

    assert_eq!(manifest.routes[0].render.strategy, RenderStrategy::Isr);
    assert_eq!(manifest.routes[0].render.revalidate, Some(90));
}

/// Which spelling an import uses is not a rendering decision.
///
/// Rule 5 of `detect_render_strategy` pre-renders a static route whose
/// reachable graph shows no data fetching, and the walk that produces that
/// graph followed relative specifiers only. An aliased import produced no
/// edge at all, so `@/lib/data` and `../../lib/data` — the same file, the
/// same `fetch` — gave the same page two different strategies, and the
/// aliased one was baked at build time and never refreshed again.
///
/// A bare package specifier is deliberately still outside the walk, and is
/// asserted here so the boundary stays a decision rather than an accident:
/// following `node_modules` would find `fetch(` in almost any dependency
/// and take automatic pre-rendering away from every page.
#[test]
fn an_aliased_import_is_followed_like_a_relative_one() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    fs::create_dir_all(app.join("news")).unwrap();
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::write(
        root.join("tsconfig.json"),
        r#"{"compilerOptions":{"baseUrl":".","paths":{"@/*":["./*"]}}}"#,
    )
    .unwrap();
    fs::write(
        root.join("lib/data.ts"),
        "export const load = fetch('https://example.com/news');",
    )
    .unwrap();

    let strategy_for = |specifier: &str| {
        fs::write(
            app.join("news/page.tsx"),
            format!(
                "import {{ load }} from '{specifier}'; \
                 export default function Page() {{ return <main>{{load}}</main>; }}"
            ),
        )
        .unwrap();
        discover_routes(DiscoverOptions::new(&app)).unwrap().routes[0]
            .render
            .strategy
    };

    assert_eq!(
        strategy_for("../../lib/data"),
        RenderStrategy::Ssr,
        "a relative import of a fetching module keeps the route dynamic"
    );
    assert_eq!(
        strategy_for("@/lib/data"),
        RenderStrategy::Ssr,
        "the same module through an alias must reach the same conclusion"
    );
    assert_eq!(
        strategy_for("my-data-lib"),
        RenderStrategy::Ssg,
        "a bare package specifier stays outside this walk on purpose"
    );
}

/// A dot in a basename is ordinary; the probe appends, it does not replace.
///
/// This walk used to build its candidates with `Path::with_extension`, which
/// replaces the last dotted segment: `./db.config` asked the filesystem for
/// `db.ts` — a file nobody wrote — and never asked for `db.config.ts`, the
/// file the bundler compiles. Three things follow from the one missing edge:
/// the module is absent from [`reachable_project_modules`] and so is never
/// staged into `<out>/server/`, its `fetch(` cannot be seen so the route is
/// baked at build time, and `validate_app` never walks it so `ruvyxa check`
/// reports clean.
#[test]
fn a_dotted_basename_import_is_probed_by_appending() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    fs::create_dir_all(app.join("news")).unwrap();
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::write(
        root.join("lib/db.config.ts"),
        "export const rows = fetch('https://example.com/rows');",
    )
    .unwrap();
    fs::write(
        app.join("news/page.tsx"),
        "import { rows } from '../../lib/db.config'; \
         export default function Page() { return <main>{rows}</main>; }",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    assert_eq!(
        manifest.routes[0].render.strategy,
        RenderStrategy::Ssr,
        "a fetch behind a dotted basename keeps the route dynamic"
    );
    assert!(
        reachable_project_modules(root, &manifest)
            .contains(&normalized_canonical_path(&root.join("lib/db.config.ts"))),
        "a dotted basename must be staged into the server output"
    );
}

/// `mjs`, `cjs`, `mts`, and `cts` are extensions this walk never probed.
///
/// The bundler's list has ten entries; this crate's private copy had six, so
/// a project whose helper was written `queue.mjs` had no edge to it at all.
#[test]
fn a_modern_extension_import_is_probed_like_the_bundler_probes_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let app = root.join("app");
    fs::create_dir_all(app.join("jobs")).unwrap();
    fs::create_dir_all(root.join("lib")).unwrap();
    fs::write(
        root.join("lib/queue.mjs"),
        "export const jobs = fetch('https://example.com/jobs');",
    )
    .unwrap();
    fs::write(
        app.join("jobs/page.tsx"),
        "import { jobs } from '../../lib/queue'; \
         export default function Page() { return <main>{jobs}</main>; }",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    assert_eq!(
        manifest.routes[0].render.strategy,
        RenderStrategy::Ssr,
        "a fetch inside a .mjs helper keeps the route dynamic"
    );
    assert!(
        reachable_project_modules(root, &manifest)
            .contains(&normalized_canonical_path(&root.join("lib/queue.mjs"))),
        "a .mjs helper must be staged into the server output"
    );
}

/// The route walk and the bundler answer a relative specifier identically.
///
/// The third replay of the `fileProbe` section of
/// `tests/fixtures/module-resolution-conformance.json`. The bundler's is in
/// `resolver.rs` and the JavaScript graph's is in
/// `tests/packages/ruvyxa/module-resolution-contract.test.mjs`; this one
/// exists because this crate carried a *third* resolver that reached the
/// conclusion the bundler's doc comment explicitly documents as wrong. The
/// walk goes through [`ModuleCache::edges`] rather than the shared function
/// directly, so the assertion is that the graph *uses* the shared probe, not
/// merely that the shared probe is correct.
#[test]
fn the_route_walk_probes_files_in_the_shared_order() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/module-resolution-conformance.json"
    ))
    .unwrap();

    for case in fixture["fileProbe"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for file in case["files"].as_array().unwrap() {
            let path = root.join(file.as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "").unwrap();
        }

        let importer = root.join("importer.ts");
        fs::write(
            &importer,
            format!(
                "import {{ value }} from '{}';\nexport const re = value;\n",
                case["specifier"].as_str().unwrap()
            ),
        )
        .unwrap();

        let canonical_root = normalized_canonical_path(root);
        let mut cache = ModuleCache::in_root(root);
        let edges = cache.edges(&importer);
        let answered = edges.first().map(|path| {
            path.strip_prefix(&canonical_root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        });

        assert_eq!(
            answered.as_deref(),
            case["expect"].as_str(),
            "{name} disagrees with the shared fixture"
        );
    }
}

/// A marker has to be a whole identifier, not a substring of one.
///
/// `prefetch(` contains `fetch(`, and `prefetch` is an API this framework
/// ships on `useRouter()` — so a page that warmed one link was read as a
/// page that fetched data and lost automatic pre-rendering. The reverse
/// direction is the dangerous one and is asserted alongside it: a member
/// access is still a call, so `globalThis.fetch(` has to keep counting.
#[test]
fn a_data_marker_must_be_its_own_identifier() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();

    let strategy_for = |body: &str| {
        fs::write(
            app.join("page.tsx"),
            format!("export default function Page() {{ {body} return <main/>; }}"),
        )
        .unwrap();
        discover_routes(DiscoverOptions::new(&app)).unwrap().routes[0]
            .render
            .strategy
    };

    for (body, why) in [
        ("router.prefetch('/products');", "prefetch is not fetch"),
        ("const value = parseheaders(raw);", "not headers()"),
        ("const value = readcookies(raw);", "not cookies()"),
        ("const value = mysearchParamsHelper;", "not searchParams"),
    ] {
        assert_eq!(strategy_for(body), RenderStrategy::Ssg, "{body} — {why}");
    }

    for (body, why) in [
        ("fetch('/api/data');", "a bare call"),
        (
            "globalThis.fetch('/api/data');",
            "a member access is still a call",
        ),
        ("await headers();", "the framework accessor"),
        ("const now = Date.now();", "a clock read"),
        (
            "const value = props.searchParams;",
            "a request-dependent prop",
        ),
        ("const key = process.env.SECRET;", "an environment read"),
    ] {
        assert_eq!(strategy_for(body), RenderStrategy::Ssr, "{body} — {why}");
    }
}

/// `export const dynamic` decides the strategy, as it does in Next.js.
///
/// A page written against that convention used to be read by nothing here:
/// `force-dynamic` on an otherwise-static page was discarded, the page was
/// pre-rendered anyway, and no diagnostic said so. Its precedence matters
/// too — `force-dynamic` outranks `revalidate`, so a page carrying both is
/// dynamic rather than ISR.
#[test]
fn the_dynamic_route_segment_config_decides_the_strategy() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();

    let strategy_for = |body: &str| {
        fs::write(
            app.join("page.tsx"),
            format!("{body}\nexport default function Page() {{ return <main/>; }}"),
        )
        .unwrap();
        let route = discover_routes(DiscoverOptions::new(&app)).unwrap().routes[0].clone();
        (route.render.strategy, route.render.revalidate)
    };

    assert_eq!(
        strategy_for("").0,
        RenderStrategy::Ssg,
        "a page with no markers is still pre-rendered by default"
    );
    assert_eq!(
        strategy_for("export const dynamic = 'force-dynamic';").0,
        RenderStrategy::Ssr,
        "force-dynamic must take the page off the pre-render path"
    );
    assert_eq!(
        strategy_for("export const dynamic = 'force-static';").0,
        RenderStrategy::Ssg
    );
    assert_eq!(
        strategy_for("export const dynamic = 'error';").0,
        RenderStrategy::Ssg,
        "error is force-static plus a runtime complaint this graph cannot make"
    );
    assert_eq!(
        strategy_for("export const dynamic = 'auto';").0,
        RenderStrategy::Ssg,
        "auto is the default and changes nothing"
    );

    // A page that reads request data is dynamic with or without the export.
    assert_eq!(
        strategy_for("export const dynamic = 'force-dynamic';\nconst now = Date.now();").0,
        RenderStrategy::Ssr
    );
    // Precedence: force-dynamic outranks an ISR opt-in.
    assert_eq!(
        strategy_for("export const dynamic = 'force-dynamic';\nexport const revalidate = 60;"),
        (RenderStrategy::Ssr, None),
        "force-dynamic outranks revalidate, as it does in Next.js"
    );
    // Without it, the ISR opt-in still wins.
    assert_eq!(
        strategy_for("export const revalidate = 60;"),
        (RenderStrategy::Isr, Some(60))
    );
    // A commented-out or quoted occurrence is not the export — the same
    // rule every other route export here is held to.
    assert_eq!(
        strategy_for("// export const dynamic = 'force-dynamic';").0,
        RenderStrategy::Ssg
    );
}

/// Next.js's name for the static parameter set is accepted.
///
/// The contract is identical — return the parameter objects to pre-render —
/// so the only thing the unfamiliar name changed was whether anything
/// noticed. A dynamic route that declared `generateStaticParams` discovered
/// as SSR and pre-rendered nothing, silently.
#[test]
fn a_dynamic_route_accepts_every_static_params_name() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("blog/[slug]")).unwrap();

    for name in STATIC_PARAMS_EXPORTS {
        fs::write(
            app.join("blog/[slug]/page.tsx"),
            format!(
                "export async function {name}() {{ return [{{ slug: 'a' }}]; }}\n\
                 export default function Page() {{ return <main/>; }}"
            ),
        )
        .unwrap();
        let route = discover_routes(DiscoverOptions::new(&app)).unwrap().routes[0].clone();
        assert_eq!(
            (route.render.strategy, route.render.has_static_params),
            (RenderStrategy::Ssg, true),
            "{name} declares a static parameter set"
        );
    }

    // A name that is not one of them stays dynamic rather than being
    // guessed at.
    fs::write(
        app.join("blog/[slug]/page.tsx"),
        "export async function makeStaticParams() { return []; }\n\
         export default function Page() { return <main/>; }",
    )
    .unwrap();
    assert_eq!(
        discover_routes(DiscoverOptions::new(&app)).unwrap().routes[0]
            .render
            .strategy,
        RenderStrategy::Ssr
    );
}

/// `template.tsx` is discovered on the same chain `layout.tsx` is.
///
/// Kept as its own chain rather than folded into `layout_chain`, because a
/// level may have either, both, or neither and composition interleaves them
/// by directory. Merging the two lists here would lose which level each
/// entry belongs to.
#[test]
fn a_template_chain_is_discovered_alongside_the_layout_chain() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(app.join("dash/reports")).unwrap();
    fs::write(
        app.join("layout.tsx"),
        "export default function L({children}) { return children }",
    )
    .unwrap();
    fs::write(
        app.join("template.tsx"),
        "export default function T({children}) { return children }",
    )
    .unwrap();
    fs::write(
        app.join("dash/layout.tsx"),
        "export default function L({children}) { return children }",
    )
    .unwrap();
    // A level with a template and no layout beside it.
    fs::write(
        app.join("dash/reports/template.tsx"),
        "export default function T({children}) { return children }",
    )
    .unwrap();
    fs::write(
        app.join("dash/reports/page.tsx"),
        "export default function Page() { return <main/>; }",
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let route = &manifest.routes[0];

    assert_eq!(route.path, "/dash/reports");
    assert_eq!(route.layout_chain, vec!["app/layout", "app/dash/layout"]);
    assert_eq!(
        route.template_chain,
        vec!["app/template", "app/dash/reports/template"],
        "root first, and only the levels that have one"
    );

    // A route with no template in scope carries an empty chain, which is
    // what keeps its emitted bundle byte-identical to before the feature.
    fs::write(
        app.join("page.tsx"),
        "export default function Home() { return <main/>; }",
    )
    .unwrap();
    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let home = manifest
        .routes
        .iter()
        .find(|route| route.path == "/")
        .expect("the home route");
    assert_eq!(
        home.template_chain,
        vec!["app/template"],
        "the root template is in scope for the root route too"
    );
}

/// Both hosts compose the same layouts and templates from the same tree.
///
/// The JavaScript half is
/// `tests/packages/ruvyxa/route-chain-contract.test.mjs` over
/// `collectLayouts`/`collectTemplates` in
/// `packages/ruvyxa/runtime/compiler.mjs`, which is what `ruvyxa dev` and
/// every deployed function compose a route from. A layout one host wraps a
/// page in and the other does not is a page that has its document shell in
/// production and loses it locally.
///
/// This side also resolves every chain entry back to a file. A chain entry
/// is an id with the extension stripped, so `app/layout` alone does not say
/// whether it came from `layout.tsx` or `layout.jsx`; a `resolve_layout_file`
/// that probes fewer extensions than the walk did names a layout nothing can
/// load, and the failure is silent — unstaged imports and a route left SSR.
#[test]
fn route_chains_match_the_shared_conformance_table() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/route-chain-conformance.json"
    ))
    .unwrap();

    for case in fixture["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let temp = tempfile::tempdir().unwrap();
        let app = temp.path().join("app");
        fs::create_dir_all(&app).unwrap();
        for file in case["tree"].as_array().unwrap() {
            let path = app.join(file.as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, "export default function Fixture() {}").unwrap();
        }
        let route_dir = match case["routeDir"].as_str().unwrap() {
            "" => app.clone(),
            relative => app.join(relative),
        };

        for (field, actual) in [
            ("layouts", layout_chain(&app, &route_dir)),
            ("templates", template_chain(&app, &route_dir)),
        ] {
            let files = case[field]
                .as_array()
                .unwrap()
                .iter()
                .map(|file| app.join(file.as_str().unwrap()))
                .collect::<Vec<_>>();
            // `route_id` is not what this case is testing, so the expected
            // ids are derived through it rather than spelled again here.
            let expected = files
                .iter()
                .map(|file| route_id(&app, file))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{name}: {field} disagree");

            for (id, file) in actual.iter().zip(&files) {
                assert_eq!(
                    resolve_layout_file(&app, id),
                    Some(normalized_canonical_path(file)),
                    "{name}: {id} does not resolve back to the file it came from"
                );
            }
        }
    }
}

/// A `@name` folder declares a slot the level's layout receives as a prop.
///
/// Slots match the URL independently of the page, which is the whole point:
/// `/dashboard/reports` renders the page from `reports/page.tsx` and the
/// team panel from `@team/reports/page.tsx` at the same time. A slot with
/// nothing for the current URL falls back to its `default.tsx`.
///
/// Before this, a `@name` directory was pruned from the walk and produced
/// nothing at all — a project that wrote one got no route, no slot, and no
/// diagnostic.
#[test]
fn a_parallel_slot_resolves_against_the_url_below_its_level() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let page = "export default function P() { return <main/>; }";
    fs::create_dir_all(app.join("dashboard/reports")).unwrap();
    fs::create_dir_all(app.join("dashboard/@team/reports")).unwrap();
    fs::create_dir_all(app.join("dashboard/@activity")).unwrap();
    fs::write(
        app.join("dashboard/layout.tsx"),
        "export default function L({children}) { return children }",
    )
    .unwrap();
    fs::write(app.join("dashboard/page.tsx"), page).unwrap();
    fs::write(app.join("dashboard/reports/page.tsx"), page).unwrap();
    // The team slot has a page for both URLs.
    fs::write(app.join("dashboard/@team/page.tsx"), page).unwrap();
    fs::write(app.join("dashboard/@team/reports/page.tsx"), page).unwrap();
    // The activity slot has a page for the index only, and a default for
    // everything else.
    fs::write(app.join("dashboard/@activity/page.tsx"), page).unwrap();
    fs::write(app.join("dashboard/@activity/default.tsx"), page).unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let slots_for = |path: &str| {
        manifest
            .routes
            .iter()
            .find(|route| route.path == path)
            .unwrap_or_else(|| panic!("no route {path}"))
            .slots
            .iter()
            .map(|slot| {
                (
                    slot.name.clone(),
                    slot.level.clone(),
                    slot.file
                        .strip_prefix(&app)
                        .unwrap_or(&slot.file)
                        .display()
                        .to_string()
                        .replace('\\', "/"),
                )
            })
            .collect::<Vec<_>>()
    };

    // A `@name` folder is still not a route of its own.
    assert_eq!(
        manifest
            .routes
            .iter()
            .map(|route| route.path.as_str())
            .collect::<Vec<_>>(),
        vec!["/dashboard", "/dashboard/reports"]
    );

    assert_eq!(
        slots_for("/dashboard"),
        vec![
            (
                "activity".to_string(),
                "app/dashboard".to_string(),
                "dashboard/@activity/page.tsx".to_string()
            ),
            (
                "team".to_string(),
                "app/dashboard".to_string(),
                "dashboard/@team/page.tsx".to_string()
            ),
        ],
        "named order, not filesystem order"
    );
    assert_eq!(
        slots_for("/dashboard/reports"),
        vec![
            (
                "activity".to_string(),
                "app/dashboard".to_string(),
                "dashboard/@activity/default.tsx".to_string()
            ),
            (
                "team".to_string(),
                "app/dashboard".to_string(),
                "dashboard/@team/reports/page.tsx".to_string()
            ),
        ],
        "the team slot follows the URL; the activity slot falls back"
    );
}

/// A slot with neither a matching page nor a default contributes nothing.
///
/// The layout simply does not receive the prop, which is what Next.js
/// renders for an unmatched slot with no `default.tsx`. Inventing an empty
/// element instead would put a wrapper in the tree the author never wrote.
#[test]
fn an_unmatched_slot_without_a_default_is_left_out() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let page = "export default function P() { return <main/>; }";
    fs::create_dir_all(app.join("dashboard/settings")).unwrap();
    fs::create_dir_all(app.join("dashboard/@team")).unwrap();
    fs::write(app.join("dashboard/settings/page.tsx"), page).unwrap();
    fs::write(app.join("dashboard/@team/page.tsx"), page).unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let route = manifest
        .routes
        .iter()
        .find(|route| route.path == "/dashboard/settings")
        .expect("the settings route");
    assert!(
        route.slots.is_empty(),
        "the team slot has nothing for /dashboard/settings: {:?}",
        route.slots
    );
}

/// The cache-control table, replayed from the fixture both languages read.
///
/// Held here rather than beside the deploy manifest because the function
/// moved: two Rust hosts answer with it now, and the strategy names in the
/// fixture are the serde names of [`RenderStrategy`], so a rename that broke
/// the wire format fails here too.
#[test]
fn document_cache_control_matches_the_shared_conformance_table() {
    const FIXTURE: &str = include_str!("../../../tests/fixtures/deploy-output-conformance.json");
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture json");
    let cases = fixture["documentCacheControl"]["cases"]
        .as_array()
        .expect("documentCacheControl cases");
    assert!(!cases.is_empty(), "the table must carry cases");
    for case in cases {
        let strategy: RenderStrategy =
            serde_json::from_value(case["strategy"].clone()).expect("strategy");
        let revalidate = case["revalidate"].as_u64();
        assert_eq!(
            document_cache_control(strategy, revalidate),
            case["expect"].as_str().expect("expect"),
            "{} {}",
            case["strategy"],
            case["why"].as_str().unwrap_or_default()
        );
    }
}

/// Which documents carry a validator, replayed from the same fixture.
///
/// The value is host-local and deliberately not shared; the membership is
/// the whole contract, because a host that validated an `ssr` document
/// would answer `304` for a page rendered for somebody else.
#[test]
fn document_validator_membership_matches_the_shared_conformance_table() {
    const FIXTURE: &str = include_str!("../../../tests/fixtures/deploy-output-conformance.json");
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture json");
    let cases = fixture["documentValidator"]["cases"]
        .as_array()
        .expect("documentValidator cases");
    assert!(!cases.is_empty(), "the table must carry cases");
    for case in cases {
        let strategy: RenderStrategy =
            serde_json::from_value(case["strategy"].clone()).expect("strategy");
        assert_eq!(
            document_has_validator(strategy),
            case["expect"].as_bool().expect("expect"),
            "{}",
            case["strategy"]
        );
    }
}

/// The manifest is published by replacement, not by truncating the file in
/// place.
///
/// `fs::write` truncates and then writes, so between the two a reader sees an
/// empty or partial `routes.json` — present to any check that only tests for
/// existence, and unparseable to `ruvyxa start` reading a directory that is
/// being rebuilt. A reader holding the previous document open is the
/// deterministic witness: after a truncating write its handle reads the new
/// build's bytes, and after a rename it still reads the document it opened.
#[test]
fn the_route_manifest_is_published_by_replacement_not_by_truncation() {
    use std::io::Read;

    let temp = tempfile::tempdir().unwrap();
    let output = temp.path().join(".ruvyxa/routes.json");
    let empty = RouteManifest {
        app_dir: temp.path().join("app"),
        routes: Vec::new(),
        i18n: None,
    };
    write_manifest(&empty, &output).unwrap();
    let published = fs::read_to_string(&output).unwrap();

    let mut reader = fs::File::open(&output).unwrap();

    let mut second = empty.clone();
    second.routes.push(RouteEntry {
        id: "app/blog/page".to_string(),
        path: "/blog".to_string(),
        kind: RouteKind::Page,
        file: temp.path().join("app/blog/page.tsx"),
        layout_chain: Vec::new(),
        template_chain: Vec::new(),
        slots: Vec::new(),
        intercepts: Vec::new(),
        server_modules: Vec::new(),
        client_modules: Vec::new(),
        runtime: RuntimeTarget::Node,
        render: RenderMeta::default(),
    });
    write_manifest(&second, &output).unwrap();

    let mut held = String::new();
    reader.read_to_string(&mut held).unwrap();
    assert_eq!(
        held, published,
        "a reader that opened the previous manifest must not see the next build's bytes \
         appear underneath it"
    );
    assert!(
        fs::read_to_string(&output).unwrap().contains("/blog"),
        "the published document must be the new one"
    );
    assert_eq!(
        fs::read_dir(output.parent().unwrap())
            .unwrap()
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .count(),
        0,
        "a published manifest must leave no temporary file behind"
    );
}

/// A walk that could not look reports; it does not answer "nothing there".
///
/// `WalkDir` reports a per-entry failure as an `Err` item and `fs::read_dir`
/// as an `Err` return, and all four of these walks used to drop them. The two
/// answers have the same shape and different meanings: an unreadable subtree
/// under `app/` loses every route inside it with no diagnostic, and — in
/// `reject_intercepting_routes` — loses the *refusal*, so an intercepting-route
/// folder inside it mounts a publicly reachable page the author wrote as an
/// overlay.
///
/// A directory that is not there is the deterministic stand-in for one that
/// cannot be read: it reaches the walk the same way on every platform, where a
/// permission bit does not. The permission case itself is asserted below on the
/// platform that has one.
#[test]
fn a_walk_that_cannot_read_a_directory_reports_instead_of_finding_nothing() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();
    let unreadable = app.join("unreadable");

    let refusal = reject_intercepting_routes(&unreadable)
        .expect_err("a subtree the walk cannot read may hold an unrefused interception");
    assert!(refusal.to_string().contains("RUV1021"), "{refusal}");

    let slots = route_slots(&app, &unreadable)
        .expect_err("a level the slot walk cannot read is not a level with no slots");
    assert!(slots.to_string().contains("RUV1021"), "{slots}");

    let intercepts = intercepts_at_level(&app, &unreadable)
        .expect_err("a level the interception walk cannot read is not a level with none");
    assert!(intercepts.to_string().contains("RUV1021"), "{intercepts}");

    let pages = intercept_pages(&unreadable)
        .expect_err("a slot directory the walk cannot read is not an empty slot");
    assert!(pages.to_string().contains("RUV1021"), "{pages}");
    assert!(
        pages.to_string().contains("unreadable"),
        "the diagnostic must name the directory: {pages}"
    );
}

/// The real shape of the defect, on the platform that can express it.
///
/// Skipped rather than failed when the process can read the directory anyway —
/// a container running as root ignores the mode bits, and a test that asserted
/// through that would be asserting nothing.
#[cfg(unix)]
#[test]
fn a_route_directory_with_no_read_permission_fails_the_build() {
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    let locked = app.join("admin");
    fs::create_dir_all(&locked).unwrap();
    fs::write(
        app.join("page.tsx"),
        "export default function Page() { return null }",
    )
    .unwrap();
    fs::write(
        locked.join("page.tsx"),
        "export default function Admin() { return null }",
    )
    .unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let reachable = fs::read_dir(&locked).is_ok();
    if reachable {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let error = discover_routes(DiscoverOptions::new(&app))
        .expect_err("a route directory the build cannot read must not silently vanish");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(error.to_string().contains("RUV1021"), "{error}");
}

/// One diagnostic per leaked variable, not per read.
///
/// `private_env_reads` reports occurrences in source order and unfiltered,
/// because that is the extraction contract `tests/fixtures/env-policy-conformance.json`
/// holds level with `privateEnvReads` in `packages/ruvyxa/runtime/compiler.mjs`.
/// The conclusion drawn from it is about the *variable*, so it is deduplicated
/// where it is emitted — the same guard, in the same shape, as
/// `crates/ruvyxa_bundler/src/boundary.rs`. Without it `ruvyxa check` and
/// `ruvyxa dev` reported one line per mention while `ruvyxa build` reported one
/// per name, so the two halves disagreed about how many problems one file has.
#[test]
fn a_private_env_variable_read_several_times_is_reported_once() {
    let temp = tempfile::tempdir().unwrap();
    let app = temp.path().join("app");
    fs::create_dir_all(&app).unwrap();
    fs::write(
        app.join("page.tsx"),
        r#"
                const url = process.env.DATABASE_URL;
                const token = process.env.API_TOKEN;
                const again = process.env.DATABASE_URL;
                const third = process.env.DATABASE_URL;

                export default function Home() {
                    return <main>{url}{token}{again}{third}</main>;
                }
            "#,
    )
    .unwrap();

    let manifest = discover_routes(DiscoverOptions::new(&app)).unwrap();
    let report = validate_app(temp.path(), &manifest).unwrap();
    let leaks = report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code == "RUV1008")
        .map(|diagnostic| diagnostic.explanation.clone())
        .collect::<Vec<_>>();

    assert_eq!(
        leaks.len(),
        2,
        "one diagnostic per leaked name: {:?}",
        report.diagnostics
    );
    // First-seen order, not sorted: the list still reads down the file.
    assert!(leaks[0].contains("DATABASE_URL"), "{leaks:?}");
    assert!(leaks[1].contains("API_TOKEN"), "{leaks:?}");
}

/// The synthesised `"app/layout"` resolves a `.jsx` root layout.
///
/// `render_root_not_found` in `crates/ruvyxa_dev_server/src/render_pipeline.rs`
/// builds its own `RouteEntry` for the 404 boundary and puts the string
/// `"app/layout"` in its chain — an id with no extension, because a chain entry
/// never carries one. Nothing tested that the probe behind it offers `.jsx`, and
/// the failure would have been quiet: the chain names a layout nothing can load,
/// its imports are never staged, and the route silently falls back to SSR.
///
/// Both spellings, and both shapes of the id, because the resolver offers the
/// project-root candidate and the app-relative one in that order and either
/// could be the one that answers.
#[test]
fn the_synthesised_root_layout_id_resolves_in_either_extension() {
    for extension in COMPONENT_EXTENSIONS {
        let temp = tempfile::tempdir().expect("temp dir");
        let app_dir = temp.path().join("app");
        std::fs::create_dir_all(&app_dir).expect("mkdir");
        let layout = app_dir.join(format!("layout.{extension}"));
        std::fs::write(&layout, b"export default function Layout() {}\n").expect("write");

        let resolved = crate::discovery::resolve_layout_file(&app_dir, "app/layout")
            .unwrap_or_else(|| panic!("`app/layout` did not resolve a layout.{extension}"));
        assert_eq!(
            resolved,
            ruvyxa_diagnostics::normalized_canonical_path(&layout),
            "layout.{extension}",
        );
    }
}

/// An id whose last segment carries a dot is not turned into another file.
///
/// The extension is appended, never substituted. `Path::with_extension` would
/// read `app/layout.mobile` as a stem plus an extension and probe
/// `app/layout.tsx`, answering with a layout the author did not name.
#[test]
fn a_dotted_layout_id_is_not_rewritten_into_another_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    let app_dir = temp.path().join("app");
    std::fs::create_dir_all(&app_dir).expect("mkdir");
    std::fs::write(
        app_dir.join("layout.tsx"),
        b"export default function L() {}\n",
    )
    .expect("write");

    assert_eq!(
        crate::discovery::resolve_layout_file(&app_dir, "app/layout.mobile"),
        None,
        "`app/layout.mobile` must not resolve to `app/layout.tsx`",
    );
}
