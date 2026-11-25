//! Private-first GitHub pull-request import, draft, confirmation, and publication APIs.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::body::Bytes;
use axum::extract::Path;
use axum::extract::State as AxumState;
use axum::extract::rejection::JsonRejection;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::events::{Event, EventKind};
use crate::history;
use crate::pr::{
    ConfirmedDraft, DiffSide, DirectPublication, DirectReply, ImportedReviewTarget,
    MAX_IMPORT_BYTES, PR_OVERVIEW_FILE_ID, PendingPublication, PendingSummary, PrDraft,
    PrDraftDestination, PrDraftItem, PrFileTarget, PrImportBundle, PrPhase, PrPublicationResult,
    PrPublicationStatus, PrPublishedLink, PrPublishedReply, PrReviewAction, PrViewedFile, file_id,
    imported_thread_id,
};
use crate::sse::BroadcastEvent;
use crate::state::{File, FileId, FileKind, LineRange, Source, Thread, ThreadId, ThreadKind};
use crate::transcript::build_transcript_with_source;

use super::app_state::{AppState, PrExecutionMode};
use super::response::api_error_response;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportResponse {
    import_id: String,
    digest: String,
    files: Vec<ImportedFileResponse>,
    seeded_thread_ids: Vec<ThreadId>,
    warnings: Vec<String>,
    idempotent: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportedFileResponse {
    key: String,
    file_id: FileId,
    path: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PrepareRequest {}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct SummaryRequest {
    request_id: String,
    summary: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DraftUpdateRequest {
    draft_id: String,
    revision: u64,
    action: PrReviewAction,
    summary: String,
    items: Vec<DraftItemUpdate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DraftItemUpdate {
    item_id: String,
    include: bool,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DraftIdentityRequest {
    draft_id: String,
    revision: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConfirmResponse {
    draft_id: String,
    revision: u64,
    preview_gfm: String,
    preview_html: String,
    digest: String,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CancelMode {
    Review,
    Edit,
    Confirm,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CancelRequest {
    mode: CancelMode,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PublishRequest {
    draft_id: String,
    revision: u64,
    digest: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PendingResponse {
    request_id: String,
    draft: PrDraft,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PublishResponse {
    request_id: String,
    status: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ViewedClearResponse {
    ok: bool,
    file_id: FileId,
}

pub(super) async fn post_api_pr_file_viewed(
    AxumState(app_state): AxumState<AppState>,
    Path(file_id): Path<String>,
) -> Response {
    let file_id = FileId(file_id);
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    let mut review = match pr_review.write() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while marking a file viewed"),
    };
    if review.phase != PrPhase::Reviewing {
        return phase_conflict("files can only be marked viewed while reviewing");
    }
    if !review
        .file_targets
        .values()
        .any(|target| target.file_id == file_id)
    {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "unknown_pr_file",
            format!("unknown PR diff fileId: {}", file_id.0),
        );
    }
    if let Some(existing) = review.viewed_files.get(&file_id).cloned() {
        return Json(existing).into_response();
    }
    let Some(head_sha) = review
        .imported
        .as_ref()
        .map(|bundle| bundle.pr.head.sha.clone())
    else {
        return phase_conflict("the PR import has not completed");
    };
    let viewed = PrViewedFile {
        file_id: file_id.clone(),
        viewed_at: Utc::now(),
        head_sha,
    };
    review.viewed_files.insert(file_id, viewed.clone());
    drop(review);
    app_state.record_mutation();
    app_state.bus.publish(BroadcastEvent {
        kind: "pr.file.viewed".to_string(),
        payload: serde_json::json!({ "viewedFile": viewed }),
    });
    Json(viewed).into_response()
}

pub(super) async fn delete_api_pr_file_viewed(
    AxumState(app_state): AxumState<AppState>,
    Path(file_id): Path<String>,
) -> Response {
    let file_id = FileId(file_id);
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    let mut review = match pr_review.write() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while clearing viewed state"),
    };
    if review.phase != PrPhase::Reviewing {
        return phase_conflict("viewed state can only be cleared while reviewing");
    }
    if !review
        .file_targets
        .values()
        .any(|target| target.file_id == file_id)
    {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "unknown_pr_file",
            format!("unknown PR diff fileId: {}", file_id.0),
        );
    }
    review.viewed_files.remove(&file_id);
    drop(review);
    app_state.record_mutation();
    app_state.bus.publish(BroadcastEvent {
        kind: "pr.file.unviewed".to_string(),
        payload: serde_json::json!({ "fileId": file_id }),
    });
    Json(ViewedClearResponse { ok: true, file_id }).into_response()
}

pub(super) async fn post_api_pr_import(
    AxumState(app_state): AxumState<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    if let Err(response) = authenticate(&pr_review, &headers) {
        return response;
    }
    if !is_json_content_type(&headers) {
        return api_error_response(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported_media_type",
            "Content-Type must be application/json",
        );
    }
    if body.len() > MAX_IMPORT_BYTES {
        return api_error_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "import_too_large",
            format!("PR import exceeds the {MAX_IMPORT_BYTES}-byte limit"),
        );
    }
    let bundle: PrImportBundle = match serde_json::from_slice(&body) {
        Ok(bundle) => bundle,
        Err(error) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                format!("invalid PR import JSON: {error}"),
            );
        }
    };
    let digest = format!("{:x}", Sha256::digest(&body));

    let mut review = match pr_review.write() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while importing"),
    };
    if let (Some(import_id), Some(existing_digest)) =
        (review.import_id.as_deref(), review.import_digest.as_deref())
    {
        if import_id == bundle.import_id && existing_digest == digest {
            let files = imported_file_responses(&review);
            let mut seeded_thread_ids = review.imported_threads.keys().cloned().collect::<Vec<_>>();
            seeded_thread_ids.sort_by(|left, right| left.0.cmp(&right.0));
            return Json(ImportResponse {
                import_id: import_id.to_string(),
                digest,
                files,
                seeded_thread_ids,
                warnings: Vec::new(),
                idempotent: true,
            })
            .into_response();
        }
        return api_error_response(
            StatusCode::CONFLICT,
            "pr_already_imported",
            "this PR session already contains a different import",
        );
    }
    if let Err(error) = bundle.validate(&review.identity) {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            error.to_string(),
        );
    }

    let mut source_files = vec![File {
        id: FileId(PR_OVERVIEW_FILE_ID.to_string()),
        path: "pr-overview.md".to_string(),
        kind: FileKind::Markdown,
        content: bundle.overview_markdown.clone(),
    }];
    let mut targets = HashMap::new();
    for imported in &bundle.files {
        let map = match crate::pr::DiffMap::parse(&imported.diff) {
            Ok(map) => map,
            Err(error) => {
                return api_error_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    error.to_string(),
                );
            }
        };
        let id = FileId(file_id(
            &bundle.pr.owner,
            &bundle.pr.repo,
            bundle.pr.number,
            &imported.old_path,
            &imported.new_path,
        ));
        let display_path = if imported.status == crate::pr::DiffStatus::Deleted {
            imported.old_path.clone()
        } else {
            imported.new_path.clone()
        };
        source_files.push(File {
            id: id.clone(),
            path: display_path.clone(),
            kind: FileKind::Diff,
            content: imported.diff.clone(),
        });
        targets.insert(
            imported.key.clone(),
            PrFileTarget {
                file_id: id,
                display_path,
                diff_map: map,
            },
        );
    }

    let mut seeded = Vec::new();
    let mut imported_threads = HashMap::new();
    let mut warnings = Vec::new();
    for seed in &bundle.seed_threads {
        if seed.outdated || seed.resolved {
            continue;
        }
        let Some(target) = targets.get(&seed.file_key) else {
            warnings.push(format!(
                "skipped review comment {}: file target is unavailable",
                seed.root_comment_id
            ));
            continue;
        };
        let Some(mapped) = target
            .diff_map
            .hunks
            .iter()
            .flat_map(|hunk| {
                hunk.rows.iter().filter_map(move |row| {
                    row.target
                        .filter(|line| line.side == seed.side && line.line == seed.line)
                        .map(|_| (hunk.index, row.row))
                })
            })
            .next()
        else {
            warnings.push(format!(
                "skipped review comment {}: {} line {} has no exact diff row",
                seed.root_comment_id,
                side_name(seed.side),
                seed.line
            ));
            continue;
        };
        let id = ThreadId(imported_thread_id(seed.root_comment_id));
        seeded.push(Thread {
            id: id.clone(),
            file_id: target.file_id.clone(),
            anchor_start: 3 + mapped.0,
            anchor_end: 3 + mapped.0,
            image_anchor: None,
            snippet: seed.body.clone(),
            breadcrumb: format!("GitHub review by @{} · {}", seed.author.login, seed.url),
            text: seed.body.clone(),
            created_at: seed.created_at,
            kind: ThreadKind::Prepopulated,
            line_range: Some(LineRange {
                start: mapped.1 as u32,
                end: mapped.1 as u32,
            }),
            orphaned: false,
            element_anchor: None,
        });
        imported_threads.insert(
            id,
            ImportedReviewTarget {
                root_comment_id: seed.root_comment_id,
                seed: seed.clone(),
            },
        );
    }

    // Match the source-update lock order (state, then source) so import cannot
    // deadlock with another writer. All fallible validation is complete before
    // either document lock is taken.
    let mut state = match app_state.state.write() {
        Ok(state) => state,
        Err(_) => return internal_error("state lock poisoned while importing PR"),
    };
    let mut source = match app_state.source.write() {
        Ok(source) => source,
        Err(_) => return internal_error("source lock poisoned while importing PR"),
    };
    if seeded.iter().any(|candidate| {
        state
            .all_threads()
            .iter()
            .any(|existing| existing.id == candidate.id)
    }) {
        return api_error_response(
            StatusCode::CONFLICT,
            "thread_id_conflict",
            "an imported review thread ID conflicts with existing local state",
        );
    }
    *source = Source {
        files: source_files,
    };
    let seeded_thread_ids = seeded.iter().map(|thread| thread.id.clone()).collect();
    for thread in seeded {
        state.add_thread(thread);
    }
    state.bump_source_version();

    review.phase = PrPhase::Reviewing;
    review.import_id = Some(bundle.import_id.clone());
    review.import_digest = Some(digest.clone());
    review.file_targets = targets;
    review.imported_threads = imported_threads;
    review.imported = Some(bundle.clone());
    app_state.record_mutation();

    let response_files = imported_file_responses(&review);
    let imported_payload = serde_json::json!({
        "importId": bundle.import_id,
        "overviewFileId": PR_OVERVIEW_FILE_ID,
        "files": response_files,
        "seededThreadIds": seeded_thread_ids,
        "warnings": warnings,
        "agentInstructions": [
            "Use the returned file IDs for all file-scoped thread work.",
            "Diff anchorStart/anchorEnd select a rendered hunk; lineRange selects rows within that hunk fence.",
            "Imported gh-review-thread-* IDs retain their GitHub root-comment reply target. Keep all local work unpublished until the reviewer confirms OK."
        ],
    });
    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::PrImported,
        at: Utc::now(),
        payload: imported_payload.clone(),
    }) {
        tracing::warn!(%error, "failed to emit pr.imported event");
    }
    app_state.bus.publish(BroadcastEvent {
        kind: "pr.imported".to_string(),
        payload: imported_payload,
    });

    Json(ImportResponse {
        import_id: bundle.import_id,
        digest,
        files: response_files,
        seeded_thread_ids,
        warnings,
        idempotent: false,
    })
    .into_response()
}

