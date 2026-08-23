//! Browser-reported HTML element anchor resolution.

use std::collections::HashSet;

use axum::Json;
use axum::extract::State as AxumState;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::sse::BroadcastEvent;
use crate::state::{FileId, FileKind, ThreadId};

use super::app_state::AppState;
use super::resolve_file_id;
use super::response::api_error_response;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ResolveAnchorsRequest {
    #[serde(default)]
    file_id: Option<FileId>,
    #[serde(default)]
    detached_thread_ids: Vec<ThreadId>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ResolveAnchorsResponse {
    file_id: FileId,
    detached_thread_ids: Vec<ThreadId>,
}

pub(super) async fn post_api_anchors_resolve(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<ResolveAnchorsRequest>, JsonRejection>,
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
    let file_id = match resolve_file_id(&app_state, request.file_id) {
        Ok(file_id) => file_id,
        Err(error) => return *error,
    };
    if app_state.file_kind(&file_id) != Some(FileKind::Html) {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "element anchor resolution is only valid for HTML files",
        );
    }

    let detached: HashSet<ThreadId> = request.detached_thread_ids.into_iter().collect();
    let detached_thread_ids = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while resolving element anchors",
            );
        };
        let anchored_threads = state
            .get_threads()
            .into_iter()
            .filter(|thread| thread.file_id == file_id && thread.element_anchor.is_some())
            .collect::<Vec<_>>();
        let known: HashSet<&ThreadId> = anchored_threads.iter().map(|thread| &thread.id).collect();
        if let Some(unknown) = detached.iter().find(|thread_id| !known.contains(thread_id)) {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                format!(
                    "detachedThreadIds references a thread that is not active on HTML file {}: {}",
                    file_id.0, unknown.0
                ),
            );
        }
        for thread in &anchored_threads {
            if let Some(stored) = state.thread_mut(&thread.id) {
                stored.orphaned = detached.contains(&thread.id);
            }
        }
        anchored_threads
            .into_iter()
            .filter(|thread| detached.contains(&thread.id))
            .map(|thread| thread.id)
            .collect::<Vec<_>>()
    };

    app_state.record_mutation();
    let response = ResolveAnchorsResponse {
        file_id,
        detached_thread_ids,
    };
    let event_payload = match serde_json::to_value(&response) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize anchor resolution: {error}"),
            );
        }
    };
    app_state.bus.publish(BroadcastEvent {
        kind: "anchors.resolved".to_string(),
        payload: event_payload,
    });
    Json(response).into_response()
}
