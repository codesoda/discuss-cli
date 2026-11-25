//! Cross-session UI preference endpoint.
//!
//! The page cannot keep these in `localStorage` — every session binds a fresh
//! port, and browser storage is scoped to that origin — so the browser posts
//! each change here and the CLI keeps it in `~/.discuss/prefs.json`.

use axum::Json;
use axum::extract::State as AxumState;
use axum::extract::rejection::JsonRejection;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::Value;

use crate::prefs::{self, UiPrefs};

use super::app_state::AppState;
use super::response::api_error_response;

#[derive(Debug, Serialize)]
pub(super) struct PrefsResponse {
    prefs: UiPrefs,
}

pub(super) async fn post_api_prefs(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<Value>, JsonRejection>,
) -> Response {
    let Json(body) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    let patch = match serde_json::from_value::<UiPrefs>(body) {
        Ok(patch) if !patch.is_empty() => patch,
        Ok(_) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "prefs patch must set at least one preference",
            );
        }
        Err(error) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                format!("invalid prefs patch: {error}"),
            );
        }
    };

    // A demo must never reach into the user's home directory, but the page
    // still needs a coherent answer, so merge in memory and discard it.
    if app_state.is_offline_demo() {
        let prefs = UiPrefs::default().merged_with(&patch);
        return (StatusCode::OK, Json(PrefsResponse { prefs })).into_response();
    }

    match prefs::merge_and_save(&app_state.prefs_path, &patch) {
        Ok(prefs) => (StatusCode::OK, Json(PrefsResponse { prefs })).into_response(),
        Err(error) => api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to save preferences: {error}"),
        ),
    }
}