pub(super) async fn post_api_pr_prepare(
    AxumState(app_state): AxumState<AppState>,
    headers: HeaderMap,
    payload: Result<Json<PrepareRequest>, JsonRejection>,
) -> Response {
    if let Err(response) = json_payload(payload) {
        return response;
    }
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    let mut review = match pr_review.write() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while preparing draft"),
    };
    if review.phase == PrPhase::Preparing
        && let (Some(pending), Some(draft)) = (&review.pending_summary, &review.draft)
    {
        return (
            StatusCode::ACCEPTED,
            Json(PendingResponse {
                request_id: pending.request_id.clone(),
                draft: draft.clone(),
            }),
        )
            .into_response();
    }
    if review.phase == PrPhase::Reviewing
        && review.pending_summary.is_none()
        && let Some(draft) = review.draft.clone()
        && !draft.summary_pending
    {
        review.phase = PrPhase::Editing;
        return Json(draft).into_response();
    }
    if review.phase != PrPhase::Reviewing {
        return phase_conflict("a PR draft can only be prepared while reviewing");
    }
    let Some(bundle) = review.imported.as_ref() else {
        return phase_conflict("the PR import has not completed");
    };
    let identity = serde_json::json!({
        "owner": bundle.pr.owner,
        "repo": bundle.pr.repo,
        "number": bundle.pr.number,
        "url": bundle.pr.url,
        "headSha": bundle.pr.head.sha,
    });
    if !review.unknown_operations.is_empty() {
        return api_error_response(
            StatusCode::CONFLICT,
            "unknown_publication_outcome",
            "publication retry is blocked because one or more operations have an unknown outcome",
        );
    }

    let state = match app_state.state.read() {
        Ok(state) => state,
        Err(_) => return internal_error("state lock poisoned while preparing PR draft"),
    };
    let mut items = Vec::new();
    let mut conversations = Vec::new();
    for thread in state.get_threads() {
        let replies = state.replies_for_thread(&thread.id);
        let takes = state.takes_for_thread(&thread.id);
        conversations.push(serde_json::json!({
            "thread": thread,
            "replies": replies,
            "takes": takes,
        }));
        if thread.kind == ThreadKind::Prepopulated {
            if replies.is_empty() && takes.is_empty() {
                continue;
            }
            let Some(target) = review.imported_threads.get(&thread.id) else {
                continue;
            };
            items.push(PrDraftItem {
                id: format!("reply:{}", thread.id.0),
                source_thread_id: thread.id.clone(),
                include: false,
                text: latest_response_text(&replies, &takes).unwrap_or_default(),
                publishable: true,
                completed: false,
                destination: PrDraftDestination::ExistingReviewThread {
                    root_comment_id: target.root_comment_id,
                },
            });
            continue;
        }
        let (destination, publishable) = local_destination(&review, &thread);
        let initial_text = latest_local_text(&thread, &replies, &takes);
        let text = match &destination {
            PrDraftDestination::NewInlineComment {
                path,
                line,
                approximate: true,
                ..
            } => {
                let path = path.replace('`', "\\`");
                let snippet = thread
                    .snippet
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                format!(
                    "> **Approximate location:** `{path}`, near original diff row {} (resolved to line {line}). Context: {}\n\n{initial_text}",
                    thread
                        .line_range
                        .map(|range| range.start)
                        .unwrap_or_default(),
                    snippet.chars().take(240).collect::<String>()
                )
            }
            _ => initial_text,
        };
        items.push(PrDraftItem {
            id: format!("comment:{}", thread.id.0),
            source_thread_id: thread.id,
            include: false,
            text,
            publishable,
            completed: false,
            destination,
        });
    }
    drop(state);

    let mut draft = PrDraft {
        draft_id: random_id("pr-draft"),
        revision: 1,
        action: PrReviewAction::CommentOnly,
        summary: String::new(),
        summary_pending: true,
        review_completed: false,
        items,
    };
    review.confirmed = None;
    review.pending_publication = None;
    review.publication_result = None;

    if app_state.pr_execution_mode() == PrExecutionMode::DemoLocal {
        draft.summary = super::demo::DEMO_PR_SUMMARY.to_string();
        draft.summary_pending = false;
        draft.revision += 1;
        review.phase = PrPhase::Editing;
        review.draft = Some(draft.clone());
        review.pending_summary = None;
        drop(review);
        app_state.record_mutation();
        app_state.bus.publish(BroadcastEvent {
            kind: "pr.draft.ready".to_string(),
            payload: serde_json::json!({ "draft": draft }),
        });
        return Json(draft).into_response();
    }

    let request_id = random_id("pr-summary");
    review.phase = PrPhase::Preparing;
    review.draft = Some(draft.clone());
    review.pending_summary = Some(PendingSummary {
        request_id: request_id.clone(),
    });
    let summary_url = callback_url(review.api_base_url.as_deref(), &headers, "/api/pr/summary");
    drop(review);

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::PrSummaryRequested,
        at: Utc::now(),
        payload: serde_json::json!({
            "requestId": request_id,
            "summaryUrl": summary_url,
            "pr": identity,
            "conversations": conversations,
        }),
    }) {
        return internal_error(format!("failed to emit pr.summary.requested: {error}"));
    }
    app_state.record_mutation();
    (
        StatusCode::ACCEPTED,
        Json(PendingResponse { request_id, draft }),
    )
        .into_response()
}

