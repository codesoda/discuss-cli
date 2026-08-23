//! HTML prototype document and relative-asset serving.

use std::path::{Component, Path as FsPath, PathBuf};

use axum::body::Body;
use axum::extract::{Path, State as AxumState};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use crate::state::{FileId, FileKind};

use super::app_state::AppState;
use super::response::api_error_response;

pub(super) async fn get_html_file(
    AxumState(app_state): AxumState<AppState>,
    Path(file_id): Path<String>,
) -> Response {
    let file_id = FileId(file_id);
    let Some(file) = app_state.file(&file_id) else {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "unknown_file",
            format!("HTML file not found: {}", file_id.0),
        );
    };
    if file.kind != FileKind::Html {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "unknown_file",
            format!("HTML file not found: {}", file_id.0),
        );
    }

    let html = prepare_html(&file.content, &prototype_base_href(&file_id.0, None));
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        html,
    )
        .into_response()
}

pub(super) async fn get_html_asset(
    AxumState(app_state): AxumState<AppState>,
    Path((file_id, asset_path)): Path<(String, String)>,
) -> Response {
    let file_id = FileId(file_id);
    let Some(file) = app_state.file(&file_id) else {
        return asset_not_found();
    };
    if file.kind != FileKind::Html {
        return asset_not_found();
    }

    let requested = FsPath::new(&asset_path);
    if requested.as_os_str().is_empty()
        || requested
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return asset_not_found();
    }

    let Some(parent) = FsPath::new(&file.path).parent() else {
        return asset_not_found();
    };
    let Ok(root) = parent.canonicalize() else {
        return asset_not_found();
    };
    let candidate: PathBuf = root.join(requested);
    let Ok(candidate) = candidate.canonicalize() else {
        return asset_not_found();
    };
    if !candidate.starts_with(&root) || !candidate.is_file() {
        return asset_not_found();
    }

    let Ok(bytes) = std::fs::read(&candidate) else {
        return asset_not_found();
    };
    let mime = mime_for_path(&candidate);
    let body = if mime.starts_with("text/html") {
        let Ok(source) = String::from_utf8(bytes) else {
            return asset_not_found();
        };
        Body::from(prepare_html(
            &source,
            &prototype_base_href(&file_id.0, requested.parent()),
        ))
    } else {
        Body::from(bytes)
    };
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, "no-store")
        .body(body)
        .unwrap_or_else(|_| asset_not_found())
}

fn asset_not_found() -> Response {
    api_error_response(
        StatusCode::NOT_FOUND,
        "not_found",
        "prototype asset not found",
    )
}

fn mime_for_path(path: &FsPath) -> &'static str {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "application/javascript",
        Some("json" | "map") => "application/json",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("html" | "htm") => "text/html; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

fn prepare_html(source: &str, base_href: &str) -> String {
    let mut html = strip_csp_meta(source);
    let base = format!("<base href=\"{base_href}\">");
    let lower = html.to_ascii_lowercase();
    if let Some(head_start) = lower.find("<head")
        && let Some(relative_end) = lower[head_start..].find('>')
    {
        html.insert_str(head_start + relative_end + 1, &base);
    } else {
        html.insert_str(0, &base);
    }

    let script = "<script src=\"/assets/discuss-inspect.js?v=4\"></script>";
    let lower = html.to_ascii_lowercase();
    if let Some(body_end) = lower.rfind("</body>") {
        html.insert_str(body_end, script);
    } else {
        html.push_str(script);
    }
    html
}

fn prototype_base_href(file_id: &str, directory: Option<&FsPath>) -> String {
    let mut href = format!("/files/{}/assets/", percent_encode_segment(file_id));
    if let Some(directory) = directory {
        for component in directory.components() {
            if let Component::Normal(segment) = component {
                href.push_str(&percent_encode_segment(&segment.to_string_lossy()));
                href.push('/');
            }
        }
    }
    href
}

fn percent_encode_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn strip_csp_meta(source: &str) -> String {
    let lower = source.to_ascii_lowercase();
    let mut output = String::with_capacity(source.len());
    let mut cursor = 0;
    while let Some(relative_start) = lower[cursor..].find("<meta") {
        let start = cursor + relative_start;
        let Some(relative_end) = lower[start..].find('>') else {
            break;
        };
        let end = start + relative_end + 1;
        let tag = &lower[start..end];
        if tag.contains("http-equiv") && tag.contains("content-security-policy") {
            output.push_str(&source[cursor..start]);
            cursor = end;
        } else {
            output.push_str(&source[cursor..end]);
            cursor = end;
        }
    }
    output.push_str(&source[cursor..]);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn injection_adds_base_and_inspector_before_closing_tags() {
        let html = prepare_html(
            "<html><head><title>x</title></head><body>ok</body></html>",
            "/files/f-2/assets/",
        );
        assert!(html.contains("<head><base href=\"/files/f-2/assets/\">"));
        assert!(html.contains("<script src=\"/assets/discuss-inspect.js?v=4\"></script></body>"));
    }

    #[test]
    fn injection_neutralizes_csp_meta() {
        let html = prepare_html(
            "<head><meta http-equiv=\"Content-Security-Policy\" content=\"script-src 'none'\"><meta name=\"viewport\"></head>",
            "/files/f-1/assets/",
        );
        assert!(
            !html
                .to_ascii_lowercase()
                .contains("content-security-policy")
        );
        assert!(html.contains("name=\"viewport\""));
    }
}
