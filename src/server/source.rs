//! `POST /api/source` — live source updates with agent-pushed re-anchoring.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::State as AxumState;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::events::{Event, EventKind};
use crate::sse::BroadcastEvent;
use crate::state::{FileId, FileKind, LineRange, ThreadId};

use super::app_state::AppState;
use super::pages::render_file_html;
use super::resolve_file_id;
use super::response::api_error_response;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SourceUpdateRequest {
    markdown: String,
    #[serde(default)]
    file_id: Option<FileId>,
    thread_anchors: Vec<ThreadAnchorUpdate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ThreadAnchorUpdate {
    thread_id: ThreadId,
    #[serde(default)]
    anchor_start: Option<usize>,
    #[serde(default)]
    anchor_end: Option<usize>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    line_range: Option<LineRange>,
    #[serde(default)]
    orphaned: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SourceUpdatedPayload {
    markdown: String,
    file_id: FileId,
    rendered_html: String,
    thread_anchors: Vec<ThreadAnchorResponse>,
    orphaned_thread_ids: Vec<ThreadId>,
    source_version: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ThreadAnchorResponse {
    thread_id: ThreadId,
    anchor_start: usize,
    anchor_end: usize,
    orphaned: bool,
}

pub(super) async fn post_api_source(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<SourceUpdateRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    let mut updates: HashMap<ThreadId, ThreadAnchorUpdate> = HashMap::new();
    for update in request.thread_anchors {
        if !update.orphaned {
            let (Some(start), Some(end)) = (update.anchor_start, update.anchor_end) else {
                return api_error_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    format!(
                        "thread {} must provide anchorStart and anchorEnd, or set orphaned: true",
                        update.thread_id.0
                    ),
                );
            };
            if start == 0 || end < start {
                return api_error_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    format!(
                        "thread {} anchors must satisfy 1 <= anchorStart <= anchorEnd",
                        update.thread_id.0
                    ),
                );
            }
            if let Some(line_range) = update.line_range
                && (line_range.start == 0 || line_range.end < line_range.start)
            {
                return api_error_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    format!(
                        "thread {} lineRange must satisfy 1 <= start <= end",
                        update.thread_id.0
                    ),
                );
            }
        }
        let thread_id = update.thread_id.clone();
        if updates.insert(thread_id.clone(), update).is_some() {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                format!(
                    "thread {} appears more than once in threadAnchors",
                    thread_id.0
                ),
            );
        }
    }

    let file_id = match resolve_file_id(&app_state, request.file_id) {
        Ok(file_id) => file_id,
        Err(error) => return *error,
    };
    if app_state.file_kind(&file_id) == Some(FileKind::Image) {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "live source updates are not supported for image files",
        );
    }

    let markdown = request.markdown;
    let (threads, source_version, updated_file) = {
        let mut state = match app_state.state.write() {
            Ok(state) => state,
            Err(_) => {
                return api_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "state lock poisoned while updating source",
                );
            }
        };

        // Strict coverage in both directions, scoped to the updated file: a
        // silently-forgotten thread would drift onto wrong content, and an
        // unknown thread id is an agent bug worth surfacing.
        let active_ids: Vec<ThreadId> = state
            .get_threads()
            .into_iter()
            .filter(|t| t.file_id == file_id)
            .map(|t| t.id)
            .collect();
        let active_set: HashSet<&ThreadId> = active_ids.iter().collect();
        if let Some(missing) = active_ids.iter().find(|id| !updates.contains_key(id)) {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                format!(
                    "threadAnchors must cover every active thread on file {}: missing {} (re-anchor it or mark it orphaned)",
                    file_id.0, missing.0
                ),
            );
        }
        if let Some(unknown) = updates.keys().find(|id| !active_set.contains(id)) {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                format!(
                    "threadAnchors references a thread that is not active on file {}: {}",
                    file_id.0, unknown.0
                ),
            );
        }

        for (thread_id, update) in &updates {
            let Some(thread) = state.thread_mut(thread_id) else {
                continue; // unreachable: validated active above
            };
            if update.orphaned {
                thread.orphaned = true;
            } else {
                thread.orphaned = false;
                thread.anchor_start = update.anchor_start.expect("validated anchorStart");
                thread.anchor_end = update.anchor_end.expect("validated anchorEnd");
                thread.line_range = update.line_range;
                if let Some(snippet) = &update.snippet {
                    thread.snippet = snippet.clone();
                }
            }
        }

        // Swap the file content while still holding the state write lock so
        // no reader can observe new anchors against the old source or vice
        // versa.
        let updated_file = match app_state.source.write() {
            Ok(mut source) => {
                let Some(file) = source.files.iter_mut().find(|file| file.id == file_id) else {
                    return api_error_response(
                        StatusCode::NOT_FOUND,
                        "unknown_file",
                        format!("unknown fileId: {}", file_id.0),
                    );
                };
                file.content = markdown.clone();
                file.clone()
            }
            Err(_) => {
                return api_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "source lock poisoned while updating source",
                );
            }
        };

        let source_version = state.bump_source_version();
        (state.get_threads(), source_version, updated_file)
    };
    app_state.record_mutation();

    let rendered_html = render_file_html(&updated_file);
    let thread_anchors = threads
        .iter()
        .filter(|thread| thread.file_id == file_id)
        .map(|thread| ThreadAnchorResponse {
            thread_id: thread.id.clone(),
            anchor_start: thread.anchor_start,
            anchor_end: thread.anchor_end,
            orphaned: thread.orphaned,
        })
        .collect();
    let orphaned_thread_ids = threads
        .iter()
        .filter(|thread| thread.orphaned && thread.file_id == file_id)
        .map(|thread| thread.id.clone())
        .collect();
    let payload = SourceUpdatedPayload {
        markdown,
        file_id,
        rendered_html,
        thread_anchors,
        orphaned_thread_ids,
        source_version,
    };
    let payload = match serde_json::to_value(&payload) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize source.updated payload: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::SourceUpdated.to_string(),
        payload: payload.clone(),
    });
    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::SourceUpdated,
        at: Utc::now(),
        payload: payload.clone(),
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit source.updated event: {error}"),
        );
    }

    Json(payload).into_response()
}