pub(super) async fn post_api_pr_summary(
    AxumState(app_state): AxumState<AppState>,
    headers: HeaderMap,
    payload: Result<Json<SummaryRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match json_payload(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    if let Err(response) = authenticate(&pr_review, &headers) {
        return response;
    }
    let mut review = match pr_review.write() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while storing summary"),
    };
    let Some(pending) = review.pending_summary.as_ref() else {
        return phase_conflict("no PR summary request is pending");
    };
    if pending.request_id != request.request_id {
        return api_error_response(
            StatusCode::CONFLICT,
            "stale_request",
            "summary requestId does not match the pending request",
        );
    }
    let Some(draft) = review.draft.as_mut() else {
        return internal_error("pending PR summary has no draft");
    };
    draft.summary = request.summary;
    draft.summary_pending = false;
    draft.revision += 1;
    let draft = draft.clone();
    review.pending_summary = None;
    review.phase = PrPhase::Editing;
    drop(review);
    app_state.record_mutation();
    app_state.bus.publish(BroadcastEvent {
        kind: "pr.draft.ready".to_string(),
        payload: serde_json::json!({ "draft": draft }),
    });
    Json(draft).into_response()
}

pub(super) async fn get_api_pr_draft(AxumState(app_state): AxumState<AppState>) -> Response {
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    let review = match pr_review.read() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while reading draft"),
    };
    match review.draft.clone() {
        Some(draft) => Json(draft).into_response(),
        None => api_error_response(
            StatusCode::NOT_FOUND,
            "draft_not_found",
            "no PR draft exists",
        ),
    }
}

