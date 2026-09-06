//! Route rules: `headers()`, `redirects()`, `rewrites()`, and `proxy.matcher`
//! from `ruvyxa.config.ts`, evaluated natively.
//!
//! The dialect is the common path-to-regexp subset. Every
//! JavaScript host evaluates the same rules through
//! `packages/@ruvyxa/core/src/route-rules.ts`; this module cannot share that
//! code, so both replay `tests/fixtures/route-rules-conformance.json`, which is
//! where the semantics are stated. Change the fixture first.
//!
//! `fancy-regex` rather than `regex`: the matcher idiom every such project
//! carries — `/((?!api|static).*)` — is a negative lookahead, which the
//! finite-automata engine refuses by design.

use std::collections::BTreeMap;

use fancy_regex::Regex;
use serde::{Deserialize, Serialize};

/// A condition on something other than the path.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RouteCondition {
    #[serde(rename = "type")]
    pub kind: ConditionKind,
    /// Not used by `host`. Header keys compare case-insensitively.
    #[serde(default)]
    pub key: Option<String>,
    /// A regex the whole value must match; named captures become parameters.
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ConditionKind {
    Header,
    Cookie,
    Query,
    Host,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeaderEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeaderRule {
    pub source: String,
    pub headers: Vec<HeaderEntry>,
    #[serde(default)]
    pub has: Vec<RouteCondition>,
    #[serde(default)]
    pub missing: Vec<RouteCondition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RedirectRule {
    pub source: String,
    pub destination: String,
    #[serde(default)]
    pub permanent: Option<bool>,
    /// Overrides `permanent`; one of the two is required.
    #[serde(default)]
    pub status_code: Option<u16>,
    #[serde(default)]
    pub has: Vec<RouteCondition>,
    #[serde(default)]
    pub missing: Vec<RouteCondition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RewriteRule {
    pub source: String,
    pub destination: String,
    #[serde(default)]
    pub has: Vec<RouteCondition>,
    #[serde(default)]
    pub missing: Vec<RouteCondition>,
}

/// `rewrites()` in its three phases. The config renderer reduces a bare list
/// to `afterFiles`, so this side only ever sees the object form.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RewritePhases {
    #[serde(default)]
    pub before_files: Vec<RewriteRule>,
    #[serde(default)]
    pub after_files: Vec<RewriteRule>,
    #[serde(default)]
    pub fallback: Vec<RewriteRule>,
}

/// One `proxy.matcher` entry. The renderer reduces a bare string to
/// `{ source }`, so this side only ever sees the object form.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MatcherEntry {
    pub source: String,
    #[serde(default)]
    pub has: Vec<RouteCondition>,
    #[serde(default)]
    pub missing: Vec<RouteCondition>,
}

/// The `proxy` block as the config renderer reports it.
///
/// `handler` is a function in `ruvyxa.config.ts` and stays in the compiled
/// config module; the renderer forwards `true` so this side knows one exists.
/// An absent `matcher` means the proxy runs on every request.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProxyConfig {
    #[serde(default)]
    pub matcher: Option<Vec<MatcherEntry>>,
    #[serde(default)]
    pub handler: Option<bool>,
}

/// The parts of a request the rules read.
#[derive(Debug, Clone, Copy)]
pub struct RuleRequest<'a> {
    /// Canonical path: decoded, no trailing slash, `/` for the root.
    pub path: &'a str,
    /// Raw query string without the leading `?`.
    pub query: Option<&'a str>,
    /// Header name/value pairs as the client sent them.
    pub headers: &'a [(String, String)],
    pub host: Option<&'a str>,
}

pub type RuleParams = BTreeMap<String, String>;

const SEGMENT: &str = "[^/]+";

/// A source pattern compiled to one anchored, case-insensitive regex.
#[derive(Debug, Clone)]
pub struct CompiledSource {
    pub source: String,
    regex: Regex,
    /// Capture names in group order; a digit string names an unnamed group.
    names: Vec<String>,
}

