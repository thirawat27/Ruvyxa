//! HTML document assembly: head/HMR injection, client hydration scripts,
//! and the dev error overlay / production error pages.

use std::fs;
use std::path::{Path, PathBuf};

use axum::http::StatusCode;
use axum::response::Response;
use ruvyxa_diagnostics::{Diagnostic, RuvyxaError};
use ruvyxa_graph::{RouteEntry, RouteParams};
use serde::Deserialize;

use crate::{ServerConfig, html_response};

pub(crate) fn compose_document(rendered: &str, head_content: &str, hmr: &str) -> String {
    if contains_ascii_case(rendered, "<html") {
        let with_head = if contains_ascii_case(rendered, "<head") {
            insert_before_ascii_case(rendered, "</head>", head_content)
        } else if let Some(body_index) = find_ascii_case(rendered, "<body") {
            let mut document = String::with_capacity(rendered.len() + head_content.len() + 32);
            document.push_str(&rendered[..body_index]);
            document.push_str("<head>");
            document.push_str(head_content);
            document.push_str("</head>");
            document.push_str(&rendered[body_index..]);
            document
        } else {
            insert_after_opening_html(rendered, head_content)
        };

        return insert_before_ascii_case(&with_head, "</body>", hmr);
    }

    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">{head_content}</head><body>{rendered}{hmr}</body></html>"
    )
}

pub(crate) fn insert_after_opening_html(rendered: &str, head_content: &str) -> String {
    let Some(html_index) = find_ascii_case(rendered, "<html") else {
        return rendered.to_string();
    };
    let Some(close_index) = rendered[html_index..].find('>') else {
        return rendered.to_string();
    };
    let insert_index = html_index + close_index + 1;
    let mut document = String::with_capacity(rendered.len() + head_content.len() + 16);
    document.push_str(&rendered[..insert_index]);
    document.push_str("<head>");
    document.push_str(head_content);
    document.push_str("</head>");
    document.push_str(&rendered[insert_index..]);
    document
}

pub(crate) fn insert_before_ascii_case(input: &str, needle: &str, insertion: &str) -> String {
    let Some(index) = find_ascii_case(input, needle) else {
        let mut output = input.to_string();
        output.push_str(insertion);
        return output;
    };

    let mut output = String::with_capacity(input.len() + insertion.len());
    output.push_str(&input[..index]);
    output.push_str(insertion);
    output.push_str(&input[index..]);
    output
}

pub(crate) fn contains_ascii_case(input: &str, needle: &str) -> bool {
    find_ascii_case(input, needle).is_some()
}