pub(super) async fn post_api_pr_draft(
    AxumState(app_state): AxumState<AppState>,
    payload: Result<Json<DraftUpdateRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match json_payload(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    let mut review = match pr_review.write() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while updating draft"),
    };
    if !matches!(review.phase, PrPhase::Editing | PrPhase::Failed) {
        return phase_conflict("the PR draft is not editable in its current phase");
    }
    let Some(draft) = review.draft.as_mut() else {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "draft_not_found",
            "no PR draft exists",
        );
    };
    if draft.draft_id != request.draft_id || draft.revision != request.revision {
        return stale_draft(draft);
    }
    let updates = request
        .items
        .into_iter()
        .map(|item| (item.item_id, (item.include, item.text)))
        .collect::<HashMap<_, _>>();
    if updates.len() != draft.items.len()
        || draft
            .items
            .iter()
            .any(|item| !updates.contains_key(&item.id))
    {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "draft update must contain each existing item exactly once",
        );
    }
    if draft.review_completed
        && (request.action != draft.action || request.summary != draft.summary)
    {
        return api_error_response(
            StatusCode::CONFLICT,
            "published_content_immutable",
            "the review action and summary are immutable because the grouped review was already published",
        );
    }
    for item in &mut draft.items {
        let (include, text) = updates.get(&item.id).expect("all item IDs checked");
        if item.completed && (*include != item.include || text != &item.text) {
            return api_error_response(
                StatusCode::CONFLICT,
                "published_content_immutable",
                format!(
                    "draft item {} is immutable because it was already published",
                    item.id
                ),
            );
        }
        if *include && !item.publishable {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "unpublishable_item",
                format!(
                    "draft item {} has no valid publication destination",
                    item.id
                ),
            );
        }
        if *include && text.trim().is_empty() {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                format!("included draft item {} must not be blank", item.id),
            );
        }
        item.include = *include;
        item.text = text.clone();
    }
    draft.action = request.action;
    draft.summary = request.summary;
    draft.revision += 1;
    let draft = draft.clone();
    review.phase = PrPhase::Editing;
    review.confirmed = None;
    review.pending_publication = None;
    review.publication_result = None;
    drop(review);
    app_state.record_mutation();
    Json(draft).into_response()
}

pub(super) async fn post_api_pr_confirm(
    AxumState(app_state): AxumState<AppState>,
    payload: Result<Json<DraftIdentityRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match json_payload(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    let mut review = match pr_review.write() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while confirming draft"),
    };
    if review.phase != PrPhase::Editing {
        return phase_conflict("the PR draft must be in editing phase before confirmation");
    }
    let Some(draft) = review.draft.as_ref() else {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "draft_not_found",
            "no PR draft exists",
        );
    };
    if draft.draft_id != request.draft_id || draft.revision != request.revision {
        return stale_draft(draft);
    }
    let has_pending_item = draft
        .items
        .iter()
        .any(|item| item.include && !item.completed);
    if (draft.review_completed || draft.summary.trim().is_empty()) && !has_pending_item {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "empty_review",
            "select at least one unpublished item or enter a review summary",
        );
    }
    if !draft.review_completed
        && draft.summary.trim().is_empty()
        && matches!(
            draft.action,
            PrReviewAction::CommentOnly | PrReviewAction::RequestChanges
        )
    {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "summary_required",
            "Comment only and Request changes reviews require a nonempty review summary",
        );
    }
    let preview_gfm = preview_gfm(draft);
    let digest = format!("{:x}", Sha256::digest(preview_gfm.as_bytes()));
    let response = ConfirmResponse {
        draft_id: draft.draft_id.clone(),
        revision: draft.revision,
        preview_html: crate::render::render(&preview_gfm),
        preview_gfm: preview_gfm.clone(),
        digest: digest.clone(),
    };
    review.confirmed = Some(ConfirmedDraft {
        draft_id: response.draft_id.clone(),
        revision: response.revision,
        digest,
        preview_gfm,
    });
    review.phase = PrPhase::Confirming;
    drop(review);
    app_state.record_mutation();
    Json(response).into_response()
}

pub(super) async fn post_api_pr_cancel(
    AxumState(app_state): AxumState<AppState>,
    payload: Result<Json<CancelRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match json_payload(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    let mut review = match pr_review.write() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while cancelling"),
    };
    match (review.phase, request.mode) {
        (PrPhase::Confirming, CancelMode::Confirm | CancelMode::Edit)
        | (PrPhase::Failed, CancelMode::Confirm | CancelMode::Edit) => {
            review.phase = PrPhase::Editing;
            review.confirmed = None;
            review.pending_publication = None;
        }
        (PrPhase::Editing | PrPhase::Preparing, CancelMode::Review | CancelMode::Edit)
        | (PrPhase::Confirming | PrPhase::Failed, CancelMode::Review) => {
            review.phase = PrPhase::Reviewing;
            review.pending_summary = None;
            review.pending_publication = None;
            review.confirmed = None;
        }
        _ => return phase_conflict("cancel mode does not match the current PR phase"),
    }
    let snapshot = review.snapshot();
    drop(review);
    app_state.record_mutation();
    Json(snapshot).into_response()
}