/// Compile a source pattern; see the fixture for the grammar.
pub fn compile_source(source: &str) -> Result<CompiledSource, String> {
    if !source.starts_with('/') {
        return Err(format!(
            "RUV1602 route rule source must start with \"/\": {source:?}"
        ));
    }
    let chars: Vec<char> = source.chars().collect();
    let mut pattern = String::new();
    let mut names = Vec::new();
    let mut unnamed = 0usize;
    let mut index = 0usize;
    while index < chars.len() {
        let char = chars[index];
        if char == '\\' {
            let Some(next) = chars.get(index + 1) else {
                return Err(format!(
                    "RUV1602 route rule source ends in an escape: {source:?}"
                ));
            };
            pattern.push_str(&escape_regex_char(*next));
            index += 2;
            continue;
        }
        if char == ':' || char == '(' {
            // A parameter owns the `/` written before it, so a modifier can
            // make the whole segment optional.
            let mut prefix = String::new();
            if pattern.ends_with("\\/") {
                pattern.truncate(pattern.len() - 2);
                prefix.push('/');
            }
            let mut name = String::new();
            if char == ':' {
                index += 1;
                while index < chars.len()
                    && (chars[index].is_ascii_alphanumeric() || chars[index] == '_')
                {
                    name.push(chars[index]);
                    index += 1;
                }
                if name.is_empty() {
                    return Err(format!(
                        "RUV1602 route rule parameter needs a name: {source:?}"
                    ));
                }
            }
            let mut capture = SEGMENT.to_string();
            if chars.get(index) == Some(&'(') {
                let Some(end) = matching_paren(&chars, index) else {
                    return Err(format!(
                        "RUV1602 route rule group is not closed: {source:?}"
                    ));
                };
                capture = chars[index + 1..end].iter().collect();
                if capture.is_empty() {
                    return Err(format!("RUV1602 route rule group is empty: {source:?}"));
                }
                index = end + 1;
            }
            if name.is_empty() {
                name = unnamed.to_string();
                unnamed += 1;
            }
            let modifier = chars
                .get(index)
                .copied()
                .filter(|m| matches!(m, '*' | '+' | '?'));
            if modifier.is_some() {
                index += 1;
            }
            names.push(name);
            let escaped_prefix = escape_regex(&prefix);
            match modifier {
                Some('*') | Some('+') => {
                    let repeated = format!("(?:{capture})(?:/(?:{capture}))*");
                    if modifier == Some('+') {
                        pattern.push_str(&format!("{escaped_prefix}({repeated})"));
                    } else {
                        pattern.push_str(&format!("(?:{escaped_prefix}({repeated}))?"));
                    }
                }
                Some('?') => pattern.push_str(&format!("(?:{escaped_prefix}({capture}))?")),
                _ => pattern.push_str(&format!("{escaped_prefix}({capture})")),
            }
            continue;
        }
        pattern.push_str(&escape_regex_char(char));
        index += 1;
    }
    // The optional trailing slash is path-to-regexp's: canonical paths never
    // carry one except the root, and it is what lets `/:path*` match `/`.
    let regex = Regex::new(&format!("(?i)^{pattern}/?$")).map_err(|error| {
        format!("RUV1602 route rule source {source:?} is not a valid pattern: {error}")
    })?;
    Ok(CompiledSource {
        source: source.to_string(),
        regex,
        names,
    })
}

