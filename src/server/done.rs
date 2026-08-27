//! `POST /api/done` — verdict validation, transcript emission, history archive.

use std::io::{self, Write};
use std::path::Path as FsPath;

use axum::Json;
use axum::body::Bytes;
use axum::extract::State as AxumState;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::events::{Event, EventKind};
use crate::history;
use crate::transcript::build_transcript_with_source;
use crate::verdict::Verdict;

use super::app_state::AppState;
use super::response::api_error_response;

#[derive(Debug, Serialize)]
pub(super) struct DoneResponse {
    ok: bool,
    message: &'static str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DoneRequest {
    verdict: DoneVerdictRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DoneVerdictRequest {
    option_id: String,
    #[serde(default)]
    feedback: Option<String>,
}

pub(super) async fn post_api_done(
    AxumState(app_state): AxumState<AppState>,
    body: Bytes,
) -> Response {
    let emitted_at = Utc::now();
    let verdict = match validate_done_verdict(&app_state, &body, emitted_at) {
        Ok(verdict) => verdict,
        Err(response) => return *response,
    };

    let source = match app_state.current_source() {
        Ok(source) => source,
        Err(message) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                message,
            );
        }
    };
    // Past validation, this request will emit a transcript. Latch that before
    // taking the read lock so background writers (the demo responder) can tell
    // that anything they add from here on would be absent from the transcript;
    // `shutdown` is only signalled further below, after the emit.
    app_state.begin_done();
    let transcript = match app_state.state.read() {
        Ok(state) => {
            let transcript = build_transcript_with_source(&state, &source);
            match verdict {
                Some(verdict) => transcript.with_verdict(verdict),
                None => transcript,
            }
        }
        Err(_) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while building transcript",
            );
        }
    };
    let payload = match serde_json::to_value(transcript) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize transcript: {error}"),
            );
        }
    };

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::SessionDone,
        at: emitted_at,
        payload: payload.clone(),
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit session.done event: {error}"),
        );
    }

    if !app_state.no_save() {
        let history_path = history::history_archive_path(
            app_state.history_dir.as_ref().as_path(),
            app_state.source_path.as_ref().as_deref(),
            app_state.files_count(),
            emitted_at,
        );
        if let Err(error) = history::write_history_archive(&history_path, &payload) {
            warn_history_archive_failure(&history_path, &error);
        }
    }

    app_state.record_mutation();
    app_state.shutdown.signal();

    Json(DoneResponse {
        ok: true,
        message: "transcript emitted",
    })
    .into_response()
}

fn validate_done_verdict(
    app_state: &AppState,
    body: &[u8],
    decided_at: DateTime<Utc>,
) -> std::result::Result<Option<Verdict>, Box<Response>> {
    let Some(config) = app_state.verdict_config.as_ref() else {
        return Ok(None);
    };

    if body.is_empty() {
        return Err(Box::new(api_error_response(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "verdict request body is required",
        )));
    }

    let request = serde_json::from_slice::<DoneRequest>(body).map_err(|error| {
        Box::new(api_error_response(
            StatusCode::BAD_REQUEST,
            "bad_request",
            format!("invalid verdict request body: {error}"),
        ))
    })?;

    let Some(option) = config
        .options
        .iter()
        .find(|option| option.id == request.verdict.option_id)
    else {
        return Err(Box::new(api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            format!("unknown verdict optionId: {}", request.verdict.option_id),
        )));
    };

    let feedback = request
        .verdict
        .feedback
        .as_deref()
        .map(str::trim)
        .filter(|feedback| !feedback.is_empty())
        .map(ToOwned::to_owned);

    if option.feedback_required && feedback.is_none() {
        return Err(Box::new(api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            format!("feedback is required for verdict optionId: {}", option.id),
        )));
    }

    Ok(Some(Verdict {
        option_id: option.id.clone(),
        label: option.label.clone(),
        feedback,
        decided_at,
    }))
}

fn warn_history_archive_failure(path: &FsPath, error: &io::Error) {
    tracing::warn!(
        path = %path.display(),
        error = %error,
        "failed to write history archive"
    );
    let _ = writeln!(
        io::stderr(),
        "warning: failed to write history archive to {}: {error}",
        path.display()
    );
}
