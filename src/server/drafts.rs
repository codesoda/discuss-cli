//! Draft persistence endpoints for unsent new-thread and follow-up text.

use axum::Json;
use axum::extract::State as AxumState;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::sse::BroadcastEvent;
use crate::state::{Draft, FileId, NewThreadDraftKey, ThreadId};

use super::app_state::AppState;
use super::resolve_file_id;
use super::response::{OkResponse, api_error_response};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpsertNewThreadDraftRequest {
    #[serde(default)]
    file_id: Option<FileId>,
    anchor_start: usize,
    anchor_end: usize,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClearNewThreadDraftRequest {
    #[serde(default)]
    file_id: Option<FileId>,
    anchor_start: usize,
    anchor_end: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpsertFollowupDraftRequest {
    thread_id: ThreadId,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct ClearFollowupDraftRequest {
    thread_id: ThreadId,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NewThreadDraftResponse {
    scope: &'static str,
    file_id: FileId,
    anchor_start: usize,
    anchor_end: usize,
    text: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct NewThreadDraftCleared {
    scope: &'static str,
    file_id: FileId,
    anchor_start: usize,
    anchor_end: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FollowupDraftResponse {
    scope: &'static str,
    thread_id: ThreadId,
    text: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct FollowupDraftCleared {
    scope: &'static str,
    thread_id: ThreadId,
}

pub(super) async fn post_api_drafts_new_thread(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<UpsertNewThreadDraftRequest>, JsonRejection>,
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

    if request.text.trim().is_empty() {
        return clear_new_thread_draft(
            &app_state,
            ClearNewThreadDraftRequest {
                file_id: Some(file_id),
                anchor_start: request.anchor_start,
                anchor_end: request.anchor_end,
            },
        );
    }

    let updated_at = Utc::now();
    let draft = Draft {
        text: request.text,
        updated_at,
    };

    let key = NewThreadDraftKey::new(file_id.clone(), request.anchor_start, request.anchor_end);
    if app_state
        .state
        .write()
        .map(|mut state| state.upsert_new_thread_draft(key, draft.clone()))
        .is_err()
    {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "state lock poisoned while saving new-thread draft",
        );
    }
    app_state.record_mutation();

    let response = NewThreadDraftResponse {
        scope: "newThread",
        file_id,
        anchor_start: request.anchor_start,
        anchor_end: request.anchor_end,
        text: draft.text,
        updated_at,
    };
    let payload = match serde_json::to_value(&response) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize new-thread draft: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: "draft.updated".to_string(),
        payload: payload.clone(),
    });

    Json(response).into_response()
}

pub(super) async fn delete_api_drafts_new_thread(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<ClearNewThreadDraftRequest>, JsonRejection>,
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

    clear_new_thread_draft(&app_state, request)
}

fn clear_new_thread_draft(app_state: &AppState, request: ClearNewThreadDraftRequest) -> Response {
    let file_id = match resolve_file_id(app_state, request.file_id) {
        Ok(file_id) => file_id,
        Err(error) => return *error,
    };
    let key = NewThreadDraftKey::new(file_id.clone(), request.anchor_start, request.anchor_end);
    if app_state
        .state
        .write()
        .map(|mut state| state.clear_new_thread_draft(&key))
        .is_err()
    {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "state lock poisoned while clearing new-thread draft",
        );
    }
    app_state.record_mutation();

    let cleared = NewThreadDraftCleared {
        scope: "newThread",
        file_id,
        anchor_start: request.anchor_start,
        anchor_end: request.anchor_end,
    };
    let payload = match serde_json::to_value(cleared) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize cleared new-thread draft: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: "draft.cleared".to_string(),
        payload: payload.clone(),
    });

    Json(OkResponse { ok: true }).into_response()
}

pub(super) async fn post_api_drafts_followup(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<UpsertFollowupDraftRequest>, JsonRejection>,
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
        return clear_followup_draft(
            &app_state,
            ClearFollowupDraftRequest {
                thread_id: request.thread_id,
            },
        );
    }

    let updated_at = Utc::now();
    let draft = Draft {
        text: request.text,
        updated_at,
    };
    let response = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while saving follow-up draft",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == request.thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", request.thread_id.0),
            );
        }

        state.upsert_followup_draft(request.thread_id.clone(), draft.clone());

        FollowupDraftResponse {
            scope: "followup",
            thread_id: request.thread_id,
            text: draft.text,
            updated_at,
        }
    };
    app_state.record_mutation();
    let payload = match serde_json::to_value(&response) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize follow-up draft: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: "draft.updated".to_string(),
        payload: payload.clone(),
    });

    Json(response).into_response()
}

pub(super) async fn delete_api_drafts_followup(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<ClearFollowupDraftRequest>, JsonRejection>,
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

    clear_followup_draft(&app_state, request)
}

fn clear_followup_draft(app_state: &AppState, request: ClearFollowupDraftRequest) -> Response {
    {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while clearing follow-up draft",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == request.thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", request.thread_id.0),
            );
        }

        state.clear_followup_draft(&request.thread_id);
    }
    app_state.record_mutation();

    let cleared = FollowupDraftCleared {
        scope: "followup",
        thread_id: request.thread_id,
    };
    let payload = match serde_json::to_value(cleared) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize cleared follow-up draft: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: "draft.cleared".to_string(),
        payload: payload.clone(),
    });

    Json(OkResponse { ok: true }).into_response()
}