fn matching_paren(chars: &[char], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut index = open;
    while index < chars.len() {
        match chars[index] {
            '\\' => index += 1,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
        index += 1;
    }
    None
}

fn escape_regex_char(char: char) -> String {
    if ".*+?^${}()|[]\\/".contains(char) {
        format!("\\{char}")
    } else {
        char.to_string()
    }
}

fn escape_regex(text: &str) -> String {
    text.chars().map(escape_regex_char).collect()
}

impl CompiledSource {
    /// Parameters when `path` matches, or `None`.
    pub fn matches(&self, path: &str) -> Option<RuleParams> {
        let captures = self.regex.captures(path).ok().flatten()?;
        let mut params = RuleParams::new();
        for (position, name) in self.names.iter().enumerate() {
            if let Some(value) = captures.get(position + 1) {
                params.insert(name.clone(), value.as_str().to_string());
            }
        }
        Some(params)
    }
}

/// A condition with its value regex compiled once.
#[derive(Debug, Clone)]
pub struct CompiledCondition {
    kind: ConditionKind,
    key: Option<String>,
    value: Option<Regex>,
}

pub fn compile_conditions(conditions: &[RouteCondition]) -> Result<Vec<CompiledCondition>, String> {
    conditions
        .iter()
        .map(|condition| {
            let value = match &condition.value {
                None => None,
                Some(value) => Some(Regex::new(&format!("^(?:{value})$")).map_err(|error| {
                    format!("RUV1602 route rule condition value {value:?} is not a valid pattern: {error}")
                })?),
            };
            Ok(CompiledCondition {
                kind: condition.kind,
                key: condition.key.clone(),
                value,
            })
        })
        .collect()
}

fn header_value<'a>(request: &RuleRequest<'a>, name: &str) -> Option<&'a str> {
    request
        .headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn cookie_value(request: &RuleRequest<'_>, name: &str) -> Option<String> {
    let header = header_value(request, "cookie")?;
    header.split(';').find_map(|part| {
        let part = part.trim();
        let (key, value) = part.split_once('=').unwrap_or((part, ""));
        (key == name).then(|| value.to_string())
    })
}

fn query_value(request: &RuleRequest<'_>, name: &str) -> Option<String> {
    parse_query(request.query?)
        .into_iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn evaluate_condition(
    condition: &CompiledCondition,
    request: &RuleRequest<'_>,
) -> Option<RuleParams> {
    let actual: String = match condition.kind {
        ConditionKind::Header => header_value(request, condition.key.as_deref()?)?.to_string(),
        ConditionKind::Cookie => cookie_value(request, condition.key.as_deref()?)?,
        ConditionKind::Query => query_value(request, condition.key.as_deref()?)?,
        ConditionKind::Host => request.host?.to_string(),
    };
    let Some(regex) = &condition.value else {
        return Some(RuleParams::new());
    };
    let captures = regex.captures(&actual).ok().flatten()?;
    let mut params = RuleParams::new();
    for name in regex.capture_names().flatten() {
        if let Some(value) = captures.name(name) {
            params.insert(name.to_string(), value.as_str().to_string());
        }
    }
    Some(params)
}

/// Every `has` must hold and no `missing` may; the merged captures, or `None`.
pub fn evaluate_conditions(
    has: &[CompiledCondition],
    missing: &[CompiledCondition],
    request: &RuleRequest<'_>,
) -> Option<RuleParams> {
    let mut params = RuleParams::new();
    for condition in has {
        params.extend(evaluate_condition(condition, request)?);
    }
    if missing
        .iter()
        .any(|condition| evaluate_condition(condition, request).is_some())
    {
        return None;
    }
    Some(params)
}

fn match_rule(
    compiled: &CompiledSource,
    has: &[CompiledCondition],
    missing: &[CompiledCondition],
    request: &RuleRequest<'_>,
) -> Option<RuleParams> {
    let mut params = compiled.matches(request.path)?;
    params.extend(evaluate_conditions(has, missing, request)?);
    Some(params)
}

/// Replace `:name` tokens (with an optional trailing modifier); an absent
/// parameter substitutes the empty string.
pub fn substitute(template: &str, params: &RuleParams) -> String {
    let mut output = String::new();
    let chars: Vec<char> = template.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        if chars[index] == ':' {
            let mut end = index + 1;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }
            if end > index + 1 {
                let name: String = chars[index + 1..end].iter().collect();
                if end < chars.len() && matches!(chars[end], '*' | '+' | '?') {
                    end += 1;
                }
                output.push_str(params.get(&name).map(String::as_str).unwrap_or(""));
                index = end;
                continue;
            }
        }
        output.push(chars[index]);
        index += 1;
    }
    output
}

