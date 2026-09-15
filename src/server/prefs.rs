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

    let Value::Object(patch) = body else {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "prefs patch must be a JSON object",
        );
    };

    if let Err(message) = prefs::validate_patch(&patch) {
        return api_error_response(StatusCode::BAD_REQUEST, "validation_error", message);
    }

    // A demo must never reach into the user's home directory, but the page
    // still needs a coherent answer, so merge in memory and discard it.
    if app_state.is_offline_demo() {
        return (
            StatusCode::OK,
            Json(PrefsResponse {
                prefs: prefs::merge(prefs::UiPrefs::new(), &patch),
            }),
        )
            .into_response();
    }

    match prefs::merge_and_save(&patch) {
        Ok(merged) => (StatusCode::OK, Json(PrefsResponse { prefs: merged })).into_response(),
        Err(error) => api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to save preferences: {error}"),
        ),
    }
}
