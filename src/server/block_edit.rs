//! `GET` / `PATCH /api/blocks/{anchor_idx}` — live block editing: read one
//! anchored block's raw markdown and splice an edited version back into the
//! source, re-anchoring comment threads mechanically.

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State as AxumState};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};

use crate::block_map::{self, BlockKind, BlockMap};
use crate::state::{File, FileId, FileKind, LineRange};

use super::app_state::AppState;
use super::resolve_file_id;
use super::response::api_error_response;
use super::source::source_updated_response;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BlockQuery {
    #[serde(default)]
    file_id: Option<FileId>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct BlockUpdateRequest {
    markdown: String,
    source_version: u64,
    #[serde(default)]
    file_id: Option<FileId>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct BlockResponse {
    markdown: String,
    file_id: FileId,
    source_version: u64,
    line_start: usize,
    line_end: usize,
    kind: BlockKind,
    /// Whether committed edits reach the file on disk (false for stdin docs
    /// and --no-save runs; the editor shows "not saved to file").
    persisted: bool,
}

/// `GET /api/blocks/{anchor_idx}` — the raw markdown behind one anchored
/// block, so a client can edit it in source form.
pub(super) async fn get_api_block(
    AxumState(app_state): AxumState<AppState>,
    Path(anchor_idx): Path<usize>,
    Query(query): Query<BlockQuery>,
) -> Response {
    let file_id = match resolve_file_id(&app_state, query.file_id) {
        Ok(file_id) => file_id,
        Err(response) => return *response,
    };
    let Some(file) = app_state.file(&file_id) else {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "unknown_file",
            format!("unknown fileId: {}", file_id.0),
        );
    };
    if file.kind != FileKind::Markdown {
        return uneditable_file_response(&file);
    }
    let source_version = match app_state.state.read() {
        Ok(state) => state.source_version(),
        Err(_) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while reading block",
            );
        }
    };

    let map = block_map::anchor_blocks(&file.content);
    let Some(block) = map.get(anchor_idx).copied() else {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("block not found: {anchor_idx}"),
        );
    };

    Json(BlockResponse {
        markdown: block_map::block_source(&file.content, &block),
        file_id,
        source_version,
        line_start: block.line_start,
        line_end: block.line_end,
        kind: block.kind,
        persisted: app_state.file_persisted(&file),
    })
    .into_response()
}

/// `PATCH /api/blocks/{anchor_idx}` — replace one anchored block's markdown,
/// splice it into the source, and mechanically re-anchor comment threads on
/// the same file. Broadcasts the same `source.updated` payload as
/// `POST /api/source`, then schedules the debounced write to disk.
pub(super) async fn patch_api_block(
    AxumState(app_state): AxumState<AppState>,
    Path(anchor_idx): Path<usize>,
    payload: std::result::Result<Json<BlockUpdateRequest>, JsonRejection>,
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
        Err(response) => return *response,
    };

    let (updated_file, new_markdown, threads, source_version) = {
        let mut state = match app_state.state.write() {
            Ok(state) => state,
            Err(_) => {
                return api_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "state lock poisoned while updating block",
                );
            }
        };

        let current_version = state.source_version();
        if request.source_version != current_version {
            return api_error_response(
                StatusCode::CONFLICT,
                "stale_source_version",
                format!(
                    "sourceVersion {} is stale; the document is now at version {current_version}, re-fetch the block against the current source",
                    request.source_version
                ),
            );
        }

        let Some(file) = app_state.file(&file_id) else {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "unknown_file",
                format!("unknown fileId: {}", file_id.0),
            );
        };
        if file.kind != FileKind::Markdown {
            return uneditable_file_response(&file);
        }
        let old_map = block_map::anchor_blocks(&file.content);
        let Some(block) = old_map.get(anchor_idx).copied() else {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("block not found: {anchor_idx}"),
            );
        };

        let new_markdown = match block_map::splice(
            &file.content,
            block.line_start,
            block.line_end,
            &request.markdown,
        ) {
            Ok(markdown) => markdown,
            Err(error) => {
                return api_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    format!("block map produced an unspliceable range: {error}"),
                );
            }
        };

        // The edit is confined to one anchor block's lines, so the change in
        // total anchor count is what the replacement produced there (minus
        // the anchors it covered: itself plus nested table rows). Threads on
        // this file are carried mechanically off that delta; anchors that
        // can't be carried orphan, matching POST /api/source semantics.
        // Threads on other files live in different anchor spaces.
        let new_map = block_map::anchor_blocks(&new_markdown);
        let old_span_len = old_map.span_len(anchor_idx);
        let delta = new_map.blocks.len() as isize - old_map.blocks.len() as isize;
        let new_total = new_map.blocks.len();

        let snapshot = state.get_threads();
        for existing in &snapshot {
            if existing.orphaned || existing.file_id != file_id {
                continue;
            }
            let carried = block_map::carry_anchor_across_edit(
                existing.anchor_start,
                existing.anchor_end,
                anchor_idx,
                old_span_len,
                delta,
                new_total,
            );
            let Some(thread) = state.thread_mut(&existing.id) else {
                continue;
            };
            match carried {
                Some((anchor_start, anchor_end)) => {
                    thread.anchor_start = anchor_start;
                    thread.anchor_end = anchor_end;
                    thread.line_range = line_range_for(&new_map, anchor_start, anchor_end);
                }
                None => thread.orphaned = true,
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
                file.content = new_markdown.clone();
                file.clone()
            }
            Err(_) => {
                return api_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "source lock poisoned while updating block",
                );
            }
        };

        let source_version = state.bump_source_version();
        (
            updated_file,
            new_markdown,
            state.get_threads(),
            source_version,
        )
    };
    app_state.record_mutation();
    // User edits are born in discuss, so discuss owns writing them back to
    // the file (unlike POST /api/source, where the agent already owns it).
    app_state.schedule_source_write(&updated_file);
    source_updated_response(
        &app_state,
        &updated_file,
        new_markdown,
        &threads,
        source_version,
    )
}

fn uneditable_file_response(file: &File) -> Response {
    api_error_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        "uneditable_file",
        format!(
            "only markdown files support block editing; {} is {}",
            file.path,
            match file.kind {
                FileKind::Diff => "a diff",
                FileKind::Image => "an image",
                FileKind::Html => "an HTML prototype",
                FileKind::Markdown => "not editable",
            }
        ),
    )
}

/// The source lines a carried anchor range spans in the new document, when
/// both endpoints resolve to mapped blocks.
pub(super) fn line_range_for(
    map: &BlockMap,
    anchor_start: usize,
    anchor_end: usize,
) -> Option<LineRange> {
    let start_block = map.get(anchor_start)?;
    let end_block = map.get(anchor_end)?;
    let start = u32::try_from(start_block.line_start).ok()?;
    let end = u32::try_from(end_block.line_end.max(start_block.line_start)).ok()?;
    Some(LineRange { start, end })
}