fn template_uses_params(template: &str) -> bool {
    let chars: Vec<char> = template.chars().collect();
    chars
        .windows(2)
        .any(|pair| pair[0] == ':' && (pair[1].is_ascii_alphanumeric() || pair[1] == '_'))
}

#[derive(Debug, Clone)]
pub struct CompiledHeaderRule {
    pub rule: HeaderRule,
    compiled: CompiledSource,
    has: Vec<CompiledCondition>,
    missing: Vec<CompiledCondition>,
}

pub fn compile_header_rules(rules: Vec<HeaderRule>) -> Result<Vec<CompiledHeaderRule>, String> {
    rules
        .into_iter()
        .map(|rule| {
            Ok(CompiledHeaderRule {
                compiled: compile_source(&rule.source)?,
                has: compile_conditions(&rule.has)?,
                missing: compile_conditions(&rule.missing)?,
                rule,
            })
        })
        .collect()
}

/// Response headers every matching rule sets, later rules overriding earlier.
pub fn apply_header_rules(
    rules: &[CompiledHeaderRule],
    request: &RuleRequest<'_>,
) -> Vec<(String, String)> {
    // Insertion-ordered by lowercase key; a later rule replaces the value in place.
    let mut result: Vec<(String, (String, String))> = Vec::new();
    for rule in rules {
        let Some(params) = match_rule(&rule.compiled, &rule.has, &rule.missing, request) else {
            continue;
        };
        for header in &rule.rule.headers {
            let key = substitute(&header.key, &params);
            let value = substitute(&header.value, &params);
            let lower = key.to_ascii_lowercase();
            match result.iter_mut().find(|(existing, _)| *existing == lower) {
                Some(slot) => slot.1 = (key, value),
                None => result.push((lower, (key, value))),
            }
        }
    }
    result.into_iter().map(|(_, pair)| pair).collect()
}

#[derive(Debug, Clone)]
pub struct CompiledRedirectRule {
    pub rule: RedirectRule,
    compiled: CompiledSource,
    has: Vec<CompiledCondition>,
    missing: Vec<CompiledCondition>,
}