pub(super) async fn post_api_pr_publish(
    AxumState(app_state): AxumState<AppState>,
    headers: HeaderMap,
    payload: Result<Json<PublishRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match json_payload(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    let mut review = match pr_review.write() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while publishing"),
    };
    if review.phase == PrPhase::Publishing
        && let Some(pending) = &review.pending_publication
    {
        if pending.draft_id == request.draft_id
            && pending.revision == request.revision
            && pending.digest == request.digest
        {
            return Json(PublishResponse {
                request_id: pending.request_id.clone(),
                status: "publishing",
            })
            .into_response();
        }
        return phase_conflict("a different PR publication request is already pending");
    }
    if !matches!(review.phase, PrPhase::Confirming | PrPhase::Failed) {
        return phase_conflict("the exact PR preview must be confirmed before publication");
    }
    if !review.unknown_operations.is_empty() {
        return api_error_response(
            StatusCode::CONFLICT,
            "unknown_publication_outcome",
            "retry is blocked because one or more publication operations have an unknown outcome",
        );
    }
    let Some(confirmed) = review.confirmed.as_ref() else {
        return phase_conflict("the exact PR preview has not been confirmed");
    };
    if confirmed.draft_id != request.draft_id
        || confirmed.revision != request.revision
        || !constant_time_eq(confirmed.digest.as_bytes(), request.digest.as_bytes())
    {
        return api_error_response(
            StatusCode::CONFLICT,
            "stale_confirmation",
            "draft, revision, or preview digest does not match the confirmed preview",
        );
    }
    let Some(draft) = review.draft.as_ref() else {
        return api_error_response(
            StatusCode::NOT_FOUND,
            "draft_not_found",
            "no PR draft exists",
        );
    };
    let has_pending_item = draft
        .items
        .iter()
        .any(|item| item.include && !item.completed);
    if (draft.review_completed || draft.summary.trim().is_empty()) && !has_pending_item {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "empty_review",
            "select at least one unpublished item or enter a review summary",
        );
    }
    if !draft.review_completed
        && draft.summary.trim().is_empty()
        && matches!(
            draft.action,
            PrReviewAction::CommentOnly | PrReviewAction::RequestChanges
        )
    {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "summary_required",
            "Comment only and Request changes reviews require a nonempty review summary",
        );
    }
    let Some(bundle) = review.imported.as_ref() else {
        return phase_conflict("the PR import has not completed");
    };
    let completed = review.completed_operations.clone();
    let mut comments = Vec::new();
    let mut comment_operations = Vec::new();
    let mut replies = Vec::new();
    let mut direct_replies = Vec::new();
    let mut expected_replies = HashMap::new();
    for item in draft.items.iter().filter(|item| item.include) {
        match &item.destination {
            PrDraftDestination::NewInlineComment {
                path, line, side, ..
            } if !completed.contains(&item.id) && !completed.contains("review") => {
                comment_operations.push(serde_json::json!({
                    "operationId": item.id,
                    "commentIndex": comments.len(),
                }));
                comments.push(serde_json::json!({
                    "path": path,
                    "line": line,
                    "side": side,
                    "body": item.text,
                }));
            }
            PrDraftDestination::ExistingReviewThread { root_comment_id }
                if !completed.contains(&item.id) =>
            {
                expected_replies.insert(item.id.clone(), *root_comment_id);
                let github_request = serde_json::json!({ "body": item.text });
                replies.push(serde_json::json!({
                    "operationId": item.id,
                    "rootCommentId": root_comment_id,
                    "githubRequest": github_request,
                }));
                direct_replies.push(DirectReply {
                    operation_id: item.id.clone(),
                    root_comment_id: *root_comment_id,
                    request: github_request,
                });
            }
            _ => {}
        }
    }
    let expects_review = grouped_review_required(
        draft.action,
        &draft.summary,
        comments.len(),
        completed.contains("review"),
    );
    let request_id = random_id("pr-publish");
    let review_request = expects_review.then(|| {
        serde_json::json!({
            "event": draft.action.github_event(),
            "body": draft.summary,
            "comments": comments,
        })
    });
    let direct_publication = DirectPublication {
        request_id: request_id.clone(),
        owner: bundle.pr.owner.clone(),
        repo: bundle.pr.repo.clone(),
        number: bundle.pr.number,
        head_sha: bundle.pr.head.sha.clone(),
        review: review_request.clone(),
        replies: direct_replies,
    };
    let execution_mode = app_state.pr_execution_mode();
    let demo_result = (execution_mode == PrExecutionMode::DemoLocal).then(|| {
        let mut reply_targets = expected_replies.iter().collect::<Vec<_>>();
        reply_targets.sort_by(|left, right| left.0.cmp(right.0));
        let mut completed_operations = reply_targets
            .iter()
            .map(|(operation_id, _)| (*operation_id).clone())
            .collect::<Vec<_>>();
        if expects_review {
            completed_operations.push("review".to_string());
        }
        PrPublicationResult {
            request_id: request_id.clone(),
            status: PrPublicationStatus::Succeeded,
            review: expects_review.then(|| PrPublishedLink {
                id: Some(9_000_056),
                url: "https://github.com/demo-only/ledgerly/pull/56#pullrequestreview-9000056"
                    .to_string(),
            }),
            replies: reply_targets
                .into_iter()
                .enumerate()
                .map(
                    |(index, (operation_id, root_comment_id))| PrPublishedReply {
                        operation_id: operation_id.clone(),
                        root_comment_id: *root_comment_id,
                        id: Some(9_100_000 + index as u64),
                        url: format!(
                            "https://github.com/demo-only/ledgerly/pull/56#discussion_r{}",
                            9_100_000 + index as u64
                        ),
                    },
                )
                .collect(),
            completed_operations,
            unknown_operations: Vec::new(),
            error: None,
        }
    });
    let result_url = callback_url(
        review.api_base_url.as_deref(),
        &headers,
        "/api/pr/publication-result",
    );
    let payload = serde_json::json!({
        "requestId": request_id,
        "pr": {
            "owner": bundle.pr.owner,
            "repo": bundle.pr.repo,
            "number": bundle.pr.number,
            "url": bundle.pr.url,
        },
        "headSha": bundle.pr.head.sha,
        "review": review_request.map(|github_request| serde_json::json!({
            "operationId": "review",
            "githubRequest": github_request,
            "commentOperations": comment_operations,
        })),
        "replies": replies,
        "resultUrl": result_url,
    });
    review.phase = PrPhase::Publishing;
    review.pending_publication = Some(PendingPublication {
        request_id: request_id.clone(),
        draft_id: request.draft_id,
        revision: request.revision,
        digest: request.digest,
        expects_review,
        expected_replies,
    });
    // Keep any links from completed operations on a prior failed attempt so a
    // later retry can present one complete submitted result.
    let secret = review.secret.clone();
    drop(review);

    match execution_mode {
        PrExecutionMode::DirectGh => {
            tokio::spawn(publish_directly(direct_publication, result_url, secret));
        }
        PrExecutionMode::DemoLocal => {
            let result = demo_result.expect("demo execution constructs a local result");
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(450)).await;
                post_local_publication_result(result, result_url, secret).await;
            });
        }
        PrExecutionMode::AgentEvent => {
            if let Err(error) = app_state.emitter.emit(&Event {
                kind: EventKind::PrPublishRequested,
                at: Utc::now(),
                payload,
            }) {
                return internal_error(format!("failed to emit pr.publish.requested: {error}"));
            }
        }
    }
    app_state.record_mutation();
    Json(PublishResponse {
        request_id,
        status: if execution_mode == PrExecutionMode::DemoLocal {
            "simulating"
        } else {
            "publishing"
        },
    })
    .into_response()
}

