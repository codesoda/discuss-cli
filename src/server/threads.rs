//! Thread lifecycle endpoints: create, reply, take, resolve, unresolve, delete.

use axum::Json;
use axum::extract::Path;
use axum::extract::State as AxumState;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::events::{Event, EventKind};
use crate::sse::BroadcastEvent;
use crate::state::{
    FileId, FileKind, ImageAnchor, LineRange, Reply, Resolution, Take, Thread, ThreadId, ThreadKind,
};

use super::app_state::AppState;
use super::resolve_file_id;
use super::response::{OkResponse, api_error_response};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateThreadRequest {
    /// Which file the anchors refer to. Optional in single-file sessions
    /// (defaults to the only file), required when multiple files are loaded.
    #[serde(default)]
    file_id: Option<FileId>,
    #[serde(default)]
    anchor_start: Option<usize>,
    #[serde(default)]
    anchor_end: Option<usize>,
    #[serde(default)]
    image_anchor: Option<ImageAnchor>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    line_range: Option<LineRange>,
    /// Optional optimistic-concurrency guard: when set, the thread is only
    /// created if the server's current source version matches, so anchors
    /// computed against an outdated document are rejected instead of drifting.
    #[serde(default)]
    source_version: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateThreadResponse {
    id: ThreadId,
    file_id: FileId,
    anchor_start: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_anchor: Option<ImageAnchor>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub(super) struct AddReplyRequest {
    text: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct AddTakeRequest {
    text: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ResolveThreadRequest {
    decision: Option<String>,
}

pub(super) async fn post_api_threads(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<CreateThreadRequest>, JsonRejection>,
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
    if let Some(line_range) = request.line_range
        && (line_range.start == 0 || line_range.end < line_range.start)
    {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "lineRange must satisfy 1 <= start <= end",
        );
    }
    if let Some(requested_version) = request.source_version {
        let current_version = match app_state.state.read() {
            Ok(state) => state.source_version(),
            Err(_) => {
                return api_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "state lock poisoned while checking source version",
                );
            }
        };
        if requested_version != current_version {
            return api_error_response(
                StatusCode::CONFLICT,
                "stale_source_version",
                format!(
                    "sourceVersion {requested_version} is stale; the document is now at version {current_version}, refresh anchors against the current source"
                ),
            );
        }
    }
    let file_id = match resolve_file_id(&app_state, request.file_id) {
        Ok(file_id) => file_id,
        Err(error) => return *error,
    };
    let Some(file_kind) = app_state.file_kind(&file_id) else {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "unknown_file",
            format!("unknown fileId: {}", file_id.0),
        );
    };

    let (text_anchor, image_anchor, snippet, text, line_range) = if file_kind == FileKind::Image {
        let Some(image_anchor) = request.image_anchor else {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "imageAnchor is required for image files",
            );
        };
        if image_anchor.x_pct > 10_000 || image_anchor.y_pct > 10_000 {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "imageAnchor coordinates must be between 0 and 10000 basis points",
            );
        }
        if request.line_range.is_some() {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "lineRange is not supported for image files",
            );
        }
        let Some(_) = request.snippet else {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "missing field `snippet`",
            );
        };
        let Some(text) = request.text else {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "missing field `text`",
            );
        };
        (None, Some(image_anchor), String::new(), text, None)
    } else {
        if request.image_anchor.is_some() {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "imageAnchor is only valid for image files",
            );
        }
        let Some(anchor_start) = request.anchor_start else {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "missing field `anchorStart`",
            );
        };
        let Some(anchor_end) = request.anchor_end else {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "missing field `anchorEnd`",
            );
        };
        if anchor_start == 0 || anchor_end < anchor_start {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "anchors must satisfy 1 <= anchorStart <= anchorEnd",
            );
        }
        let Some(snippet) = request.snippet else {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "missing field `snippet`",
            );
        };
        let Some(text) = request.text else {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "missing field `text`",
            );
        };
        (
            Some((anchor_start, anchor_end)),
            None,
            snippet,
            text,
            request.line_range,
        )
    };

    let created_at = Utc::now();
    let thread = {
        let mut state = match app_state.state.write() {
            Ok(state) => state,
            Err(_) => {
                return api_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "state lock poisoned while creating thread",
                );
            }
        };
        let (anchor_start, anchor_end, breadcrumb) = if let Some(image_anchor) = image_anchor {
            let pin_number = state
                .all_threads()
                .iter()
                .filter(|thread| thread.file_id == file_id && thread.image_anchor.is_some())
                .map(|thread| thread.anchor_start)
                .max()
                .unwrap_or(0)
                + 1;
            (
                pin_number,
                pin_number,
                format!(
                    "pin {pin_number} at {}%,{}%",
                    format_basis_points(image_anchor.x_pct),
                    format_basis_points(image_anchor.y_pct)
                ),
            )
        } else {
            let (anchor_start, anchor_end) = text_anchor.expect("validated text anchors");
            (anchor_start, anchor_end, String::new())
        };
        let thread = Thread {
            id: app_state.next_user_thread_id(),
            file_id: file_id.clone(),
            anchor_start,
            anchor_end,
            image_anchor,
            snippet,
            breadcrumb,
            text,
            created_at,
            kind: ThreadKind::User,
            line_range,
            orphaned: false,
        };
        state.add_thread(thread.clone());
        thread
    };
    app_state.record_mutation();

    let payload = match serde_json::to_value(&thread) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize created thread: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::ThreadCreated.to_string(),
        payload: payload.clone(),
    });

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::ThreadCreated,
        at: created_at,
        payload,
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit thread.created event: {error}"),
        );
    }

    Json(CreateThreadResponse {
        id: thread.id,
        file_id,
        anchor_start: thread.anchor_start,
        image_anchor: thread.image_anchor,
        created_at,
    })
    .into_response()
}

