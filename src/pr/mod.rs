//! Isolated core contracts for private-first GitHub pull-request review.

pub mod diff_map;
pub mod instructions;
pub mod session;
pub mod types;
pub mod url;

pub use diff_map::{DiffHunk, DiffLine, DiffMap, DiffRow, DiffSide, DiffStatus, MappedDiffRow};
pub use instructions::{
    GH_AUTH_STATUS_COMMAND, GH_GROUPED_REVIEW_COMMAND, GH_HEAD_RECHECK_COMMAND,
    GH_ISSUE_COMMENTS_COMMAND, GH_PR_CLONE_COMMAND, GH_PR_DIFF_COMMAND, GH_PR_HEAD_FETCH_COMMAND,
    GH_PR_VIEW_COMMAND, GH_REVIEW_COMMENTS_COMMAND, GH_REVIEW_REPLY_COMMAND,
    GH_REVIEW_THREADS_COMMAND, GH_REVIEWS_COMMAND, agent_instructions,
};
pub use session::{
    ConfirmedDraft, ImportedReviewTarget, PendingPublication, PendingSummary, PrDisplayMetadata,
    PrDraft, PrDraftDestination, PrDraftItem, PrFileLink, PrFileTarget, PrPhase,
    PrPublicationError, PrPublicationResult, PrPublicationStatus, PrPublishedLink,
    PrPublishedReply, PrReviewAction, PrReviewState, PrSessionSnapshot,
};
pub use types::{
    DiffContext, DiffContextSource, DiscussionKind, GithubAuthor, GithubRef, ImportedDiscussion,
    ImportedDiscussionComment, ImportedFile, ImportedPullRequest, MAX_DISCUSSION_COMMENTS,
    MAX_DISCUSSIONS, MAX_FILE_DIFF_BYTES, MAX_FILES, MAX_IMPORT_BYTES, MAX_SEED_THREADS,
    MAX_STRING_BYTES, PR_IMPORT_SCHEMA_VERSION, PrImportBundle, PullRequestState, SeedReviewThread,
};
pub use url::GithubPrUrl;

use sha2::{Digest, Sha256};

/// Stable file ID for the generated pull-request overview.
pub const PR_OVERVIEW_FILE_ID: &str = "pr-overview";

/// Derives a stable per-file ID from the full pull-request and path identity.
///
/// Each field is encoded as an eight-byte big-endian length followed by its
/// UTF-8 bytes, preventing boundary ambiguities between adjacent fields.
pub fn file_id(owner: &str, repo: &str, number: u64, old_path: &str, new_path: &str) -> String {
    let mut digest = Sha256::new();
    let number = number.to_string();
    for field in [owner, repo, number.as_str(), old_path, new_path] {
        digest.update((field.len() as u64).to_be_bytes());
        digest.update(field.as_bytes());
    }
    let hash = format!("{:x}", digest.finalize());
    format!("pr-file-{}", &hash[..16])
}

/// Derives the stable local ID for an imported GitHub review thread.
pub fn imported_thread_id(root_comment_id: u64) -> String {
    format!("gh-review-thread-{root_comment_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_ids_are_stable_and_unambiguous() {
        assert_eq!(PR_OVERVIEW_FILE_ID, "pr-overview");
        let first = file_id("acme", "project", 51, "a.rs", "b.rs");
        assert_eq!(first, file_id("acme", "project", 51, "a.rs", "b.rs"));
        assert!(first.starts_with("pr-file-"));
        assert_eq!(first.len(), "pr-file-".len() + 16);
        assert_ne!(first, file_id("acm", "eproject", 51, "a.rs", "b.rs"));
        assert_ne!(first, file_id("acme", "project", 52, "a.rs", "b.rs"));
        assert_ne!(first, file_id("acme", "project", 51, "a.r", "sb.rs"));
        assert_eq!(imported_thread_id(456), "gh-review-thread-456");
    }
}
