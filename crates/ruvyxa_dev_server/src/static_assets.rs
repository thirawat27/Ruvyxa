//! Public-directory and client-bundle static file serving: path safety,
//! image format fallback, ETag/conditional responses, and content types.

use std::fs;
use std::path::{Path, PathBuf};

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use ruvyxa_diagnostics::{Result, RuvyxaError};

use crate::apply_security_headers;

pub(crate) async fn serve_public_file(
    public_dir: &Path,
    request_path: &str,
    request_headers: Option<&HeaderMap>,
) -> Result<Option<Response>> {
    let trimmed = request_path.trim_start_matches('/');
    if !is_safe_relative_path(trimmed) {
        return Ok(None);
    }

    let Some((file, vary_accept)) = resolve_public_asset(public_dir, trimmed, request_headers)
    else {
        return Ok(None);
    };
    let metadata = match tokio::fs::metadata(&file).await {
        Ok(meta) if meta.is_file() => meta,
        _ => return Ok(None),
    };

    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|source| RuvyxaError::Io {
            message: format!("Failed to read public file {}", file.display()),
            source,
        })?;

    // Compute ETag using blake3 hash
    let etag = compute_etag(&bytes);

    // Check If-None-Match for conditional response
    if let Some(headers) = request_headers
        && let Some(if_none_match) = headers.get(header::IF_NONE_MATCH)
        && etag_matches(if_none_match, &etag)
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        if vary_accept {
            response
                .headers_mut()
                .insert(header::VARY, HeaderValue::from_static("Accept"));
        }
        apply_security_headers(&mut response);
        return Ok(Some(response));
    }

    let content_type = content_type_for(&file);
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600, must-revalidate"),
    );
    if vary_accept {
        headers.insert(header::VARY, HeaderValue::from_static("Accept"));
    }

    let _ = metadata; // used for existence check
    apply_security_headers(&mut response);
    Ok(Some(response))
}

/// Sync fallback for static file serving (used by render_request test/bench path).
pub(crate) fn serve_public_file_sync(
    public_dir: &Path,
    request_path: &str,
) -> Result<Option<Response>> {
    let trimmed = request_path.trim_start_matches('/');
    if !is_safe_relative_path(trimmed) {
        return Ok(None);
    }
    let Some((file, _)) = resolve_public_asset(public_dir, trimmed, None) else {
        return Ok(None);
    };
    let bytes = fs::read(&file)?;
    let content_type = content_type_for(&file);
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    apply_security_headers(&mut response);
    Ok(Some(response))
}

/// Sync fallback for client file serving (used by render_request test/bench path).
pub(crate) fn serve_client_file_sync(
    client_dir: &Path,
    request_path: &str,
) -> Result<Option<Response>> {
    let Some(file_name) = request_path.strip_prefix("/__ruvyxa/client/") else {
        return Ok(None);
    };
    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Ok(None);
    }
    let Some(file) = contained_public_asset(client_dir, &client_dir.join(file_name)) else {
        return Ok(None);
    };
    if !file.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&file)?;
    let mut response = bytes.into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    apply_security_headers(&mut response);
    Ok(Some(response))
}

pub(crate) async fn serve_client_file(
    client_dir: &Path,
    request_path: &str,
    request_headers: Option<&HeaderMap>,
) -> Result<Option<Response>> {
    let Some(file_name) = request_path.strip_prefix("/__ruvyxa/client/") else {
        return Ok(None);
    };

    if file_name.is_empty()
        || file_name.contains('/')
        || file_name.contains('\\')
        || file_name.contains("..")
    {
        return Ok(None);
    }

    let Some(file) = contained_public_asset(client_dir, &client_dir.join(file_name)) else {
        return Ok(None);
    };
    match tokio::fs::metadata(&file).await {
        Ok(meta) if meta.is_file() => {}
        _ => return Ok(None),
    }

    let bytes = tokio::fs::read(&file)
        .await
        .map_err(|source| RuvyxaError::Io {
            message: format!("Failed to read client file {}", file.display()),
            source,
        })?;

    // Client bundles are content-hashed, so use immutable caching with ETag
    let etag = compute_etag(&bytes);

    if let Some(headers) = request_headers
        && let Some(if_none_match) = headers.get(header::IF_NONE_MATCH)
        && etag_matches(if_none_match, &etag)
    {
        let mut response = StatusCode::NOT_MODIFIED.into_response();
        apply_security_headers(&mut response);
        return Ok(Some(response));
    }

    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/javascript; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).unwrap_or_else(|_| HeaderValue::from_static("")),
    );
    apply_security_headers(&mut response);
    Ok(Some(response))
}