pub(crate) fn find_ascii_case(input: &str, needle: &str) -> Option<usize> {
    input
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

#[derive(Debug, Deserialize)]
struct ClientAssetManifest {
    routes: Vec<ClientAssetRoute>,
}

#[derive(Debug, Deserialize)]
struct ClientAssetRoute {
    path: String,
    src: String,
    #[serde(rename = "sharedChunks")]
    shared_chunks: Vec<ClientSharedChunk>,
}

#[derive(Debug, Deserialize)]
struct ClientSharedChunk {
    src: String,
}

pub(crate) struct ClientAssets {
    pub(crate) src: String,
    pub(crate) preloads: Vec<String>,
}

pub(crate) fn client_hydration_script(
    config: &ServerConfig,
    route: &RouteEntry,
    request_path: &str,
    params: &RouteParams,
) -> String {
    let params_json = serde_json::to_string(params).unwrap_or_else(|_| "{}".to_string());
    let params_json = safe_json_for_script(&params_json);
    let request_path_json = safe_json_for_script(
        &serde_json::to_string(request_path).unwrap_or_else(|_| "\"/\"".to_string()),
    );
    let assets = if config.watch {
        ClientAssets {
            src: format!(
                "/__ruvyxa/client?path={}",
                url_encode_component(request_path)
            ),
            preloads: Vec::new(),
        }
    } else {
        prebuilt_client_assets(config, &route.path).unwrap_or_else(|| ClientAssets {
            src: format!(
                "/__ruvyxa/client?path={}",
                url_encode_component(request_path)
            ),
            preloads: Vec::new(),
        })
    };
    let preload_links = assets
        .preloads
        .iter()
        .map(|src| {
            let src = escape_html(src);
            format!(r#"<link rel="modulepreload" href="{src}">"#)
        })
        .collect::<String>();
    let src = escape_html(&assets.src);

    format!(
        r#"{preload_links}<script>globalThis.__RUVYXA_ROUTE_PARAMS__ = {params_json};globalThis.__RUVYXA_REQUEST_PATH__ = {request_path_json};</script><script type="module" src="{src}"></script>"#,
    )
}

pub(crate) fn prebuilt_client_assets(
    config: &ServerConfig,
    route_path: &str,
) -> Option<ClientAssets> {
    let source = fs::read_to_string(config.client_dir.join("manifest.json")).ok()?;
    let manifest: ClientAssetManifest = serde_json::from_str(&source).ok()?;
    manifest
        .routes
        .into_iter()
        .find(|route| route.path == route_path)
        .map(|route| ClientAssets {
            src: route.src,
            preloads: route
                .shared_chunks
                .into_iter()
                .map(|chunk| chunk.src)
                .collect(),
        })
}

pub(crate) fn safe_json_for_script(json: &str) -> String {
    json.replace("</", "<\\/")
}

pub(crate) fn hmr_client_script() -> &'static str {
    r#"<script>
(() => {
  const protocol = location.protocol === "https:" ? "wss" : "ws";
  const socket = new WebSocket(`${protocol}://${location.host}/__ruvyxa/hmr`);
  socket.addEventListener("message", (event) => {
    // A clean page load keeps the browser's ESM module graph and React root in sync.
    // This also covers route, CSS, and imported-module changes consistently.
    JSON.parse(event.data);
    location.reload();
  });
})();
</script>"#
}

pub(crate) fn url_encode_component(input: &str) -> String {
    let mut output = String::new();

    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                output.push(byte as char)
            }
            _ => output.push_str(&format!("%{byte:02X}")),
        }
    }

    output
}

pub(crate) fn extract_code_frame(file: &Path, line: Option<u32>) -> Option<String> {
    let line = line?;
    let source = fs::read_to_string(file).ok()?;
    let lines: Vec<&str> = source.lines().collect();
    let total = lines.len();
    let idx = line.saturating_sub(1) as usize;
    if idx >= total {
        return None;
    }
    let start = idx.saturating_sub(2);
    let end = (idx + 3).min(total);
    let mut frame = String::new();
    let max_digits = end.to_string().len().max(2);
    for (i, line_text) in lines[start..end].iter().enumerate() {
        let i = start + i;
        let num = i + 1;
        let prefix = if i == idx { ">" } else { " " };
        let marker = if i == idx { "  ← error" } else { "" };
        frame.push_str(&format!(
            " {prefix} {:>width$} │ {}{}\n",
            num,
            line_text,
            marker,
            width = max_digits
        ));
    }
    Some(frame)
}

pub(crate) fn error_response(
    status: StatusCode,
    diagnostics: &Diagnostic,
    is_dev: bool,
) -> Response {
    if !is_dev {
        return html_response(status, plain_error_page("Internal server error"));
    }
    let code_frame = diagnostics
        .span
        .as_ref()
        .and_then(|span| extract_code_frame(&span.file, span.line));
    let body = dev_diagnostic_overlay(diagnostics, code_frame.as_deref());
    html_response(status, body)
}

pub(crate) fn public_internal_error(config: &ServerConfig, error: &RuvyxaError) -> String {
    if config.watch {
        error.to_string()
    } else {
        "Internal server error".to_string()
    }
}

pub(crate) fn error_page(message: &str, show_overlay: bool) -> String {
    if show_overlay {
        dev_error_overlay(message, None, None, None)
    } else {
        plain_error_page(message)
    }
}