async fn publish_directly(publication: DirectPublication, result_url: String, secret: String) {
    let result = crate::pr::publish(publication).await;
    post_local_publication_result(result, result_url, secret).await;
}

async fn post_local_publication_result(
    result: PrPublicationResult,
    result_url: String,
    secret: String,
) {
    let body = match serde_json::to_vec(&result) {
        Ok(body) => body,
        Err(error) => {
            tracing::error!(%error, "could not encode direct PR publication result");
            return;
        }
    };
    let client = match reqwest::Client::builder().no_proxy().build() {
        Ok(client) => client,
        Err(error) => {
            tracing::error!(%error, "could not create direct PR publication callback client");
            return;
        }
    };
    let response = client
        .post(result_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .bearer_auth(secret)
        .body(body)
        .send()
        .await;
    match response {
        Ok(response) if response.status().is_success() => {}
        Ok(response) => {
            tracing::error!(status = %response.status(), "direct PR publication callback was rejected");
        }
        Err(error) => {
            tracing::error!(%error, "direct PR publication callback failed");
        }
    }
}

pub(super) async fn post_api_pr_publication_result(
    AxumState(app_state): AxumState<AppState>,
    headers: HeaderMap,
    payload: Result<Json<PrPublicationResult>, JsonRejection>,
) -> Response {
    let Json(result) = match json_payload(payload) {
        Ok(payload) => payload,
        Err(response) => return response,
    };
    let pr_review = match require_pr(&app_state) {
        Ok(state) => state,
        Err(response) => return response,
    };
    if let Err(response) = authenticate(&pr_review, &headers) {
        return response;
    }
    let mut review = match pr_review.write() {
        Ok(review) => review,
        Err(_) => return internal_error("PR state lock poisoned while storing publication result"),
    };
    let Some(pending) = review.pending_publication.as_ref() else {
        return phase_conflict("no PR publication request is pending");
    };
    if pending.request_id != result.request_id {
        return api_error_response(
            StatusCode::CONFLICT,
            "stale_request",
            "publication requestId does not match the pending request",
        );
    }
    let expects_review = pending.expects_review;
    let expected_replies = pending.expected_replies.clone();
    let mut expected_operations = expected_replies.keys().cloned().collect::<HashSet<_>>();
    if expects_review {
        expected_operations.insert("review".to_string());
    }
    let completed = result
        .completed_operations
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    let unknown = result
        .unknown_operations
        .iter()
        .cloned()
        .collect::<HashSet<_>>();
    if completed.len() != result.completed_operations.len()
        || unknown.len() != result.unknown_operations.len()
        || !completed.is_disjoint(&unknown)
        || !completed.is_subset(&expected_operations)
        || !unknown.is_subset(&expected_operations)
    {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "publication operation IDs must be unique, expected, and cannot be both completed and unknown",
        );
    }
    if result.review.as_ref().is_some_and(|review| {
        review.id.is_none_or(|id| id == 0) || !is_github_result_url(&review.url)
    }) {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "published review results require a positive GitHub id and github.com URL",
        );
    }
    let mut returned_replies = HashSet::new();
    for reply in &result.replies {
        if !returned_replies.insert(reply.operation_id.clone())
            || expected_replies.get(&reply.operation_id) != Some(&reply.root_comment_id)
            || reply.id.is_none_or(|id| id == 0)
            || !is_github_result_url(&reply.url)
        {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                "publication result contains an invalid, unexpected, or duplicate review-thread reply",
            );
        }
    }
    match result.status {
        PrPublicationStatus::Succeeded => {
            if result.error.is_some()
                || !unknown.is_empty()
                || completed != expected_operations
                || result.review.is_some() != expects_review
                || returned_replies.len() != expected_replies.len()
                || !expected_replies
                    .keys()
                    .all(|operation_id| returned_replies.contains(operation_id))
            {
                return api_error_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    "a succeeded publication result must account for every requested operation exactly once",
                );
            }
        }
        PrPublicationStatus::Failed => {
            let completed_reply_operations = completed
                .iter()
                .filter(|operation_id| expected_replies.contains_key(*operation_id))
                .cloned()
                .collect::<HashSet<_>>();
            if result.error.is_none()
                || (unknown.is_empty() && completed == expected_operations)
                || result.review.is_some() != completed.contains("review")
                || (result.review.is_some() && !expects_review)
                || returned_replies != completed_reply_operations
            {
                return api_error_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    "a failed publication result must include an error and mark every returned GitHub object completed",
                );
            }
        }
    }
    let made_progress = !completed.is_empty();
    review
        .completed_operations
        .extend(completed.iter().cloned());
    for operation_id in &expected_operations {
        review.unknown_operations.remove(operation_id);
    }
    review.unknown_operations.extend(unknown);
    if made_progress && let Some(draft) = review.draft.as_mut() {
        if completed.contains("review") {
            draft.review_completed = true;
            for item in &mut draft.items {
                if matches!(
                    item.destination,
                    PrDraftDestination::NewInlineComment { .. }
                ) {
                    item.completed = true;
                }
            }
        }
        for item in &mut draft.items {
            if completed.contains(&item.id) {
                item.completed = true;
            }
        }
    }
    let mut result = result;
    if let Some(previous) = review.publication_result.as_ref() {
        if result.review.is_none() {
            result.review = previous.review.clone();
        }
        let mut seen = result
            .replies
            .iter()
            .map(|reply| reply.operation_id.clone())
            .collect::<HashSet<_>>();
        for reply in &previous.replies {
            if seen.insert(reply.operation_id.clone()) {
                result.replies.push(reply.clone());
            }
        }
    }
    result.completed_operations = review.completed_operations.iter().cloned().collect();
    result.completed_operations.sort();
    result.unknown_operations = review.unknown_operations.iter().cloned().collect();
    result.unknown_operations.sort();
    review.publication_result = Some(result.clone());

    if result.status == PrPublicationStatus::Failed {
        review.phase = PrPhase::Failed;
        if review.unknown_operations.is_empty() {
            review.pending_publication = None;
        }
        let draft = review.draft.clone();
        drop(review);
        app_state.record_mutation();
        app_state.bus.publish(BroadcastEvent {
            kind: "pr.publication.failed".to_string(),
            payload: serde_json::json!({
                "result": result,
                "draft": draft,
            }),
        });
        return Json(result).into_response();
    }

    // Build a terminal snapshot for the transcript without committing the
    // browser-visible terminal phase until stdout emission and archiving have
    // completed. Keeping the pending request allows an identical callback to
    // resume finalization if those local steps fail.
    review.phase = PrPhase::Published;
    let mut pr_snapshot = review.snapshot();
    pr_snapshot.demo = app_state.pr_execution_mode() == PrExecutionMode::DemoLocal;
    review.phase = PrPhase::Publishing;
    drop(review);
    let source = match app_state.current_source() {
        Ok(source) => source,
        Err(message) => return internal_error(message),
    };
    let finalization = format!("publication:{}", result.request_id);
    if !app_state.claim_done(&finalization) {
        return phase_conflict("another finalization is already pending or complete");
    }
    let transcript = match app_state.state.read() {
        Ok(state) => build_transcript_with_source(&state, &source).with_pr_session(pr_snapshot),
        Err(_) => {
            app_state.fail_done(&finalization);
            return internal_error("state lock poisoned while building PR transcript");
        }
    };
    let event_at = Utc::now();
    let event_payload = match serde_json::to_value(transcript) {
        Ok(payload) => payload,
        Err(error) => {
            app_state.fail_done(&finalization);
            return internal_error(format!("failed to serialize PR transcript: {error}"));
        }
    };
    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::SessionDone,
        at: event_at,
        payload: event_payload.clone(),
    }) {
        app_state.fail_done(&finalization);
        return internal_error(format!("failed to emit session.done: {error}"));
    }
    if !app_state.no_save() {
        let path = history::history_archive_path(
            app_state.history_dir.as_ref().as_path(),
            app_state.source_path.as_ref().as_deref(),
            app_state.files_count(),
            event_at,
        );
        if let Err(error) = history::write_history_archive(&path, &event_payload) {
            tracing::warn!(path = %path.display(), %error, "failed to write PR history archive");
        }
    }
    if let Some(pr_review) = &app_state.pr_review {
        let mut review = match pr_review.write() {
            Ok(review) => review,
            Err(_) => return internal_error("PR state lock poisoned while finalizing publication"),
        };
        let pending_matches = review
            .pending_publication
            .as_ref()
            .is_some_and(|pending| pending.request_id == result.request_id);
        if !pending_matches {
            return phase_conflict("PR publication finalization request changed unexpectedly");
        }
        review.phase = PrPhase::Published;
        review.pending_publication = None;
    }
    app_state.complete_done(&finalization);
    app_state.record_mutation();
    app_state.bus.publish(BroadcastEvent {
        kind: "pr.publication.succeeded".to_string(),
        payload: serde_json::json!({ "result": result }),
    });
    let shutdown_state = app_state.clone();
    tokio::spawn(async move {
        // Give the browser time to receive the terminal SSE and render the
        // submitted links before the loopback listener disappears. Reconnect
        // also reconciles /api/state during this window.
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        shutdown_state.signal_shutdown();
    });
    Json(result).into_response()
}