fn format_basis_points(value: u16) -> String {
    let whole = value / 100;
    let fraction = value % 100;
    match fraction {
        0 => whole.to_string(),
        fraction if fraction % 10 == 0 => format!("{whole}.{}", fraction / 10),
        fraction => format!("{whole}.{fraction:02}"),
    }
}

pub(super) async fn post_api_thread_replies(
    AxumState(app_state): AxumState<AppState>,
    Path(thread_id): Path<String>,
    payload: std::result::Result<Json<AddReplyRequest>, JsonRejection>,
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

    if request.text.trim().is_empty() {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "reply text must not be empty",
        );
    }

    let thread_id = ThreadId(thread_id);
    let reply = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while adding reply",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        }

        state.add_reply(Reply {
            id: app_state.next_reply_id(),
            thread_id: thread_id.clone(),
            text: request.text,
            created_at: Utc::now(),
        })
    };
    app_state.record_mutation();

    let payload = match serde_json::to_value(&reply) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize reply: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::ReplyAdded.to_string(),
        payload: payload.clone(),
    });

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::ReplyAdded,
        at: reply.created_at,
        payload,
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit reply.added event: {error}"),
        );
    }

    Json(reply).into_response()
}

pub(super) async fn post_api_thread_takes(
    AxumState(app_state): AxumState<AppState>,
    Path(thread_id): Path<String>,
    payload: std::result::Result<Json<AddTakeRequest>, JsonRejection>,
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

    if request.text.trim().is_empty() {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "take text must not be empty",
        );
    }

    let thread_id = ThreadId(thread_id);
    let take = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while adding take",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        }

        state.add_take(Take {
            id: app_state.next_take_id(),
            thread_id: thread_id.clone(),
            text: request.text,
            created_at: Utc::now(),
        })
    };
    app_state.record_mutation();

    let payload = match serde_json::to_value(&take) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize take: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: "take.added".to_string(),
        payload: payload.clone(),
    });

    Json(take).into_response()
}

pub(super) async fn post_api_thread_resolve(
    AxumState(app_state): AxumState<AppState>,
    Path(thread_id): Path<String>,
    payload: std::result::Result<Json<ResolveThreadRequest>, JsonRejection>,
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

    let thread_id = ThreadId(thread_id);
    let resolution = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while resolving thread",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        }

        state.set_resolution(
            thread_id.clone(),
            Resolution {
                decision: request.decision,
                resolved_at: Utc::now(),
            },
        )
    };
    app_state.record_mutation();

    let payload = serde_json::json!({
        "threadId": thread_id,
        "resolution": resolution,
    });

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::ThreadResolved.to_string(),
        payload: payload.clone(),
    });

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::ThreadResolved,
        at: resolution.resolved_at,
        payload,
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit thread.resolved event: {error}"),
        );
    }

    Json(resolution).into_response()
}

pub(super) async fn post_api_thread_unresolve(
    AxumState(app_state): AxumState<AppState>,
    Path(thread_id): Path<String>,
) -> Response {
    let thread_id = ThreadId(thread_id);
    let emitted_at = Utc::now();

    {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while unresolving thread",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        }

        state.clear_resolution(&thread_id);
    }
    app_state.record_mutation();

    let payload = serde_json::json!({ "threadId": thread_id });

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::ThreadUnresolved.to_string(),
        payload: payload.clone(),
    });

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::ThreadUnresolved,
        at: emitted_at,
        payload,
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit thread.unresolved event: {error}"),
        );
    }

    Json(OkResponse { ok: true }).into_response()
}

pub(super) async fn delete_api_thread(
    AxumState(app_state): AxumState<AppState>,
    Path(thread_id): Path<String>,
) -> Response {
    let thread_id = ThreadId(thread_id);
    let emitted_at = Utc::now();

    {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while deleting thread",
            );
        };

        let Some(thread) = state
            .get_threads()
            .into_iter()
            .find(|thread| thread.id == thread_id)
        else {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        };

        if thread.kind == ThreadKind::Prepopulated {
            return api_error_response(
                StatusCode::FORBIDDEN,
                "prepopulated_thread",
                format!("prepopulated thread cannot be deleted: {}", thread_id.0),
            );
        }

        state.soft_delete_thread(&thread_id);
    }
    app_state.record_mutation();

    let payload = serde_json::json!({ "threadId": thread_id });

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::ThreadDeleted.to_string(),
        payload: payload.clone(),
    });

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::ThreadDeleted,
        at: emitted_at,
        payload,
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit thread.deleted event: {error}"),
        );
    }

    Json(OkResponse { ok: true }).into_response()
}
