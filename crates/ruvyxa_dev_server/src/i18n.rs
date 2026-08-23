use axum::http::{HeaderMap, header};
use ruvyxa_graph::{I18nRouting, RouteKind, RouteManifest, RouteParams};

use crate::RadixRouter;
use crate::html_document::escape_html;

pub(crate) fn locale_redirect_path(
    config: Option<&I18nRouting>,
    manifest: &RouteManifest,
    router: &RadixRouter,
    request_path: &str,
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

    #[test]
    fn cookie_wins_then_accept_language_matches_primary_subtags() {
        let config = config();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("fr-CA,th;q=.8"),
        );
        assert_eq!(preferred_locale(&config, &headers), "fr-FR");
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("x=1; RUVYXA_LOCALE=th"),
        );
        assert_eq!(preferred_locale(&config, &headers), "th");
    }

    /// Reserved paths are excluded by whole segment, not by raw byte prefix.
    /// A project route whose name merely begins with `/__ruvyxa` or `/api` is
    /// the project's own page and must still be redirected to a locale.
    #[test]
    fn reserved_prefixes_are_excluded_by_segment_not_by_byte_prefix() {
        let manifest = RouteManifest {
            app_dir: PathBuf::from("app"),
            i18n: Some(config()),
            routes: vec![RouteEntry {
                id: "app/[lang]/__ruvyxa-notes/page".into(),
                path: "/[lang]/__ruvyxa-notes".into(),
                kind: RouteKind::Page,
                file: PathBuf::from("app/[lang]/__ruvyxa-notes/page.tsx"),
                layout_chain: vec![],
                template_chain: Vec::new(),
                slots: Vec::new(),
                intercepts: Vec::new(),
                server_modules: vec![],
                client_modules: vec![],
                runtime: ruvyxa_graph::RuntimeTarget::Node,
                render: Default::default(),
            }],
        };
        let router = RadixRouter::compile(&manifest);
        let headers = HeaderMap::new();

        assert_eq!(
            locale_redirect_path(
                Some(&config()),
                &manifest,
                &router,
                "/__ruvyxa-notes",
                "GET",
                &headers
            ),
            Some("/en/__ruvyxa-notes".to_string()),
            "a project page is not a reserved endpoint just because it shares a byte prefix"
        );

        // The real reserved namespace still opts out.
        assert_eq!(
            locale_redirect_path(
                Some(&config()),
                &manifest,
                &router,
                "/__ruvyxa/flight",
                "GET",
                &headers
            ),
            None
        );
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
