//! Held-back rewrites: the agent proposes a whole new document, the reviewer
//! accepts all, some, or none of its changed blocks. Nothing on screen or on
//! disk changes until they do.
//!
//! - `GET /api/proposals` — pending proposals plus the hold switch.
//! - `POST /api/proposals` — the agent posts `{markdown, summary, baseVersion}`.
//! - `POST /api/proposals/{id}/accept` — apply all hunks, or `hunkIds`.
//! - `POST /api/proposals/{id}/discard` — drop it; `redo: true` asks the
//!   agent to try again against the current document.
//! - `PUT /api/proposals/hold` — when on, agent `POST /api/source` pushes
//!   arrive as proposals too.
//! - `POST /api/rewrite` — the reviewer's Rewrite button; tells the agent.

use std::collections::{BTreeSet, HashMap};

use axum::Json;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State as AxumState};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::block_map;
use crate::events::{Event, EventKind};
use crate::proposal_diff::BlockDiff;
use crate::render;
use crate::sse::BroadcastEvent;
use crate::state::{FileId, FileKind};

use super::app_state::AppState;
use super::block_edit::line_range_for;
use super::resolve_file_id;
use super::response::api_error_response;
use super::source::source_updated_response;

/// One pending proposal per file; a newer one replaces it.
#[derive(Debug, Default)]
pub(crate) struct ProposalStore {
    by_file: HashMap<FileId, Proposal>,
    next_number: u64,
}

#[derive(Debug, Clone)]
struct Proposal {
    id: String,
    file_id: FileId,
    summary: String,
    base_version: u64,
    created_at: DateTime<Utc>,
    /// `"proposal"` for `POST /api/proposals`, `"source"` for a held push.
    origin: &'static str,
    diff: BlockDiff,
}