pub(crate) fn plain_error_page(message: &str) -> String {
    let not_found = message.contains("Route not found");
    let code = if not_found { "404" } else { "500" };
    let title = if not_found {
        "This page could not be found."
    } else {
        "Ruvyxa hit an unexpected error."
    };

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="robots" content="noindex">
<title>Ruvyxa Error - {code}</title>
<style>
  :root {{ color-scheme: light; --bg: #18181c; --ink: #4c1d95; --muted: #6d4b8f; --accent: #7c3aed; --line: rgba(124,58,237,.28); }}
  *, *::before, *::after {{ box-sizing: border-box; }}
  html, body {{ min-height: 100%; }}
  body {{ display: grid; min-height: 100vh; place-items: center; margin: 0; padding: 28px; color: var(--ink); background: radial-gradient(circle at 50% 38%, rgba(111, 65, 143, .18), transparent 34rem), var(--bg); font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
  .error-card {{ width: min(760px, 100%); padding: clamp(30px, 6vw, 66px); border: 1px solid rgba(124,58,237,.16); border-radius: 24px; background: #fff; box-shadow: 0 28px 90px rgba(0,0,0,.38), 0 0 0 1px rgba(255,255,255,.7) inset; text-align: center; }}
  .logo {{ display: block; width: clamp(82px, 15vw, 132px); height: clamp(82px, 15vw, 132px); margin: 0 auto 28px; object-fit: contain; filter: drop-shadow(0 12px 22px rgba(123, 62, 226, .3)); }}
  .status {{ display: inline-flex; align-items: center; justify-content: center; gap: clamp(14px, 3vw, 34px); margin: 0 auto 18px; }}
  .code {{ color: var(--accent); font: 800 clamp(36px, 7vw, 58px)/1 ui-monospace, SFMono-Regular, Consolas, monospace; letter-spacing: -.06em; }}
  .divider {{ width: 1px; height: 62px; background: var(--line); }}
  h1 {{ margin: 0; color: var(--ink); font-size: clamp(22px, 4vw, 34px); font-weight: 520; letter-spacing: -.035em; }}
  .message {{ max-width: 620px; margin: 18px auto 0; color: var(--muted); font: 15px/1.7 ui-monospace, SFMono-Regular, Consolas, monospace; white-space: pre-wrap; overflow-wrap: anywhere; }}
  .path-label {{ display: inline-block; margin-top: 20px; padding: 6px 12px; border: 1px solid rgba(124,58,237,.2); border-radius: 999px; color: #6d28d9; background: #f4efff; font-size: clamp(13px, 1.8vw, 16px); font-weight: 700; letter-spacing: .06em; text-transform: uppercase; text-shadow: 0 1px 0 rgba(255,255,255,.8); }}
  @media (max-width: 560px) {{ body {{ padding: 16px; }} .error-card {{ padding: 34px 22px; border-radius: 18px; }} .status {{ flex-direction: column; gap: 12px; }} .code {{ font-size: clamp(42px, 14vw, 54px); }} .divider {{ width: 64px; height: 1px; }} h1 {{ max-width: 260px; text-align: center; }} }}
</style>
</head>
<body>
<main class="error-card" aria-labelledby="error-title">
  <img class="logo" src="/ruvyxa.png" alt="Ruvyxa">
  <div class="status" aria-label="Error status">
    <span class="code">{code}</span>
    <span class="divider" aria-hidden="true"></span>
    <h1 id="error-title">{title}</h1>
  </div>
  <pre class="message">{}</pre>
  <div class="path-label">Ruvyxa Error</div>
</main>
</body>
</html>"##,
        escape_html(message)
    )
}

pub(crate) fn dev_error_overlay(
    message: &str,
    code_frame: Option<&str>,
    stack: Option<&str>,
    suggestion: Option<&str>,
) -> String {
    let mut lines = message.lines();
    let title = lines.next().unwrap_or("Unhandled Runtime Error");
    let detail = lines.collect::<Vec<_>>().join("\n");
    render_error_overlay(ErrorOverlayView {
        code: "RUV_RUNTIME",
        title,
        detail: if detail.trim().is_empty() {
            message
        } else {
            &detail
        },
        location: None,
        code_frame,
        stack,
        suggestion,
        import_chain: &[],
        affected_routes: &[],
    })
}

pub(crate) fn dev_diagnostic_overlay(diagnostic: &Diagnostic, code_frame: Option<&str>) -> String {
    let location = diagnostic
        .span
        .as_ref()
        .map(|span| match (span.line, span.column) {
            (Some(line), Some(column)) => format!("{}:{line}:{column}", span.file.display()),
            (Some(line), None) => format!("{}:{line}", span.file.display()),
            _ => span.file.display().to_string(),
        });
    render_error_overlay(ErrorOverlayView {
        code: diagnostic.code,
        title: &diagnostic.title,
        detail: &diagnostic.explanation,
        location: location.as_deref(),
        code_frame,
        stack: None,
        suggestion: diagnostic.suggested_fix.as_deref(),
        import_chain: &diagnostic.import_chain,
        affected_routes: &diagnostic.affected_routes,
    })
}

pub(crate) struct ErrorOverlayView<'a> {
    code: &'a str,
    title: &'a str,
    detail: &'a str,
    location: Option<&'a str>,
    code_frame: Option<&'a str>,
    stack: Option<&'a str>,
    suggestion: Option<&'a str>,
    import_chain: &'a [PathBuf],
    affected_routes: &'a [String],
}

pub(crate) fn render_error_overlay(view: ErrorOverlayView<'_>) -> String {
    let ErrorOverlayView {
        code,
        title,
        detail,
        location,
        code_frame,
        stack,
        suggestion,
        import_chain,
        affected_routes,
    } = view;
    let frame_html = code_frame
        .map(|f| {
            format!(
                r#"<section class="source"><div class="source-head"><span>Source</span><code>{}</code></div><pre>{}</pre></section>"#,
                escape_html(location.unwrap_or("source unavailable")),
                escape_html(f)
            )
        })
        .unwrap_or_default();
    let stack_html = stack
        .map(|s| {
            format!(
                r#"<details><summary>Stack trace</summary><pre>{}</pre></details>"#,
                escape_html(s)
            )
        })
        .unwrap_or_default();
    let suggestion_html = suggestion
        .map(|s| {
            format!(
                r#"<section class="hint"><strong>Suggested fix</strong><p>{}</p></section>"#,
                escape_html(s)
            )
        })
        .unwrap_or_default();
    let location_html = location
        .map(|location| format!(r#"<div class="location">{}</div>"#, escape_html(location)))
        .unwrap_or_default();
    let import_chain_html = if import_chain.is_empty() {
        String::new()
    } else {
        format!(
            r#"<details open><summary>Import chain ({})</summary><ol>{}</ol></details>"#,
            import_chain.len(),
            import_chain
                .iter()
                .map(|path| format!(
                    "<li><code>{}</code></li>",
                    escape_html(&path.display().to_string())
                ))
                .collect::<String>()
        )
    };
    let routes_html = if affected_routes.is_empty() {
        String::new()
    } else {
        format!(
            r#"<details open><summary>Affected routes ({})</summary><ul>{}</ul></details>"#,
            affected_routes.len(),
            affected_routes
                .iter()
                .map(|route| format!("<li><code>{}</code></li>", escape_html(route)))
                .collect::<String>()
        )
    };

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Ruvyxa Error - {title}</title>
<style>
  *, *::before, *::after {{ box-sizing: border-box; }}
  :root {{ color-scheme: light; font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; }}
  body {{
    margin: 0;
    min-height: 100vh;
    color: #171717;
    background: linear-gradient(135deg, #f1f1f1, #d9d9d9);
  }}
  .backdrop {{
    min-height: 100vh;
    padding: clamp(16px, 5vw, 64px);
    background: rgba(245, 245, 245, .76);
    backdrop-filter: blur(9px);
  }}
  .dialog {{
    width: min(920px, 100%);
    margin: 0 auto;
    background: #fff;
    border: 1px solid #d7d7d7;
    border-top: 3px solid #ef5b5b;
    border-radius: 8px;
    box-shadow: 0 24px 64px rgba(0, 0, 0, .2);
    overflow: hidden;
  }}
  .toolbar {{
    display: flex;
    align-items: center;
    justify-content: space-between;
    min-height: 46px;
    padding: 0 14px;
    border-bottom: 1px solid #ececec;
    color: #6b6b6b;
    font-size: 12px;
  }}
  .toolbar button {{ border: 0; background: transparent; color: #707070; font-size: 22px; cursor: pointer; padding: 4px 8px; }}
  .content {{ padding: clamp(20px, 4vw, 40px); }}
  .eyebrow {{ color: #d53535; font: 700 12px/1.4 ui-monospace, SFMono-Regular, Consolas, monospace; letter-spacing: .06em; }}
  h1 {{ margin: 8px 0 6px; font-size: clamp(20px, 3vw, 28px); line-height: 1.25; }}
  .location {{ color: #b4232d; font: 500 13px/1.5 ui-monospace, SFMono-Regular, Consolas, monospace; overflow-wrap: anywhere; }}
  .detail {{ margin: 18px 0 24px; color: #424242; white-space: pre-wrap; overflow-wrap: anywhere; }}
  .source {{ margin: 20px 0; border: 1px solid #222; border-radius: 6px; overflow: hidden; background: #101010; color: #f5f5f5; }}
  .source-head {{ display: flex; justify-content: space-between; gap: 16px; padding: 8px 12px; border-bottom: 1px solid #333; color: #d7d7d7; font-size: 12px; }}
  .source-head code {{ color: #a8a8a8; overflow-wrap: anywhere; text-align: right; }}
  .source pre {{ margin: 0; padding: 16px; overflow: auto; color: #f3f3f3; font: 13px/1.6 ui-monospace, SFMono-Regular, Consolas, monospace; tab-size: 2; }}
  .hint {{ margin: 18px 0; padding: 14px 16px; border: 1px solid #9dd5ab; border-left: 4px solid #2f9e44; border-radius: 6px; background: #f3fbf5; }}
  .hint strong {{ color: #176b2c; }}
  .hint p {{ margin: 5px 0 0; color: #285b35; white-space: pre-wrap; }}
  details {{ margin-top: 12px; border: 1px solid #e2e2e2; border-radius: 6px; padding: 10px 12px; }}
  summary {{ cursor: pointer; font-weight: 650; }}
  details pre {{ overflow: auto; white-space: pre-wrap; color: #454545; font: 12px/1.55 ui-monospace, SFMono-Regular, Consolas, monospace; }}
  details ol, details ul {{ margin-bottom: 0; padding-left: 24px; }}
  details li {{ margin: 5px 0; overflow-wrap: anywhere; }}
  .footer {{ padding: 12px 20px; border-top: 1px solid #ececec; background: #fafafa; color: #777; font-size: 12px; text-align: center; }}
  @media (max-width: 600px) {{
    .backdrop {{ padding: 0; }}
    .dialog {{ min-height: 100vh; border-radius: 0; border-left: 0; border-right: 0; }}
    .source-head {{ flex-direction: column; }}
    .source-head code {{ text-align: left; }}
  }}
</style>
</head>
<body>
<main class="backdrop">
  <section class="dialog" id="ruvyxa-error-overlay" role="dialog" aria-modal="true" aria-labelledby="ruvyxa-error-title">
    <div class="toolbar"><span>‹ &nbsp; 1 of 1 unhandled error &nbsp; ›</span><button type="button" aria-label="Close error overlay" onclick="document.getElementById('ruvyxa-error-overlay').hidden=true">×</button></div>
    <div class="content">
      <div class="eyebrow">{code}</div>
      <h1 id="ruvyxa-error-title">{title}</h1>
      {location_html}
      <div class="detail">{detail}</div>
      {frame_html}
      {suggestion_html}
      {import_chain_html}
      {routes_html}
      {stack_html}
    </div>
    <div class="footer">Ruvyxa Dev Server — fix the error and save to hot-reload</div>
  </section>
</main>
</body>
</html>"#,
        code = escape_html(code),
        title = escape_html(title),
        detail = escape_html(detail),
    )
}

pub(crate) fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