pub fn compile_redirect_rules(
    rules: Vec<RedirectRule>,
) -> Result<Vec<CompiledRedirectRule>, String> {
    rules
        .into_iter()
        .map(|rule| {
            if rule.status_code.is_none() && rule.permanent.is_none() {
                return Err(format!(
                    "RUV1602 redirect for {:?} needs permanent or statusCode",
                    rule.source
                ));
            }
            Ok(CompiledRedirectRule {
                compiled: compile_source(&rule.source)?,
                has: compile_conditions(&rule.has)?,
                missing: compile_conditions(&rule.missing)?,
                rule,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedirectDecision {
    pub location: String,
    pub status: u16,
}

/// The first redirect that matches, with the request query carried when the
/// destination has none.
pub fn match_redirect(
    rules: &[CompiledRedirectRule],
    request: &RuleRequest<'_>,
) -> Option<RedirectDecision> {
    for rule in rules {
        let Some(params) = match_rule(&rule.compiled, &rule.has, &rule.missing, request) else {
            continue;
        };
        // The destination is split before substitution: a `?` after a
        // parameter name would otherwise read as the optional modifier.
        let (base, destination_query) = split_destination(&rule.rule.destination);
        let mut location = substitute(base, &params);
        match destination_query {
            Some(query) => {
                location.push('?');
                location.push_str(&substitute(query, &params));
            }
            None => {
                if let Some(query) = request.query.filter(|query| !query.is_empty()) {
                    location.push('?');
                    location.push_str(query);
                }
            }
        }
        let status = rule
            .rule
            .status_code
            .unwrap_or(if rule.rule.permanent == Some(true) {
                308
            } else {
                307
            });
        return Some(RedirectDecision { location, status });
    }
    None
}

#[derive(Debug, Clone)]
pub struct CompiledRewriteRule {
    pub rule: RewriteRule,
    compiled: CompiledSource,
    has: Vec<CompiledCondition>,
    missing: Vec<CompiledCondition>,
}

pub fn compile_rewrite_rules(rules: Vec<RewriteRule>) -> Result<Vec<CompiledRewriteRule>, String> {
    rules
        .into_iter()
        .map(|rule| {
            Ok(CompiledRewriteRule {
                compiled: compile_source(&rule.source)?,
                has: compile_conditions(&rule.has)?,
                missing: compile_conditions(&rule.missing)?,
                rule,
            })
        })
        .collect()
}

/// The first rewrite that matches, as a target URL or path with query.
///
/// Parameters the destination does not use are appended to its query when it
/// uses none; the request's own query is merged after. The query is
/// re-serialized as `application/x-www-form-urlencoded`, as `URLSearchParams`
/// does on the JavaScript side.
pub fn match_rewrite(rules: &[CompiledRewriteRule], request: &RuleRequest<'_>) -> Option<String> {
    for rule in rules {
        let Some(params) = match_rule(&rule.compiled, &rule.has, &rule.missing, request) else {
            continue;
        };
        let uses_params = template_uses_params(&rule.rule.destination);
        let (base_template, query_template) = split_destination(&rule.rule.destination);
        let base = substitute(base_template, &params);
        let mut pairs = query_template
            .map(|query| parse_query(&substitute(query, &params)))
            .unwrap_or_default();
        if !uses_params {
            pairs.extend(params.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        if let Some(query) = request.query {
            pairs.extend(parse_query(query));
        }
        let serialized = serialize_query(&pairs);
        return Some(if serialized.is_empty() {
            base
        } else {
            format!("{base}?{serialized}")
        });
    }
    None
}

#[derive(Debug, Clone)]
pub struct CompiledMatcherEntry {
    compiled: CompiledSource,
    has: Vec<CompiledCondition>,
    missing: Vec<CompiledCondition>,
}

pub fn compile_matcher(entries: &[MatcherEntry]) -> Result<Vec<CompiledMatcherEntry>, String> {
    entries
        .iter()
        .map(|entry| {
            Ok(CompiledMatcherEntry {
                compiled: compile_source(&entry.source)?,
                has: compile_conditions(&entry.has)?,
                missing: compile_conditions(&entry.missing)?,
            })
        })
        .collect()
}

/// Every rule list a host evaluates, compiled once at startup.
#[derive(Debug, Clone, Default)]
pub struct RouteRules {
    pub headers: Vec<CompiledHeaderRule>,
    pub redirects: Vec<CompiledRedirectRule>,
    pub before_files: Vec<CompiledRewriteRule>,
    pub after_files: Vec<CompiledRewriteRule>,
    pub fallback: Vec<CompiledRewriteRule>,
}

impl RouteRules {
    pub fn compile(
        headers: Vec<HeaderRule>,
        redirects: Vec<RedirectRule>,
        rewrites: RewritePhases,
    ) -> Result<Self, String> {
        Ok(Self {
            headers: compile_header_rules(headers)?,
            redirects: compile_redirect_rules(redirects)?,
            before_files: compile_rewrite_rules(rewrites.before_files)?,
            after_files: compile_rewrite_rules(rewrites.after_files)?,
            fallback: compile_rewrite_rules(rewrites.fallback)?,
        })
    }

    /// Whether any list is non-empty, so a host can skip the stage entirely.
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
            && self.redirects.is_empty()
            && self.before_files.is_empty()
            && self.after_files.is_empty()
            && self.fallback.is_empty()
    }
}

/// The `proxy` block, compiled: which requests the host hands to the handler.
#[derive(Debug, Clone)]
pub struct CompiledProxy {
    /// `None` runs the handler on every request; `Some` only on a match.
    matcher: Option<Vec<CompiledMatcherEntry>>,
}

impl CompiledProxy {
    pub fn compile(config: &ProxyConfig) -> Result<Self, String> {
        Ok(Self {
            matcher: config.matcher.as_deref().map(compile_matcher).transpose()?,
        })
    }

    /// Whether the handler runs for this request.
    pub fn wants(&self, request: &RuleRequest<'_>) -> bool {
        match &self.matcher {
            None => true,
            Some(entries) => matcher_matches(entries, request),
        }
    }
}

/// Whether any matcher entry matches; an empty matcher matches nothing.
pub fn matcher_matches(entries: &[CompiledMatcherEntry], request: &RuleRequest<'_>) -> bool {
    entries
        .iter()
        .any(|entry| match_rule(&entry.compiled, &entry.has, &entry.missing, request).is_some())
}

/// A destination's path and query halves, split before substitution so a `?`
/// following a parameter name is a query separator and not a modifier.
fn split_destination(destination: &str) -> (&str, Option<&str>) {
    match destination.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (destination, None),
    }
}

/// Request headers as the rules read them, from axum's map.
pub fn header_pairs(headers: &axum::http::HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_string(), value.to_string()))
        })
        .collect()
}

/// The `Host` header, when it is readable text.
pub fn host_header(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
}

/// `application/x-www-form-urlencoded` parsing: `+` is a space, `%XX` a byte.
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (form_decode(key), form_decode(value))
        })
        .collect()
}

