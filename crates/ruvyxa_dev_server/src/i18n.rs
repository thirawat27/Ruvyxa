use axum::http::{HeaderMap, header};
use ruvyxa_graph::{I18nRouting, RouteKind, RouteManifest, RouteParams};

use crate::RadixRouter;
use crate::html_document::escape_html;

/// The prefixed URL an unprefixed request belongs at, or `None` to leave it
/// alone.
///
/// `request_path` is the canonical path and carries no query by construction;
/// `query` is the request target's query string without its leading `?`, and is
/// `None` for a caller that has no request URI at all — the build's prerenderer
/// is one. Both hosts built the `Location` from the path alone, so
/// `GET /about?q=hello` answered `/en/about` and every query-bearing entry
/// point on an i18n site lost its parameters on the first, unprefixed hit. A
/// 307 preserves the method and the body and says nothing about the query: the
/// query belongs to the target URI and has to be reproduced explicitly.
///
/// Mirrored by `localeRedirect` in
/// `packages/ruvyxa/runtime/serverless-handler.mjs`; both replay
/// `tests/fixtures/i18n-routing-conformance.json`.
pub(crate) fn locale_redirect_path(
    config: Option<&I18nRouting>,
    manifest: &RouteManifest,
    router: &RadixRouter,
    request_path: &str,
    query: Option<&str>,
    method: &str,
    headers: &HeaderMap,
) -> Option<String> {
    let config = config?;
    if !matches!(method, "GET" | "HEAD")
        // Matched as a whole segment, like the `/api` case below it. Every
        // reserved endpoint lives under `/__ruvyxa/`, so a bare prefix test
        // also swallowed project routes that merely start with those bytes —
        // `/__ruvyxa-notes` is a page a project may legitimately own, and it
        // was silently excluded from locale redirection.
        || request_path == "/__ruvyxa"
        || request_path.starts_with("/__ruvyxa/")
        || request_path == "/api"
        || request_path.starts_with("/api/")
        || std::path::Path::new(request_path).extension().is_some()
        || path_locale(config, request_path).is_some()
    {
        return None;
    }

    let preferred = preferred_locale(config, headers);
    let candidate = prefixed_path(preferred, request_path);
    router
        .find(manifest, &candidate)
        .filter(|matched| matched.route.kind == RouteKind::Page)
        .map(|_| candidate)
        .or_else(|| {
            let fallback = prefixed_path(&config.default_locale, request_path);
            router
                .find(manifest, &fallback)
                .filter(|matched| matched.route.kind == RouteKind::Page)
                .map(|_| fallback)
        })
        .map(|location| with_query(location, query))
}

/// Reattach the request's query to a redirect target.
///
/// An empty query is not a query: a bare `/about?` redirects to `/en/about`
/// rather than `/en/about?`, which is also what `URL.search` reports on the
/// deployed host.
///
/// The bytes are normalized on the way out, and that is the half the two hosts
/// disagreed about. This host had the raw request-target query and reproduced it
/// verbatim; the deployed host reads `URL.search`, which percent-encodes the
/// characters a URI may not carry literally. So one project deployed two ways
/// answered the same request with two different `Location` values.
///
/// `URL.search` is the right answer of the two, and not by preference. A
/// `Location` is a URI reference (RFC 9110 §10.2.2), and a literal space, `"`,
/// `<`, `>`, `` ` ``, `{`, `}`, `|`, `\` or `^` is not allowed in one — so the
/// verbatim spelling could emit a header a client is entitled to reject, while
/// the encoded spelling always names the same resource. Encoding here is
/// idempotent: an already-encoded `%20` is left alone, because `%` itself is
/// passed through.
fn with_query(location: String, query: Option<&str>) -> String {
    match query.filter(|query| !query.is_empty()) {
        Some(query) => format!("{location}?{}", encoded_query(query)),
        None => location,
    }
}