fn local_destination(
    review: &crate::pr::PrReviewState,
    thread: &Thread,
) -> (PrDraftDestination, bool) {
    if thread.orphaned {
        return (
            PrDraftDestination::None {
                reason: "thread anchor is orphaned".to_string(),
            },
            false,
        );
    }
    let Some((_, target)) = review
        .file_targets
        .iter()
        .find(|(_, target)| target.file_id == thread.file_id)
    else {
        return (
            PrDraftDestination::None {
                reason: "thread is not attached to an imported diff file".to_string(),
            },
            false,
        );
    };
    if target.diff_map.binary {
        return (
            PrDraftDestination::None {
                reason: "binary diffs cannot receive inline review comments".to_string(),
            },
            false,
        );
    }
    let Some(line_range) = thread.line_range else {
        return (
            PrDraftDestination::None {
                reason: "thread has no diff line range".to_string(),
            },
            false,
        );
    };
    let Some(hunk_index) = thread.anchor_start.checked_sub(3) else {
        return (
            PrDraftDestination::None {
                reason: "thread is not anchored to a diff hunk".to_string(),
            },
            false,
        );
    };
    let Some(mapped) = target
        .diff_map
        .nearest_valid_row(hunk_index, line_range.start as usize)
    else {
        return (
            PrDraftDestination::None {
                reason: "diff hunk has no publishable line".to_string(),
            },
            false,
        );
    };
    (
        PrDraftDestination::NewInlineComment {
            path: target.display_path.clone(),
            line: mapped.line,
            side: mapped.side,
            approximate: mapped.approximate,
        },
        true,
    )
}

fn grouped_review_required(
    action: PrReviewAction,
    summary: &str,
    inline_comment_count: usize,
    already_completed: bool,
) -> bool {
    !already_completed
        && (!summary.trim().is_empty()
            || inline_comment_count > 0
            || action != PrReviewAction::CommentOnly)
}