fn form_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'+' => output.push(b' '),
            b'%' if index + 2 < bytes.len() => {
                match u8::from_str_radix(&text[index + 1..index + 3], 16) {
                    Ok(byte) => {
                        output.push(byte);
                        index += 2;
                    }
                    Err(_) => output.push(b'%'),
                }
            }
            byte => output.push(byte),
        }
        index += 1;
    }
    String::from_utf8_lossy(&output).into_owned()
}

/// The WHATWG urlencoded serializer: alphanumerics and `*-._` verbatim, space
/// as `+`, everything else percent-encoded uppercase.
fn form_encode(text: &str) -> String {
    let mut output = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                output.push(byte as char)
            }
            b' ' => output.push('+'),
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }
    output
}

fn serialize_query(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", form_encode(key), form_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn fixture() -> Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/route-rules-conformance.json");
        serde_json::from_str(
            &std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("read {}: {error}", path.display())),
        )
        .expect("conformance fixture is valid JSON")
    }

    struct OwnedRequest {
        path: String,
        query: Option<String>,
        headers: Vec<(String, String)>,
        host: Option<String>,
    }

    impl OwnedRequest {
        fn from_value(value: &Value) -> Self {
            Self {
                path: value["path"].as_str().expect("request path").to_string(),
                query: value["query"].as_str().map(str::to_string),
                headers: value["headers"]
                    .as_object()
                    .map(|headers| {
                        headers
                            .iter()
                            .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                            .collect()
                    })
                    .unwrap_or_default(),
                host: value["host"].as_str().map(str::to_string),
            }
        }

        fn borrow(&self) -> RuleRequest<'_> {
            RuleRequest {
                path: &self.path,
                query: self.query.as_deref(),
                headers: &self.headers,
                host: self.host.as_deref(),
            }
        }
    }

    fn params_json(params: Option<RuleParams>) -> Value {
        match params {
            None => Value::Null,
            Some(params) => serde_json::to_value(params).unwrap(),
        }
    }

    fn conditions(value: &Value) -> Vec<CompiledCondition> {
        let conditions: Vec<RouteCondition> = match value {
            Value::Null => Vec::new(),
            other => serde_json::from_value(other.clone()).expect("conditions"),
        };
        compile_conditions(&conditions).expect("conditions compile")
    }

    /// Replay the shared table. `tests/packages/core/route-rules.test.ts`
    /// drives the JavaScript module through the same file.
    #[test]
    fn matches_the_shared_cross_language_conformance_table() {
        let fixture = fixture();

        for entry in fixture["sources"].as_array().unwrap() {
            let source = entry["source"].as_str().unwrap();
            let compiled = compile_source(source).unwrap_or_else(|e| panic!("{source}: {e}"));
            for (path, expected) in entry["cases"].as_object().unwrap() {
                assert_eq!(
                    params_json(compiled.matches(path)),
                    *expected,
                    "{source} against {path}"
                );
            }
        }

        for source in fixture["invalidSources"].as_array().unwrap() {
            let source = source.as_str().unwrap();
            let error = compile_source(source)
                .err()
                .unwrap_or_else(|| panic!("{source:?} must be refused"));
            assert!(error.contains("RUV1602"), "{error}");
        }

        for case in fixture["conditions"].as_array().unwrap() {
            let request = OwnedRequest::from_value(&case["request"]);
            let actual = evaluate_conditions(
                &conditions(&case["has"]),
                &conditions(&case["missing"]),
                &request.borrow(),
            );
            assert_eq!(
                params_json(actual),
                case["result"],
                "conditions: {}",
                case["$why"]
            );
        }

        for case in fixture["headers"].as_array().unwrap() {
            let rules: Vec<HeaderRule> = serde_json::from_value(case["rules"].clone()).unwrap();
            let compiled = compile_header_rules(rules).unwrap();
            let request = OwnedRequest::from_value(&case["request"]);
            let actual = apply_header_rules(&compiled, &request.borrow());
            let expected: Vec<(String, String)> =
                serde_json::from_value(case["result"].clone()).unwrap();
            assert_eq!(actual, expected, "headers: {}", case["$why"]);
        }

        for case in fixture["redirects"].as_array().unwrap() {
            let rules: Vec<RedirectRule> = serde_json::from_value(case["rules"].clone()).unwrap();
            let compiled = compile_redirect_rules(rules).unwrap();
            let request = OwnedRequest::from_value(&case["request"]);
            let actual = match_redirect(&compiled, &request.borrow()).map(|decision| {
                serde_json::json!({ "location": decision.location, "status": decision.status })
            });
            assert_eq!(
                actual.unwrap_or(Value::Null),
                case["result"],
                "redirects: {}",
                case["$why"]
            );
        }

        for case in fixture["rewrites"].as_array().unwrap() {
            let rules: Vec<RewriteRule> = serde_json::from_value(case["rules"].clone()).unwrap();
            let compiled = compile_rewrite_rules(rules).unwrap();
            let request = OwnedRequest::from_value(&case["request"]);
            let actual = match_rewrite(&compiled, &request.borrow());
            assert_eq!(
                actual.map(Value::String).unwrap_or(Value::Null),
                case["result"],
                "rewrites: {}",
                case["$why"]
            );
        }

        for case in fixture["matcher"].as_array().unwrap() {
            let matcher = &case["matcher"];
            // The renderer's normalization, restated: a string is `{ source }`.
            let entries: Vec<MatcherEntry> = match matcher {
                Value::String(source) => vec![MatcherEntry {
                    source: source.clone(),
                    has: Vec::new(),
                    missing: Vec::new(),
                }],
                Value::Array(items) => items
                    .iter()
                    .map(|item| match item {
                        Value::String(source) => MatcherEntry {
                            source: source.clone(),
                            has: Vec::new(),
                            missing: Vec::new(),
                        },
                        other => serde_json::from_value(other.clone()).unwrap(),
                    })
                    .collect(),
                other => panic!("unexpected matcher {other}"),
            };
            let compiled = compile_matcher(&entries).unwrap();
            let request = OwnedRequest::from_value(&case["request"]);
            assert_eq!(
                matcher_matches(&compiled, &request.borrow()),
                case["result"].as_bool().unwrap(),
                "matcher {matcher} against {}",
                request.path
            );
        }
    }

    #[test]
    fn a_redirect_without_permanent_or_status_code_is_refused() {
        let error = compile_redirect_rules(vec![RedirectRule {
            source: "/a".into(),
            destination: "/b".into(),
            permanent: None,
            status_code: None,
            has: Vec::new(),
            missing: Vec::new(),
        }])
        .err()
        .unwrap();
        assert!(error.contains("permanent or statusCode"), "{error}");
    }
}