/// Normalize a query the way `URL.search` normalizes one.
///
/// Measured against Node rather than derived from the spec, because the deployed
/// host's behaviour *is* `URL`, and a rule written from the spec would have been
/// wrong in both directions: a first attempt encoded `` ` ``, `{`, `}`, `|`,
/// `\` and `^` -- all of which `URL` leaves alone -- and left tab, newline and
/// carriage return in place, which `URL` deletes outright.
///
/// What `URL` does, exactly:
///
/// - space, `"`, `<`, `>` are percent-encoded;
/// - tab, line feed and carriage return are **removed**, not encoded;
/// - other C0 controls and `DEL` are percent-encoded;
/// - everything else, `%` and `&` and `=` and `+` included, is passed through.
///
/// That last point is why this is not a general-purpose encoder. Re-encoding
/// `%` would turn one `%20` into `%2520` every time a redirect chained, and
/// re-encoding `&` or `=` would rewrite a query the caller meant literally.
///
/// Dropping the newline pair is also the one part of this with a security edge:
/// a raw CR or LF reproduced into a `Location` header is response splitting.
/// Axum's `HeaderValue` would refuse the header rather than emit it, so the
/// failure mode here was a dropped redirect rather than a split response -- but
/// removing them is both what the other host does and the answer that cannot go
/// wrong.
fn encoded_query(query: &str) -> String {
    let mut encoded = String::with_capacity(query.len());
    for byte in query.bytes() {
        match byte {
            b'\t' | b'\n' | b'\r' => {}
            b' ' => encoded.push_str("%20"),
            b'"' => encoded.push_str("%22"),
            b'<' => encoded.push_str("%3C"),
            b'>' => encoded.push_str("%3E"),
            0x00..=0x1F | 0x7F => {
                encoded.push('%');
                encoded.push_str(&format!("{byte:02X}"));
            }
            _ => encoded.push(char::from(byte)),
        }
    }
    encoded
}

pub(crate) fn localized_head(
    config: Option<&I18nRouting>,
    route_path: &str,
    request_path: &str,
    params: &RouteParams,
) -> Option<(String, String)> {
    let config = config?;
    let locale_segment = format!("[{}]", config.locale_param);
    if route_path.split('/').nth(1) != Some(locale_segment.as_str()) {
        return None;
    }
    let locale = params
        .get(&config.locale_param)
        .and_then(serde_json::Value::as_str)
        .and_then(|value| canonical_locale(config, value))?;
    let rest = request_path
        .strip_prefix('/')
        .and_then(|path| path.split_once('/').map(|(_, rest)| rest));
    let mut head = String::new();
    for alternate in &config.locales {
        let href = localized_path(alternate, rest);
        head.push_str("<link rel=\"alternate\" hreflang=\"");
        head.push_str(alternate);
        head.push_str("\" href=\"");
        head.push_str(&escape_html(&href));
        head.push_str("\">");
    }
    let default_href = localized_path(&config.default_locale, rest);
    head.push_str("<link rel=\"alternate\" hreflang=\"x-default\" href=\"");
    head.push_str(&escape_html(&default_href));
    head.push_str("\">");
    Some((locale.to_string(), head))
}

fn preferred_locale<'a>(config: &'a I18nRouting, headers: &HeaderMap) -> &'a str {
    if !config.detect_locale {
        return &config.default_locale;
    }
    if let Some(cookie) = headers
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
    {
        for pair in cookie.split(';') {
            let Some((name, value)) = pair.trim().split_once('=') else {
                continue;
            };
            if name == config.cookie
                && let Some(locale) = canonical_locale(config, value.trim())
            {
                return locale;
            }
        }
    }
    if let Some(accept) = headers
        .get(header::ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
    {
        let mut languages = accept
            .split(',')
            .filter_map(|entry| {
                let mut parts = entry.trim().split(';');
                let language = parts.next()?.trim();
                if language.is_empty() || language == "*" {
                    return None;
                }
                let quality = parts
                    .find_map(|part| part.trim().strip_prefix("q="))
                    .and_then(|value| value.parse::<f32>().ok())
                    .unwrap_or(1.0);
                Some((language, quality))
            })
            .collect::<Vec<_>>();
        languages.sort_by(|left, right| right.1.total_cmp(&left.1));
        for (language, quality) in languages {
            if quality <= 0.0 {
                continue;
            }
            if let Some(locale) = canonical_locale(config, language) {
                return locale;
            }
            let primary = language.split('-').next().unwrap_or(language);
            if let Some(locale) = config.locales.iter().find(|locale| {
                locale
                    .split('-')
                    .next()
                    .is_some_and(|part| part.eq_ignore_ascii_case(primary))
            }) {
                return locale;
            }
        }
    }
    &config.default_locale
}

fn path_locale<'a>(config: &'a I18nRouting, path: &str) -> Option<&'a str> {
    let segment = path.strip_prefix('/')?.split('/').next()?;
    canonical_locale(config, segment)
}

