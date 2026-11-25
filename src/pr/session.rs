use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    DiffMap, DiffSide, GithubPrUrl, ImportedPullRequest, PrImportBundle, SeedReviewThread,
};
use crate::state::{FileId, ThreadId};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PrPhase {
    Loading,
    Reviewing,
    Preparing,
    Editing,
    Confirming,
    Publishing,
    Failed,
    Published,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PrReviewAction {
    Approve,
    RequestChanges,
    CommentOnly,
}

impl PrReviewAction {
    pub fn github_event(self) -> &'static str {
        match self {
            Self::Approve => "APPROVE",
            Self::RequestChanges => "REQUEST_CHANGES",
            Self::CommentOnly => "COMMENT",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum PrDraftDestination {
    NewInlineComment {
        path: String,
        line: u32,
        side: DiffSide,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        approximate: bool,
    },
    ExistingReviewThread {
        root_comment_id: u64,
    },
    None {
        reason: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDraftItem {
    pub id: String,
    pub source_thread_id: ThreadId,
    pub include: bool,
    pub text: String,
    pub publishable: bool,
    #[serde(default)]
    pub completed: bool,
    pub destination: PrDraftDestination,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDraft {
    pub draft_id: String,
    pub revision: u64,
    pub action: PrReviewAction,
    pub summary: String,
    pub summary_pending: bool,
    #[serde(default)]
    pub review_completed: bool,
    pub items: Vec<PrDraftItem>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrDisplayMetadata {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub title: String,
    pub state: super::PullRequestState,
    pub author: String,
    pub base_ref: String,
    pub head_ref: String,
    pub head_sha: String,
    pub is_draft: bool,
}

impl From<&ImportedPullRequest> for PrDisplayMetadata {
    fn from(pr: &ImportedPullRequest) -> Self {
        Self {
            owner: pr.owner.clone(),
            repo: pr.repo.clone(),
            number: pr.number,
            url: pr.url.clone(),
            title: pr.title.clone(),
            state: pr.state,
            author: pr.author.login.clone(),
            base_ref: pr.base.ref_name.clone(),
            head_ref: pr.head.ref_name.clone(),
            head_sha: pr.head.sha.clone(),
            is_draft: pr.is_draft,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrFileLink {
    pub key: String,
    pub file_id: FileId,
    pub path: String,
}

/// A changed file explicitly marked as reviewed at one immutable PR head.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrViewedFile {
    pub file_id: FileId,
    pub viewed_at: DateTime<Utc>,
    pub head_sha: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrPublicationError {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrPublishedLink {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub url: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrPublishedReply {
    pub operation_id: String,
    pub root_comment_id: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub url: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PrPublicationStatus {
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrPublicationResult {
    pub request_id: String,
    pub status: PrPublicationStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review: Option<PrPublishedLink>,
    #[serde(default)]
    pub replies: Vec<PrPublishedReply>,
    #[serde(default)]
    pub completed_operations: Vec<String>,
    #[serde(default)]
    pub unknown_operations: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<PrPublicationError>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PrSessionSnapshot {
    pub phase: PrPhase,
    pub url: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub demo: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pr: Option<PrDisplayMetadata>,
    #[serde(default)]
    pub files: Vec<PrFileLink>,
    #[serde(default)]
    pub viewed_files: Vec<PrViewedFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<PrDraft>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publication_result: Option<PrPublicationResult>,
}

#[derive(Clone, Debug)]
pub struct PrFileTarget {
    pub file_id: FileId,
    pub display_path: String,
    pub diff_map: DiffMap,
}

#[derive(Clone, Debug)]
pub struct ImportedReviewTarget {
    pub root_comment_id: u64,
    pub seed: SeedReviewThread,
}

#[derive(Clone, Debug)]
pub struct PendingSummary {
    pub request_id: String,
}

#[derive(Clone, Debug)]
pub struct PendingPublication {
    pub request_id: String,
    pub draft_id: String,
    pub revision: u64,
    pub digest: String,
    pub expects_review: bool,
    pub expected_replies: HashMap<String, u64>,
}

#[derive(Clone, Debug)]
pub struct ConfirmedDraft {
    pub draft_id: String,
    pub revision: u64,
    pub digest: String,
    pub preview_gfm: String,
}

#[derive(Clone)]
pub struct PrReviewState {
    pub identity: GithubPrUrl,
    pub secret: String,
    pub api_base_url: Option<String>,
    pub phase: PrPhase,
    pub imported: Option<PrImportBundle>,
    pub import_id: Option<String>,
    pub import_digest: Option<String>,
    pub file_targets: HashMap<String, PrFileTarget>,
    pub imported_threads: HashMap<ThreadId, ImportedReviewTarget>,
    pub viewed_files: HashMap<FileId, PrViewedFile>,
    pub draft: Option<PrDraft>,
    pub pending_summary: Option<PendingSummary>,
    pub confirmed: Option<ConfirmedDraft>,
    pub pending_publication: Option<PendingPublication>,
    pub publication_result: Option<PrPublicationResult>,
    pub completed_operations: HashSet<String>,
    pub unknown_operations: HashSet<String>,
}

impl std::fmt::Debug for PrReviewState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PrReviewState")
            .field("identity", &self.identity)
            .field("secret", &"[redacted]")
            .field("phase", &self.phase)
            .field("import_id", &self.import_id)
            .field("viewed_files", &self.viewed_files)
            .field("draft", &self.draft)
            .field("pending_summary", &self.pending_summary)
            .field("confirmed", &self.confirmed)
            .field("pending_publication", &self.pending_publication)
            .field("publication_result", &self.publication_result)
            .finish_non_exhaustive()
    }
}

impl PrReviewState {
    pub fn new(identity: GithubPrUrl, secret: String) -> Self {
        Self {
            identity,
            secret,
            api_base_url: None,
            phase: PrPhase::Loading,
            imported: None,
            import_id: None,
            import_digest: None,
            file_targets: HashMap::new(),
            imported_threads: HashMap::new(),
            viewed_files: HashMap::new(),
            draft: None,
            pending_summary: None,
            confirmed: None,
            pending_publication: None,
            publication_result: None,
            completed_operations: HashSet::new(),
            unknown_operations: HashSet::new(),
        }
    }

    pub fn snapshot(&self) -> PrSessionSnapshot {
        let mut files = self
            .file_targets
            .iter()
            .map(|(key, target)| PrFileLink {
                key: key.clone(),
                file_id: target.file_id.clone(),
                path: target.display_path.clone(),
            })
            .collect::<Vec<_>>();
        files.sort_by(|left, right| left.key.cmp(&right.key));
        let mut viewed_files = self.viewed_files.values().cloned().collect::<Vec<_>>();
        viewed_files.sort_by(|left, right| left.file_id.0.cmp(&right.file_id.0));
        PrSessionSnapshot {
            phase: self.phase,
            url: self.identity.canonical_url().to_string(),
            demo: false,
            pr: self.imported.as_ref().map(|bundle| (&bundle.pr).into()),
            files,
            viewed_files,
            draft: self.draft.clone(),
            publication_result: self.publication_result.clone(),
        }
    }
}