impl Proposal {
    fn to_json(&self, current_version: u64) -> Value {
        let hunks: Vec<Value> = self
            .diff
            .hunks
            .iter()
            .map(|hunk| {
                json!({
                    "id": hunk.id,
                    "oldAnchorStart": hunk.old_anchors.map(|(start, _)| start),
                    "oldAnchorEnd": hunk.old_anchors.map(|(_, end)| end),
                    "insertAfter": hunk.insert_after,
                    "oldMarkdown": hunk.old_markdown,
                    "newMarkdown": hunk.new_markdown,
                    "newHtml": render::render(&hunk.new_markdown),
                })
            })
            .collect();
        json!({
            "proposalId": self.id,
            "fileId": self.file_id,
            "summary": self.summary,
            "baseVersion": self.base_version,
            "stale": self.base_version != current_version,
            "createdAt": self.created_at,
            "origin": self.origin,
            "hunks": hunks,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CreateProposalRequest {
    markdown: String,
    #[serde(default)]
    summary: String,
    base_version: u64,
    #[serde(default)]
    file_id: Option<FileId>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AcceptProposalRequest {
    /// Hunks to apply; omitted means all of them.
    #[serde(default)]
    hunk_ids: Option<Vec<usize>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DiscardProposalRequest {
    #[serde(default)]
    redo: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct HoldRequest {
    hold: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RewriteRequest {
    #[serde(default)]
    instruction: String,
    #[serde(default)]
    file_id: Option<FileId>,
}

fn bad_request(rejection: JsonRejection) -> Response {
    api_error_response(
        StatusCode::BAD_REQUEST,
        "bad_request",
        rejection.body_text(),
    )
}

fn lock_error(what: &str) -> Response {
    api_error_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal_error",
        format!("{what} lock poisoned"),
    )
}

fn current_version(app_state: &AppState) -> u64 {
    app_state
        .state
        .read()
        .map(|state| state.source_version())
        .unwrap_or(0)
}

/// Publish on SSE, and on stdout when the agent needs to hear about it.
fn announce(app_state: &AppState, kind: EventKind, payload: Value, to_agent: bool) {
    app_state.bus.publish(BroadcastEvent {
        kind: kind.to_string(),
        payload: payload.clone(),
    });
    if to_agent
        && let Err(error) = app_state.emitter.emit(&Event {
            kind,
            at: Utc::now(),
            payload,
        })
    {
        tracing::warn!(error = %error, "failed to emit {kind} event");
    }
}

pub(super) async fn get_api_proposals(AxumState(app_state): AxumState<AppState>) -> Response {
    let version = current_version(&app_state);
    let Ok(store) = app_state.proposals.lock() else {
        return lock_error("proposal");
    };
    let mut proposals: Vec<&Proposal> = store.by_file.values().collect();
    proposals.sort_by(|a, b| a.id.cmp(&b.id));
    Json(json!({
        "holdAgentEdits": app_state.hold_agent_edits(),
        "sourceVersion": version,
        "proposals": proposals.iter().map(|p| p.to_json(version)).collect::<Vec<_>>(),
    }))
    .into_response()
}

pub(super) async fn post_api_proposals(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<CreateProposalRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => return bad_request(rejection),
    };
    let file_id = match resolve_file_id(&app_state, request.file_id) {
        Ok(file_id) => file_id,
        Err(response) => return *response,
    };
    let version = current_version(&app_state);
    if request.base_version != version {
        return api_error_response(
            StatusCode::CONFLICT,
            "stale_source_version",
            format!(
                "baseVersion {} is stale; the document is now at version {version}. Re-read the source and propose against it",
                request.base_version
            ),
        );
    }
    match create_proposal(
        &app_state,
        file_id,
        &request.markdown,
        request.summary,
        version,
        "proposal",
    ) {
        Ok(proposal) => (StatusCode::CREATED, Json(proposal)).into_response(),
        Err(response) => *response,
    }
}

/// Store a proposal for `file_id` (replacing any pending one) and tell the
/// browser. Shared with held `POST /api/source` pushes.
pub(super) fn create_proposal(
    app_state: &AppState,
    file_id: FileId,
    markdown: &str,
    summary: String,
    base_version: u64,
    origin: &'static str,
) -> std::result::Result<Value, Box<Response>> {
    let Some(file) = app_state.file(&file_id) else {
        return Err(Box::new(api_error_response(
            StatusCode::NOT_FOUND,
            "unknown_file",
            format!("unknown fileId: {}", file_id.0),
        )));
    };
    if file.kind != FileKind::Markdown {
        return Err(Box::new(api_error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "uneditable_file",
            "only markdown files take rewrite proposals",
        )));
    }
    let diff = BlockDiff::new(&file.content, markdown);
    if diff.hunks.is_empty() {
        return Err(Box::new(api_error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "no_changes",
            "the proposed markdown has no block changes",
        )));
    }

    let (proposal, replaced) = {
        let Ok(mut store) = app_state.proposals.lock() else {
            return Err(Box::new(lock_error("proposal")));
        };
        store.next_number += 1;
        let proposal = Proposal {
            id: format!("p-{}", store.next_number),
            file_id: file_id.clone(),
            summary,
            base_version,
            created_at: Utc::now(),
            origin,
            diff,
        };
        let replaced = store.by_file.insert(file_id, proposal.clone());
        (proposal, replaced)
    };
    if let Some(old) = replaced {
        announce(
            app_state,
            EventKind::ProposalDiscarded,
            json!({"proposalId": old.id, "fileId": old.file_id, "reason": "replaced", "redoRequested": false}),
            false,
        );
    }
    let payload = proposal.to_json(base_version);
    // The agent made it, so the browser is the only one who needs to hear.
    announce(
        app_state,
        EventKind::ProposalCreated,
        payload.clone(),
        false,
    );
    app_state.record_mutation();
    Ok(payload)
}

fn take_proposal(
    app_state: &AppState,
    proposal_id: &str,
) -> std::result::Result<Proposal, Box<Response>> {
    let Ok(store) = app_state.proposals.lock() else {
        return Err(Box::new(lock_error("proposal")));
    };
    store
        .by_file
        .values()
        .find(|p| p.id == proposal_id)
        .cloned()
        .ok_or_else(|| {
            Box::new(api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("no pending proposal {proposal_id}"),
            ))
        })
}

fn remove_proposal(app_state: &AppState, proposal: &Proposal) {
    if let Ok(mut store) = app_state.proposals.lock()
        && store
            .by_file
            .get(&proposal.file_id)
            .is_some_and(|p| p.id == proposal.id)
    {
        store.by_file.remove(&proposal.file_id);
    }
}

pub(super) async fn post_api_proposal_accept(
    AxumState(app_state): AxumState<AppState>,
    Path(proposal_id): Path<String>,
    payload: Option<Json<AcceptProposalRequest>>,
) -> Response {
    let request = payload.map(|Json(r)| r).unwrap_or_default();
    let proposal = match take_proposal(&app_state, &proposal_id) {
        Ok(proposal) => proposal,
        Err(response) => return *response,
    };
    let all_ids: BTreeSet<usize> = proposal.diff.hunks.iter().map(|h| h.id).collect();
    let accepted: BTreeSet<usize> = match request.hunk_ids {
        None => all_ids.clone(),
        Some(ids) => {
            if let Some(unknown) = ids.iter().find(|id| !all_ids.contains(id)) {
                return api_error_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    format!("proposal {proposal_id} has no hunk {unknown}"),
                );
            }
            ids.into_iter().collect()
        }
    };
    let file_id = proposal.file_id.clone();

    let (updated_file, new_markdown, threads, source_version) = {
        let Ok(mut state) = app_state.state.write() else {
            return lock_error("state");
        };
        let current = state.source_version();
        if current != proposal.base_version {
            return api_error_response(
                StatusCode::CONFLICT,
                "stale_proposal",
                format!(
                    "proposal {proposal_id} was made against version {} but the document is now at {current}; discard it and ask for a redo",
                    proposal.base_version
                ),
            );
        }
        let composed = proposal.diff.compose(|id| accepted.contains(&id));
        let new_map = block_map::anchor_blocks(&composed.markdown);

        for existing in state.get_threads() {
            if existing.orphaned || existing.file_id != file_id {
                continue;
            }
            let carried = composed.carry(existing.anchor_start, existing.anchor_end);
            let Some(thread) = state.thread_mut(&existing.id) else {
                continue;
            };
            match carried {
                Some((start, end)) => {
                    thread.anchor_start = start;
                    thread.anchor_end = end;
                    thread.line_range = line_range_for(&new_map, start, end);
                }
                None => thread.orphaned = true,
            }
        }

        let updated_file = match app_state.source.write() {
            Ok(mut source) => {
                let Some(file) = source.files.iter_mut().find(|file| file.id == file_id) else {
                    return api_error_response(
                        StatusCode::NOT_FOUND,
                        "unknown_file",
                        format!("unknown fileId: {}", file_id.0),
                    );
                };
                file.content = composed.markdown.clone();
                file.clone()
            }
            Err(_) => return lock_error("source"),
        };
        let source_version = state.bump_source_version();
        (
            updated_file,
            composed.markdown,
            state.get_threads(),
            source_version,
        )
    };
    remove_proposal(&app_state, &proposal);
    app_state.record_mutation();
    // The reviewer approved it in discuss, so discuss writes it to disk.
    app_state.schedule_source_write(&updated_file);
    let response = source_updated_response(
        &app_state,
        &updated_file,
        new_markdown,
        &threads,
        source_version,
    );
    if !response.status().is_success() {
        return response;
    }

    let rejected: Vec<Value> = proposal
        .diff
        .hunks
        .iter()
        .filter(|h| !accepted.contains(&h.id))
        .map(|h| json!({"id": h.id, "oldMarkdown": h.old_markdown, "newMarkdown": h.new_markdown}))
        .collect();
    let payload = json!({
        "proposalId": proposal.id,
        "fileId": file_id,
        "acceptedHunkIds": accepted,
        "rejectedHunks": rejected,
        "sourceVersion": source_version,
    });
    announce(
        &app_state,
        EventKind::ProposalAccepted,
        payload.clone(),
        true,
    );
    Json(payload).into_response()
}

pub(super) async fn post_api_proposal_discard(
    AxumState(app_state): AxumState<AppState>,
    Path(proposal_id): Path<String>,
    payload: Option<Json<DiscardProposalRequest>>,
) -> Response {
    let request = payload.map(|Json(r)| r).unwrap_or_default();
    let proposal = match take_proposal(&app_state, &proposal_id) {
        Ok(proposal) => proposal,
        Err(response) => return *response,
    };
    remove_proposal(&app_state, &proposal);
    app_state.record_mutation();
    let payload = json!({
        "proposalId": proposal.id,
        "fileId": proposal.file_id,
        "reason": "user",
        "redoRequested": request.redo,
        "summary": proposal.summary,
    });
    announce(
        &app_state,
        EventKind::ProposalDiscarded,
        payload.clone(),
        true,
    );
    Json(payload).into_response()
}

pub(super) async fn put_api_proposals_hold(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<HoldRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => return bad_request(rejection),
    };
    app_state.set_hold_agent_edits(request.hold);
    let payload = json!({"holdAgentEdits": request.hold});
    app_state.bus.publish(BroadcastEvent {
        kind: "proposal.hold_changed".to_string(),
        payload: payload.clone(),
    });
    Json(payload).into_response()
}

pub(super) async fn post_api_rewrite(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<RewriteRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => return bad_request(rejection),
    };
    let file_id = match resolve_file_id(&app_state, request.file_id) {
        Ok(file_id) => file_id,
        Err(response) => return *response,
    };
    if app_state.file_kind(&file_id) != Some(FileKind::Markdown) {
        return api_error_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            "uneditable_file",
            "only markdown files can be rewritten",
        );
    }
    let payload = json!({
        "fileId": file_id,
        "instruction": request.instruction.trim(),
        "sourceVersion": current_version(&app_state),
    });
    app_state.record_mutation();
    announce(
        &app_state,
        EventKind::RewriteRequested,
        payload.clone(),
        true,
    );
    (StatusCode::ACCEPTED, Json(payload)).into_response()
}
