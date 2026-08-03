//! Shared response helpers and generic API payload shapes.

use axum::Json;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use super::{ASSET_CACHE_CONTROL, JAVASCRIPT_CONTENT_TYPE};

#[derive(Debug, Serialize)]
pub(super) struct OkResponse {
    pub(super) ok: bool,
}

#[derive(Debug, Serialize)]
struct ApiErrorResponse {
    error: ApiError,
}

#[derive(Debug, Serialize)]
struct ApiError {
    code: &'static str,
    message: String,
}

pub(super) fn api_error_response(
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
) -> Response {
    (
        status,
        Json(ApiErrorResponse {
            error: ApiError {
                code,
                message: message.into(),
            },
        }),
    )
        .into_response()
}

pub(super) fn javascript_response(body: &'static str) -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, JAVASCRIPT_CONTENT_TYPE),
            (header::CACHE_CONTROL, ASSET_CACHE_CONTROL),
        ],
        body,
    )
}

pub(super) async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}
