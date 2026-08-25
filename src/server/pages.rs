//! Page rendering plus the read-only state, heartbeat, SSE, and asset routes.

use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State as AxumState};
use axum::http::StatusCode;
use axum::http::header;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use std::sync::OnceLock;
use tokio::sync::broadcast;

use crate::assets;
use crate::state::{File, FileId, FileKind};
use crate::update::{self, VersionStatus};
use crate::{render, template};

use super::app_state::AppState;
use super::response::{OkResponse, api_error_response, javascript_response};
use super::{ASSET_CACHE_CONTROL, SSE_HEARTBEAT_INTERVAL};

pub(super) async fn get_root(AxumState(app_state): AxumState<AppState>) -> Response {
    match render_root_page(&app_state) {
        Ok(page) => (
            StatusCode::OK,
            [
                (header::CONTENT_TYPE, "text/html; charset=utf-8"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            page,
        )
            .into_response(),
        Err(message) => (StatusCode::INTERNAL_SERVER_ERROR, message).into_response(),
    }
}

fn render_root_page(app_state: &AppState) -> std::result::Result<String, String> {
    let snapshot = app_state.snapshot_with_files()?;
    let initial_state_json = serde_json::to_string(&snapshot)
        .map_err(|error| format!("failed to serialize initial state: {error}"))?;
    let source = app_state.current_source()?;

    // Every file is pre-rendered and seeded into the page so switching files
    // in the sidebar is a client-side swap with no extra round trip.
    let rendered_files: Vec<RenderedFile> = source
        .files
        .iter()
        .map(|file| RenderedFile {
            id: file.id.clone(),
            html: render_file_html_with_version(file, app_state.raw_file_version(&file.id)),
        })
        .collect();
    let rendered_files_json = serde_json::to_string(&rendered_files)
        .map_err(|error| format!("failed to serialize rendered files: {error}"))?;

    let first_file_html = rendered_files
        .first()
        .map(|file| file.html.clone())
        .unwrap_or_default();

    Ok(template::render_page(
        &first_file_html,
        &initial_state_json,
        &rendered_files_json,
    ))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RenderedFile {
    id: FileId,
    html: String,
}

/// Renders one source file to HTML: markdown files through the markdown
/// renderer directly, diff files through a synthesized markdown document,
/// and images through the stable raw-file route with a pin overlay.
pub(super) fn render_file_html(file: &File) -> String {
    render_file_html_with_version(file, None)
}

fn render_file_html_with_version(file: &File, raw_version: Option<&str>) -> String {
    match file.kind {
        FileKind::Markdown => render::render(&file.content),
        FileKind::Diff => render::render(&crate::diff::diff_content_to_markdown(
            &file.path,
            &file.content,
        )),
        FileKind::Image => {
            let file_id = escape_html_attribute(&file.id.0);
            let alt = escape_html_attribute(&file.path);
            let version = raw_version
                .map(|version| format!("?v={version}"))
                .unwrap_or_default();
            format!(
                "<div class=\"image-review\" data-file-id=\"{file_id}\"><img src=\"/api/files/{file_id}/raw{version}\" alt=\"{alt}\"><div class=\"pin-layer\"></div></div>"
            )
        }
        FileKind::Html => {
            let file_id = escape_html_attribute(&file.id.0);
            let title = escape_html_attribute(&file.path);
            format!(
                "<div class=\"html-review\" data-file-id=\"{file_id}\"><iframe class=\"prototype-frame\" src=\"/files/{file_id}\" title=\"HTML prototype: {title}\" sandbox=\"allow-scripts allow-same-origin\"></iframe></div>"
            )
        }
    }
}

fn escape_html_attribute(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for character in input.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

pub(super) async fn get_api_file_raw(
    AxumState(app_state): AxumState<AppState>,
    Path(file_id): Path<String>,
) -> Response {
    let file_id = FileId(file_id);
    let Some((bytes, mime)) = app_state.raw_file(&file_id) else {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "unknown_file",
            format!("image file not found: {}", file_id.0),
        );
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, *mime)
        .header(header::CACHE_CONTROL, ASSET_CACHE_CONTROL)
        .header("content-security-policy", "sandbox")
        .header("x-content-type-options", "nosniff")
        .body(Body::from(bytes.clone()))
        .expect("valid raw-file response")
}

pub(super) async fn get_api_state(AxumState(app_state): AxumState<AppState>) -> Response {
    match app_state.snapshot_with_files() {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(message) => {
            api_error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
        }
    }
}

/// Successful release lookups are cached for the life of the process so
/// page reloads do not re-hit GitHub; failed lookups stay uncached and
/// retry on the next request.
static VERSION_STATUS_CACHE: OnceLock<VersionStatus> = OnceLock::new();

pub(super) async fn get_api_version() -> Response {
    if let Some(status) = VERSION_STATUS_CACHE.get() {
        return Json(status.clone()).into_response();
    }

    let status = tokio::task::spawn_blocking(update::version_status)
        .await
        .unwrap_or_else(|_| VersionStatus::current_only());
    if status.latest.is_some() {
        let _ = VERSION_STATUS_CACHE.set(status.clone());
    }

    Json(status).into_response()
}

pub(super) async fn post_api_heartbeat(AxumState(app_state): AxumState<AppState>) -> Response {
    match app_state.record_heartbeat() {
        Ok(_) => Json(OkResponse { ok: true }).into_response(),
        Err(message) => {
            api_error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
        }
    }
}

pub(super) async fn get_api_events(AxumState(app_state): AxumState<AppState>) -> impl IntoResponse {
    let mut events = app_state.bus.subscribe();
    let mut shutdown = app_state.subscribe_shutdown();
    let stream = async_stream::stream! {
        loop {
            tokio::select! {
                biased;

                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                event = events.recv() => {
                    match event {
                        Ok(event) => {
                            let Ok(payload) = serde_json::to_string(&event.payload) else {
                                continue;
                            };
                            yield Ok::<_, std::convert::Infallible>(
                                SseEvent::default().event(event.kind).data(payload),
                            );
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(SSE_HEARTBEAT_INTERVAL)
            .text("keep-alive"),
    )
}

pub(super) async fn get_mermaid_js() -> impl IntoResponse {
    javascript_response(assets::mermaid_js())
}

pub(super) async fn get_mermaid_shim_js() -> impl IntoResponse {
    javascript_response(assets::mermaid_shim_js())
}

pub(super) async fn get_discuss_inspect_js() -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        assets::discuss_inspect_js(),
    )
}
