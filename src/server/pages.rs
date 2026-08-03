//! Page rendering plus the read-only state, heartbeat, SSE, and asset routes.

use axum::Json;
use axum::extract::State as AxumState;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use tokio::sync::broadcast;

use crate::assets;
use crate::state::{File, FileId, FileKind};
use crate::{render, template};

use super::SSE_HEARTBEAT_INTERVAL;
use super::app_state::AppState;
use super::response::{OkResponse, api_error_response, javascript_response};

pub(super) async fn get_root(AxumState(app_state): AxumState<AppState>) -> Response {
    match render_root_page(&app_state) {
        Ok(page) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
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
            html: render_file_html(file),
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
/// renderer directly, diff files through a synthesized markdown document
/// (heading + one fenced `diff-<lang>` block per hunk).
pub(super) fn render_file_html(file: &File) -> String {
    match file.kind {
        FileKind::Markdown => render::render(&file.content),
        FileKind::Diff => render::render(&crate::diff::diff_content_to_markdown(
            &file.path,
            &file.content,
        )),
    }
}

pub(super) async fn get_api_state(AxumState(app_state): AxumState<AppState>) -> Response {
    match app_state.snapshot_with_files() {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(message) => {
            api_error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
        }
    }
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
