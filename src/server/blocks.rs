//! Read-only block-segmentation endpoint so agents can compute thread anchors
//! without reimplementing the markdown splitter.

use axum::Json;
use axum::extract::Path;
use axum::extract::State as AxumState;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::blocks::{Block, markdown_blocks};
use crate::diff;
use crate::state::{FileId, FileKind};

use super::app_state::AppState;
use super::response::api_error_response;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BlocksResponse {
    file_id: FileId,
    source_version: u64,
    blocks: Vec<Block>,
}

pub(super) async fn get_api_file_blocks(
    AxumState(app_state): AxumState<AppState>,
    Path(file_id): Path<String>,
) -> Response {
    let file_id = FileId(file_id);
    let Some(file) = app_state.file(&file_id) else {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "unknown_file",
            format!("unknown fileId: {}", file_id.0),
        );
    };

    let blocks = match file.kind {
        FileKind::Markdown => markdown_blocks(&file.content),
        FileKind::Diff => {
            markdown_blocks(&diff::diff_content_to_markdown(&file.path, &file.content))
        }
        FileKind::Image | FileKind::Html => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "blocks are only available for markdown and diff files; use imageAnchor/elementAnchor",
            );
        }
    };

    // The source and state locks are read sequentially, never nested (source
    // updates nest state -> source, so nesting here could deadlock). A source
    // update landing between the reads at worst reports a sourceVersion one
    // ahead of the segmented content — the same race the stale_source_version
    // guard on POST /api/threads already arbitrates.
    let source_version = match app_state.state.read() {
        Ok(state) => state.source_version(),
        Err(_) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while reading source version",
            );
        }
    };

    Json(BlocksResponse {
        file_id,
        source_version,
        blocks,
    })
    .into_response()
}
