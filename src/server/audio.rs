//! Voice notes: the browser uploads a recorded clip, discuss saves it to a
//! per-process temp folder and hands the path to the agent on
//! `thread.created`. An optional engine-independent command supplies transcript text.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::Json;
use axum::body::Bytes;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use super::response::api_error_response;

/// Largest clip accepted. Opus in WebM runs about 16 KB/s, so this is
/// several minutes of talking.
pub(super) const MAX_AUDIO_BYTES: usize = 25 * 1024 * 1024;

static AUDIO_DIR: OnceLock<PathBuf> = OnceLock::new();
static NEXT_AUDIO_NUMBER: AtomicU64 = AtomicU64::new(1);

const AUDIO_TYPES: &[(&str, &str)] = &[
    ("audio/webm", "webm"),
    ("audio/ogg", "ogg"),
    ("audio/mp4", "m4a"),
    ("audio/mpeg", "mp3"),
    ("audio/wav", "wav"),
];

fn audio_dir() -> &'static PathBuf {
    AUDIO_DIR.get_or_init(|| {
        std::env::temp_dir()
            .join("discuss-audio")
            .join(std::process::id().to_string())
    })
}

/// Maps a client-supplied `audioId` (`v-N.ext`) to the saved clip, or `None`
/// when the id is malformed or no such clip was uploaded in this process.
pub(super) fn audio_path_for_id(audio_id: &str) -> Option<PathBuf> {
    let (stem, ext) = audio_id.split_once('.')?;
    let number = stem.strip_prefix("v-")?;
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    if !AUDIO_TYPES.iter().any(|(_, known)| *known == ext) {
        return None;
    }
    let path = audio_dir().join(audio_id);
    path.is_file().then_some(path)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UploadAudioResponse {
    audio_id: String,
    audio_path: String,
    transcript: Option<String>,
}

pub(super) async fn post_api_audio(
    axum::extract::State(state): axum::extract::State<super::AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !state.voice_notes_enabled() {
        return api_error_response(
            StatusCode::FORBIDDEN,
            "voice_notes_disabled",
            "voice notes require a configured transcription command",
        );
    }
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    // Browsers send e.g. `audio/webm;codecs=opus`; match on the base type.
    let base_type = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let Some((_, ext)) = AUDIO_TYPES.iter().find(|(mime, _)| *mime == base_type) else {
        return api_error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            format!("unsupported audio type `{content_type}`"),
        );
    };
    if body.is_empty() {
        return api_error_response(StatusCode::BAD_REQUEST, "bad_request", "empty audio body");
    }

    let dir = audio_dir();
    if let Err(error) = tokio::fs::create_dir_all(dir).await {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to create audio folder: {error}"),
        );
    }
    let audio_id = format!(
        "v-{}.{ext}",
        NEXT_AUDIO_NUMBER.fetch_add(1, Ordering::Relaxed)
    );
    let path = dir.join(&audio_id);
    if let Err(error) = tokio::fs::write(&path, &body).await {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to save audio: {error}"),
        );
    }

    let transcript = if let Some(command) = state
        .transcription_command
        .as_ref()
        .filter(|c| !c.is_empty())
    {
        match transcribe(command, &path).await {
            Ok(text) => Some(text),
            Err(error) => {
                tracing::warn!(%error, "voice transcription failed; preserving audio for agent");
                None
            }
        }
    } else {
        None
    };

    (
        StatusCode::CREATED,
        Json(UploadAudioResponse {
            audio_id,
            audio_path: path.display().to_string(),
            transcript,
        }),
    )
        .into_response()
}

pub(super) async fn get_api_audio(Path(audio_id): Path<String>) -> Response {
    let Some(path) = audio_path_for_id(&audio_id) else {
        return api_error_response(StatusCode::NOT_FOUND, "not_found", "no such audio clip");
    };
    let ext = audio_id.rsplit('.').next().unwrap_or("");
    let mime = AUDIO_TYPES
        .iter()
        .find(|(_, known)| *known == ext)
        .map_or("application/octet-stream", |(mime, _)| mime);
    match tokio::fs::read(&path).await {
        Ok(bytes) => (StatusCode::OK, [(header::CONTENT_TYPE, mime)], bytes).into_response(),
        Err(error) => api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to read audio: {error}"),
        ),
    }
}

async fn transcribe(command: &[String], path: &std::path::Path) -> Result<String, String> {
    let Some(program) = command.first().filter(|value| !value.is_empty()) else {
        return Err("empty transcription command".into());
    };
    let args: Vec<_> = command[1..]
        .iter()
        .map(|arg| arg.replace("{filepath}", &path.to_string_lossy()))
        .collect();
    let mut child = tokio::process::Command::new(program);
    child.args(args).kill_on_drop(true);
    let output = tokio::time::timeout(std::time::Duration::from_secs(120), child.output())
        .await
        .map_err(|_| "transcription timed out".to_string())?
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(format!("transcription exited with {}", output.status));
    }
    let text = String::from_utf8(output.stdout).map_err(|error| error.to_string())?;
    if text.trim().is_empty() {
        return Err("empty transcript".into());
    }
    Ok(text.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unconfigured_voice_notes_reject_uploads() {
        for command in [None, Some(vec![]), Some(vec!["  ".into()])] {
            let state = super::super::AppState::for_process().with_transcription_command(command);
            assert!(!state.voice_notes_enabled());
            let response = post_api_audio(
                axum::extract::State(state),
                HeaderMap::new(),
                Bytes::from_static(b"audio"),
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
        }
        let state = super::super::AppState::for_process()
            .with_transcription_command(Some(vec!["transcribe".into(), "{filepath}".into()]));
        assert!(state.voice_notes_enabled());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn transcription_preserves_path_arguments_and_checks_output() {
        let path = std::path::Path::new("/tmp/a 'quoted' ; clip.webm");
        let command = vec!["printf".into(), "%s".into(), "{filepath}".into()];
        assert_eq!(
            transcribe(&command, path).await.unwrap(),
            path.to_str().unwrap()
        );
        assert!(transcribe(&[], path).await.is_err());
        assert!(transcribe(&["false".into()], path).await.is_err());
        assert!(transcribe(&["true".into()], path).await.is_err());
    }

    #[test]
    fn rejects_malformed_audio_ids() {
        for id in [
            "",
            "v-.webm",
            "v-1",
            "v-1.exe",
            "../v-1.webm",
            "v-1/../x.webm",
            "x-1.webm",
        ] {
            assert!(audio_path_for_id(id).is_none(), "{id} should be rejected");
        }
    }
}