fn canonical_locale<'a>(config: &'a I18nRouting, locale: &str) -> Option<&'a str> {
    config
        .locales
        .iter()
        .find(|supported| supported.eq_ignore_ascii_case(locale))
        .map(String::as_str)
}

fn prefixed_path(locale: &str, request_path: &str) -> String {
    if request_path == "/" {
        format!("/{locale}")
    } else {
        format!("/{locale}{request_path}")
    }
}

fn localized_path(locale: &str, rest: Option<&str>) -> String {
    rest.filter(|rest| !rest.is_empty())
        .map_or_else(|| format!("/{locale}"), |rest| format!("/{locale}/{rest}"))
}

#[cfg(test)]
mod tests {
    /// The shared query-normalisation table.
    ///
    /// `tests/packages/ruvyxa/serverless-handler.test.mjs` replays the same
    /// cases through `URL.search`, which is the deployed host's implementation.
    /// This host used to reproduce the raw request-target bytes instead, so one
    /// project deployed two ways answered the same request with two different
    /// `Location` values.
    #[test]
    fn replays_the_shared_query_normalisation_table() {
        let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/i18n-routing-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(&fixture_path)
                .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
        )
        .expect("the i18n fixture is valid JSON");

        let cases = fixture["queryCases"]["cases"]
            .as_array()
            .expect("the fixture carries query cases");
        assert!(!cases.is_empty(), "an empty table asserts nothing");