pub(crate) fn resolve_public_asset(
    public_dir: &Path,
    request_path: &str,
    _request_headers: Option<&HeaderMap>,
) -> Option<(PathBuf, bool)> {
    let requested = public_dir.join(request_path);
    if requested.is_file() {
        return contained_public_asset(public_dir, &requested).map(|file| (file, false));
    }

    // Development keeps source images untouched while the React component
    // points at the production `.webp` URL. Resolve that URL to exactly one
    // source format; ambiguity matches the build-time collision guard.
    if requested.extension().and_then(|value| value.to_str()) == Some("webp") {
        let mut candidates = ["png", "jpg", "jpeg", "PNG", "JPG", "JPEG"]
            .map(|extension| requested.with_extension(extension))
            .into_iter()
            .filter_map(|path| {
                path.is_file()
                    .then(|| contained_public_asset(public_dir, &path))
                    .flatten()
            })
            .collect::<Vec<_>>();
        candidates.sort();
        candidates.dedup();
        if candidates.len() == 1 {
            return Some((candidates.into_iter().next()?, false));
        }
    }

    // Keep server deployments compatible with plain `<img src="hero.png">`
    // while the build output stores only `hero.webp`.
    if is_convertible_image_url(&requested) {
        let webp = requested.with_extension("webp");
        if webp.is_file() {
            return contained_public_asset(public_dir, &webp).map(|file| (file, false));
        }
    }
    None
}

/// Canonicalize asset paths before serving them so public-directory symlinks
/// cannot expose files outside the configured root.
pub(crate) fn contained_public_asset(public_dir: &Path, candidate: &Path) -> Option<PathBuf> {
    if !public_dir.exists() || !candidate.exists() {
        return None;
    }
    let public_root = ruvyxa_diagnostics::normalized_canonical_path(public_dir);
    let candidate = ruvyxa_diagnostics::normalized_canonical_path(candidate);
    candidate.starts_with(&public_root).then_some(candidate)
}

pub(crate) fn is_convertible_image_url(path: &Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("png" | "jpg" | "jpeg")
    )
}

pub(crate) fn is_safe_relative_path(path: &str) -> bool {
    if path.is_empty() || path.contains('\\') {
        return false;
    }

    Path::new(path).components().all(|component| {
        matches!(
            component,
            std::path::Component::Normal(_) | std::path::Component::CurDir
        )
    })
}

/// Compute a strong ETag using blake3 hash of file content.
pub(crate) fn compute_etag(bytes: &[u8]) -> String {
    let hash = blake3::hash(bytes);
    format!("\"{}\"", &hash.to_hex()[..16])
}

pub(crate) fn etag_matches(value: &HeaderValue, etag: &str) -> bool {
    let Ok(value) = value.to_str() else {
        return false;
    };
    let target = etag.trim_matches('"');
    value.split(',').any(|candidate| {
        let candidate = candidate.trim();
        if candidate == "*" {
            return true;
        }
        candidate
            .strip_prefix("W/")
            .unwrap_or(candidate)
            .trim_matches('"')
            == target
    })
}

pub(crate) fn content_type_for(path: &Path) -> &'static str {
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("webmanifest") => "application/manifest+json; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        _ => "application/octet-stream",
    }
}

pub(crate) fn public_asset_links(public_dir: &Path) -> String {
    let mut links = Vec::new();

    if public_dir.join("ruvyxa.png").exists() {
        links.push(r#"<link rel="icon" type="image/png" href="/ruvyxa.png">"#.to_string());
    }

    links.join("")
}