fn latest_local_text(
    thread: &Thread,
    replies: &[crate::state::Reply],
    takes: &[crate::state::Take],
) -> String {
    let mut latest = (!thread.text.is_empty()).then(|| (thread.created_at, thread.text.clone()));
    for reply in replies {
        if latest
            .as_ref()
            .is_none_or(|(at, _)| reply.created_at >= *at)
        {
            latest = Some((reply.created_at, reply.text.clone()));
        }
    }
    for take in takes {
        if latest.as_ref().is_none_or(|(at, _)| take.created_at >= *at) {
            latest = Some((take.created_at, take.text.clone()));
        }
    }
    latest.map(|(_, text)| text).unwrap_or_default()
}

fn latest_response_text(
    replies: &[crate::state::Reply],
    takes: &[crate::state::Take],
) -> Option<String> {
    let mut latest = None;
    for reply in replies {
        if latest
            .as_ref()
            .is_none_or(|(at, _): &(chrono::DateTime<Utc>, String)| reply.created_at >= *at)
        {
            latest = Some((reply.created_at, reply.text.clone()));
        }
    }
    for take in takes {
        if latest
            .as_ref()
            .is_none_or(|(at, _): &(chrono::DateTime<Utc>, String)| take.created_at >= *at)
        {
            latest = Some((take.created_at, take.text.clone()));
        }
    }
    latest.map(|(_, text)| text)
}

fn preview_gfm(draft: &PrDraft) -> String {
    let mut preview = if draft.review_completed {
        "Grouped review: already published; this retry contains only the remaining thread replies."
            .to_string()
    } else {
        format!(
            "Action: {}\n\nReview body:\n{}",
            draft.action.github_event(),
            draft.summary
        )
    };
    for item in draft
        .items
        .iter()
        .filter(|item| item.include && !item.completed)
    {
        match &item.destination {
            PrDraftDestination::NewInlineComment {
                path, line, side, ..
            } => preview.push_str(&format!(
                "\n\nNew inline comment — {path}:{line} ({}):\n{}",
                side_name(*side),
                item.text
            )),
            PrDraftDestination::ExistingReviewThread { root_comment_id } => {
                preview.push_str(&format!(
                    "\n\nReply to review thread {root_comment_id}:\n{}",
                    item.text
                ))
            }
            PrDraftDestination::None { .. } => {}
        }
    }
    preview
}

fn imported_file_responses(review: &crate::pr::PrReviewState) -> Vec<ImportedFileResponse> {
    let mut files = review
        .file_targets
        .iter()
        .map(|(key, target)| ImportedFileResponse {
            key: key.clone(),
            file_id: target.file_id.clone(),
            path: target.display_path.clone(),
        })
        .collect::<Vec<_>>();
    files.sort_by(|left, right| left.key.cmp(&right.key));
    files
}

#[allow(clippy::result_large_err)]
fn require_pr(
    app_state: &AppState,
) -> Result<std::sync::Arc<std::sync::RwLock<crate::pr::PrReviewState>>, Response> {
    app_state.pr_review.clone().ok_or_else(|| {
        api_error_response(
            StatusCode::NOT_FOUND,
            "not_pr_session",
            "this endpoint is only available in PR sessions",
        )
    })
}

#[allow(clippy::result_large_err)]
fn authenticate(
    state: &std::sync::RwLock<crate::pr::PrReviewState>,
    headers: &HeaderMap,
) -> Result<(), Response> {
    let supplied = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default();
    let expected = state
        .read()
        .map_err(|_| internal_error("PR state lock poisoned while authenticating"))?
        .secret
        .clone();
    if constant_time_eq(supplied.as_bytes(), expected.as_bytes()) {
        Ok(())
    } else {
        Err(api_error_response(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid PR session bearer token is required",
        ))
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let max = left.len().max(right.len());
    let mut difference = left.len() ^ right.len();
    for index in 0..max {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn is_json_content_type(headers: &HeaderMap) -> bool {
    headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value.split(';').next().is_some_and(|mime| {
                let mime = mime.trim().to_ascii_lowercase();
                mime == "application/json"
                    || (mime.starts_with("application/") && mime.ends_with("+json"))
            })
        })
}

#[allow(clippy::result_large_err)]
fn json_payload<T>(payload: Result<Json<T>, JsonRejection>) -> Result<Json<T>, Response> {
    payload.map_err(|rejection| {
        api_error_response(rejection.status(), "bad_request", rejection.body_text())
    })
}

fn callback_url(base_url: Option<&str>, headers: &HeaderMap, path: &str) -> String {
    if let Some(base_url) = base_url {
        return format!("{base_url}{path}");
    }
    // Embedding tests can construct PR AppState without a readiness callback;
    // production PR sessions always set `api_base_url` from the bound listener.
    let host = headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("127.0.0.1");
    format!("http://{host}{path}")
}

fn random_id(prefix: &str) -> String {
    let bytes = rand::random::<[u8; 16]>();
    let mut value = String::with_capacity(prefix.len() + 1 + bytes.len() * 2);
    value.push_str(prefix);
    value.push('-');
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn is_github_result_url(value: &str) -> bool {
    url::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
    })
}

fn side_name(side: DiffSide) -> &'static str {
    match side {
        DiffSide::Left => "LEFT",
        DiffSide::Right => "RIGHT",
    }
}

fn stale_draft(draft: &PrDraft) -> Response {
    api_error_response(
        StatusCode::CONFLICT,
        "stale_draft",
        format!(
            "draftId/revision is stale; current draft is {} revision {}",
            draft.draft_id, draft.revision
        ),
    )
}

fn phase_conflict(message: impl Into<String>) -> Response {
    api_error_response(StatusCode::CONFLICT, "invalid_pr_phase", message)
}

fn internal_error(message: impl Into<String>) -> Response {
    api_error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reply_only_comment_review_does_not_create_standalone_review_body() {
        assert!(!grouped_review_required(
            PrReviewAction::CommentOnly,
            "",
            0,
            false
        ));
        assert!(grouped_review_required(
            PrReviewAction::CommentOnly,
            "summary",
            0,
            false
        ));
        assert!(grouped_review_required(
            PrReviewAction::CommentOnly,
            "",
            1,
            false
        ));
        assert!(grouped_review_required(
            PrReviewAction::Approve,
            "",
            0,
            false
        ));
        assert!(!grouped_review_required(
            PrReviewAction::Approve,
            "summary",
            1,
            true
        ));
    }
}