        for case in cases {
            let name = case["name"].as_str().expect("each case is named");
            let query = case["query"].as_str().expect("each case has a query");
            let expect = case["expect"].as_str().expect("each case has an answer");
            assert_eq!(encoded_query(query), expect, "query case: {name}");

            // And through the function that actually builds the header, so the
            // normalisation cannot be correct in isolation and unreached in use.
            assert_eq!(
                with_query("/en/about".to_string(), Some(query)),
                format!("/en/about?{expect}"),
                "redirect target: {name}",
            );
        }
    }

    /// The root path keeps its query when it is redirected.
    ///
    /// `prefixed_path` special-cases `"/"` so the redirect is `/en` rather than
    /// `/en/`, and nothing exercised that branch together with a query — the two
    /// halves were tested apart. A 307 preserves the method and the body and says
    /// nothing about the query, so the query has to be reproduced explicitly; a
    /// branch that forgot it would send `/` visitors to a bare `/en` and drop
    /// whatever they arrived with, which for a marketing link is the entire
    /// point of the visit.
    ///
    /// The obvious fixture route for this is `/[lang]`, and the shared i18n
    /// table deliberately leaves it out: one dynamic segment matches any
    /// one-segment path, so `/about` would match it and the two replays would
    /// disagree for a reason that has nothing to do with locales. Its
    /// `$routesNote` says so. This lives here instead.
    #[test]
    fn the_root_path_carries_its_query_into_the_locale_redirect() {
        let config = config();
        // `/[lang]` alone, and nothing beside it. The shared fixture leaves this
        // route out on purpose -- one dynamic segment matches any one-segment
        // path, so `/about` would match `[lang]=about` and never reach the
        // redirect -- which is exactly why this case has no home there.
        let manifest = RouteManifest {
            app_dir: PathBuf::from("app"),
            i18n: Some(config.clone()),
            routes: vec![page_route("/[lang]")],
        };
        let router = RadixRouter::compile(&manifest);
        let headers = HeaderMap::new();

        for (query, expected) in [
            (None, "/en"),
            (Some(""), "/en"),
            (Some("q=hello"), "/en?q=hello"),
            (Some("a=1&b=2"), "/en?a=1&b=2"),
        ] {
            assert_eq!(
                locale_redirect_path(
                    Some(&config),
                    &manifest,
                    &router,
                    "/",
                    query,
                    "GET",
                    &headers,
                )
                .as_deref(),
                Some(expected),
                "query {query:?}",
            );
        }
    }

    /// The same rule one segment down, so the `"/"` branch is the only special
    /// case and not a place where two behaviours quietly diverged.
    #[test]
    fn a_nested_path_carries_its_query_the_same_way() {
        let config = config();
        let manifest = RouteManifest {
            app_dir: PathBuf::from("app"),
            i18n: Some(config.clone()),
            routes: vec![page_route("/[lang]/about")],
        };
        let router = RadixRouter::compile(&manifest);
        let headers = HeaderMap::new();

        assert_eq!(
            locale_redirect_path(
                Some(&config),
                &manifest,
                &router,
                "/about",
                Some("q=hello"),
                "GET",
                &headers,
            )
            .as_deref(),
            Some("/en/about?q=hello"),
        );
    }

    use super::*;
    use axum::http::HeaderValue;
    use ruvyxa_graph::RouteEntry;
    use std::path::PathBuf;

    fn config() -> I18nRouting {
        I18nRouting {
            locales: vec!["en".into(), "th".into(), "fr-FR".into()],
            default_locale: "en".into(),
            locale_param: "lang".into(),
            detect_locale: true,
            cookie: "RUVYXA_LOCALE".into(),
        }
    }

    fn page_route(path: &str) -> RouteEntry {
        RouteEntry {
            id: format!("app{path}/page"),
            path: path.into(),
            kind: RouteKind::Page,
            file: PathBuf::from(format!("app{path}/page.tsx")),
            layout_chain: vec![],
            template_chain: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
            server_modules: vec![],
            client_modules: vec![],
            runtime: ruvyxa_graph::RuntimeTarget::Node,
            render: Default::default(),
        }
    }

    /// The locale-redirect table, replayed from the fixture both hosts read.
    ///
    /// This replaced two hand-written tests that each pinned one behavior on
    /// this side alone. Both were right and neither reached the deployed
    /// handler, which is where `detectLocale: false` and the reserved-prefix
    /// rule had drifted — see the fixture's own comment.
    #[test]
    fn locale_redirects_match_the_shared_conformance_table() {
        const FIXTURE: &str = include_str!("../../../tests/fixtures/i18n-routing-conformance.json");
        let fixture: serde_json::Value = serde_json::from_str(FIXTURE).expect("fixture json");
        let table = &fixture["config"];
        let routes = fixture["routes"]
            .as_array()
            .expect("routes")
            .iter()
            .map(|path| page_route(path.as_str().expect("route path")))
            .collect::<Vec<_>>();
        let cases = fixture["cases"].as_array().expect("cases");
        assert!(!cases.is_empty(), "the table must carry cases");

        for case in cases {
            let config = I18nRouting {
                locales: table["locales"]
                    .as_array()
                    .expect("locales")
                    .iter()
                    .map(|value| value.as_str().expect("locale").to_string())
                    .collect(),
                default_locale: table["defaultLocale"].as_str().expect("default").into(),
                locale_param: table["localeParam"].as_str().expect("param").into(),
                detect_locale: case["detectLocale"].as_bool().expect("detectLocale"),
                cookie: table["cookie"].as_str().expect("cookie").into(),
            };
            let manifest = RouteManifest {
                app_dir: PathBuf::from("app"),
                i18n: Some(config.clone()),
                routes: routes.clone(),
            };
            let router = RadixRouter::compile(&manifest);
            let mut headers = HeaderMap::new();
            for (name, value) in case["headers"].as_object().expect("headers") {
                headers.insert(
                    header::HeaderName::from_bytes(name.as_bytes()).expect("header name"),
                    HeaderValue::from_str(value.as_str().expect("header value"))
                        .expect("header value"),
                );
            }

            assert_eq!(
                locale_redirect_path(
                    Some(&config),
                    &manifest,
                    &router,
                    case["path"].as_str().expect("path"),
                    case["query"].as_str(),
                    case["method"].as_str().expect("method"),
                    &headers,
                ),
                case["redirect"].as_str().map(str::to_string),
                "{} {}",
                case["path"],
                case["$why"].as_str().unwrap_or_default()
            );
        }
    }

    #[test]
    fn localized_head_uses_the_route_parameter_and_emits_x_default() {
        let route = RouteEntry {
            id: "app/[lang]/about/page".into(),
            path: "/[lang]/about".into(),
            kind: RouteKind::Page,
            file: PathBuf::from("app/[lang]/about/page.tsx"),
            layout_chain: vec![],
            template_chain: Vec::new(),
            slots: Vec::new(),
            intercepts: Vec::new(),
            server_modules: vec![],
            client_modules: vec![],
            runtime: ruvyxa_graph::RuntimeTarget::Node,
            render: Default::default(),
        };
        let params = RouteParams::from([("lang".into(), serde_json::json!("th"))]);
        let (lang, head) =
            localized_head(Some(&config()), &route.path, "/th/about", &params).unwrap();
        assert_eq!(lang, "th");
        assert!(head.contains("hreflang=\"fr-FR\" href=\"/fr-FR/about\""));
        assert!(head.contains("hreflang=\"x-default\" href=\"/en/about\""));
    }
}
