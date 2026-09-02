use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};

use super::diff_map::{DiffMap, DiffSide, DiffStatus};
use super::url::GithubPrUrl;
use crate::{DiscussError, Result};

/// Current PR import wire-schema version.
pub const PR_IMPORT_SCHEMA_VERSION: u32 = 1;
/// Maximum serialized size of one import bundle.
pub const MAX_IMPORT_BYTES: usize = 50 * 1024 * 1024;
/// Maximum size of a normal imported string.
pub const MAX_STRING_BYTES: usize = 2 * 1024 * 1024;
/// Maximum size of one per-file diff string.
pub const MAX_FILE_DIFF_BYTES: usize = 10 * 1024 * 1024;
/// Maximum number of changed files in an import.
pub const MAX_FILES: usize = 1_000;
/// Maximum number of top-level discussion entries in an import.
pub const MAX_DISCUSSIONS: usize = 10_000;
/// Maximum number of comments nested under one discussion.
pub const MAX_DISCUSSION_COMMENTS: usize = 10_000;
/// Maximum number of review threads seeded by an import.
pub const MAX_SEED_THREADS: usize = 10_000;

/// Version-one bundle posted by an agent after loading a PR with `gh`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrImportBundle {
    #[serde(deserialize_with = "deserialize_schema_version")]
    pub schema_version: u32,
    pub import_id: String,
    pub pr: ImportedPullRequest,
    pub overview_markdown: String,
    pub diff: DiffContext,
    pub files: Vec<ImportedFile>,
    pub discussions: Vec<ImportedDiscussion>,
    pub seed_threads: Vec<SeedReviewThread>,
}

impl PrImportBundle {
    /// Validates bounds, identity, uniqueness, SHAs, and each per-file diff.
    pub fn validate(&self, expected: &GithubPrUrl) -> Result<()> {
        self.validate_identity(expected)?;
        validate_sha("pr.base.sha", &self.pr.base.sha)?;
        validate_sha("pr.head.sha", &self.pr.head.sha)?;
        if self.import_id.trim().is_empty() {
            return Err(config_error("PR import `importId` must not be empty"));
        }
        if self.overview_markdown.trim().is_empty() {
            return Err(config_error(
                "PR import `overviewMarkdown` must not be empty",
            ));
        }
        if self.diff.context_source != DiffContextSource::GitUnified10
            || self.diff.context_lines != Some(10)
        {
            return Err(config_error(
                "PR import diff context must be `git-unified-10` with `contextLines: 10`",
            ));
        }
        check_count("files", self.files.len(), MAX_FILES)?;
        check_count("discussions", self.discussions.len(), MAX_DISCUSSIONS)?;
        check_count("seedThreads", self.seed_threads.len(), MAX_SEED_THREADS)?;
        self.validate_strings_and_total_size()?;

        let mut keys = HashSet::new();
        let mut old_paths = HashSet::new();
        let mut new_paths = HashSet::new();
        for file in &self.files {
            require_nonempty("file.key", &file.key)?;
            require_nonempty("file.oldPath", &file.old_path)?;
            require_nonempty("file.newPath", &file.new_path)?;
            if !keys.insert(file.key.as_str()) {
                return Err(config_error(format!(
                    "PR import contains duplicate file key {:?}",
                    file.key
                )));
            }
            if !old_paths.insert(file.old_path.as_str()) {
                return Err(config_error(format!(
                    "PR import contains duplicate old path {:?}",
                    file.old_path
                )));
            }
            if !new_paths.insert(file.new_path.as_str()) {
                return Err(config_error(format!(
                    "PR import contains duplicate new path {:?}",
                    file.new_path
                )));
            }
            let map = DiffMap::parse(&file.diff).map_err(|error| {
                config_error(format!("invalid diff for file key {:?}: {error}", file.key))
            })?;
            if map.old_path != file.old_path || map.new_path != file.new_path {
                return Err(config_error(format!(
                    "diff paths for file key {:?} are {:?} -> {:?}, expected {:?} -> {:?}",
                    file.key, map.old_path, map.new_path, file.old_path, file.new_path
                )));
            }
            if map.status != file.status {
                return Err(config_error(format!(
                    "diff status for file key {:?} is {:?}, expected {:?}",
                    file.key, map.status, file.status
                )));
            }
            if map.binary != file.binary {
                return Err(config_error(format!(
                    "diff binary metadata for file key {:?} does not match its diff",
                    file.key
                )));
            }
        }

        let mut discussion_ids = HashSet::new();
        let mut discussion_root_ids = HashSet::new();
        for discussion in &self.discussions {
            require_nonempty("discussion.id", &discussion.id)?;
            if !discussion_ids.insert(discussion.id.as_str()) {
                return Err(config_error(format!(
                    "PR import contains duplicate discussion id {:?}",
                    discussion.id
                )));
            }
            if let Some(root_id) = discussion.root_comment_id
                && (root_id == 0 || !discussion_root_ids.insert(root_id))
            {
                return Err(config_error(format!(
                    "discussion root comment id {root_id} must be positive and unique"
                )));
            }
            check_count(
                "discussion.comments",
                discussion.comments.len(),
                MAX_DISCUSSION_COMMENTS,
            )?;
        }

        let mut root_ids = HashSet::new();
        for thread in &self.seed_threads {
            let Some(discussion) = self
                .discussions
                .iter()
                .find(|discussion| discussion.id == thread.discussion_id)
            else {
                return Err(config_error(format!(
                    "seed thread root {} references unknown discussion {:?}",
                    thread.root_comment_id, thread.discussion_id
                )));
            };
            if discussion.kind != DiscussionKind::ReviewThread
                || discussion.review_id.is_none_or(|id| id == 0)
                || discussion.root_comment_id != Some(thread.root_comment_id)
            {
                return Err(config_error(format!(
                    "seed thread root {} must reference a reviewThread with matching positive reviewId/rootCommentId",
                    thread.root_comment_id
                )));
            }
            if discussion.resolved != Some(thread.resolved)
                || discussion.outdated != Some(thread.outdated)
            {
                return Err(config_error(format!(
                    "seed thread root {} resolved/outdated state does not match its discussion",
                    thread.root_comment_id
                )));
            }
            let Some(file) = self.files.iter().find(|file| file.key == thread.file_key) else {
                return Err(config_error(format!(
                    "seed thread root {} references unknown file key {:?}",
                    thread.root_comment_id, thread.file_key
                )));
            };
            if thread.path != file.old_path && thread.path != file.new_path {
                return Err(config_error(format!(
                    "seed thread root {} path {:?} does not match its imported file",
                    thread.root_comment_id, thread.path
                )));
            }
            if thread.github_thread_node_id.trim().is_empty() {
                return Err(config_error(format!(
                    "seed thread root {} must retain its GitHub review-thread node id",
                    thread.root_comment_id
                )));
            }
            if thread.root_comment_id == 0 || !root_ids.insert(thread.root_comment_id) {
                return Err(config_error(format!(
                    "seed thread root comment id {} must be positive and unique",
                    thread.root_comment_id
                )));
            }
            validate_sha("seedThreads.commitId", &thread.commit_id)?;
            if thread.line == 0 || thread.start_line == Some(0) {
                return Err(config_error(format!(
                    "seed thread root {} uses a zero GitHub line number",
                    thread.root_comment_id
                )));
            }
            if thread.start_line.is_some() != thread.start_side.is_some() {
                return Err(config_error(format!(
                    "seed thread root {} must provide both startLine and startSide, or neither",
                    thread.root_comment_id
                )));
            }
        }
        Ok(())
    }

    /// Validates that imported URL fields exactly match the CLI URL identity.
    pub fn validate_identity(&self, expected: &GithubPrUrl) -> Result<()> {
        let imported = GithubPrUrl::parse(&self.pr.url)
            .map_err(|error| config_error(format!("PR import `pr.url` is invalid: {error}")))?;
        if imported.canonical_url() != expected.canonical_url()
            || imported.owner() != self.pr.owner
            || imported.repo() != self.pr.repo
            || imported.number() != self.pr.number
            || self.pr.owner != expected.owner()
            || self.pr.repo != expected.repo()
            || self.pr.number != expected.number()
        {
            return Err(config_error(format!(
                "PR import identity does not match CLI URL {}; got {}/{}/pull/{} with URL {:?}",
                expected, self.pr.owner, self.pr.repo, self.pr.number, self.pr.url
            )));
        }
        Ok(())
    }

    fn validate_strings_and_total_size(&self) -> Result<()> {
        let mut strings = vec![
            ("importId", self.import_id.as_str()),
            ("pr.owner", self.pr.owner.as_str()),
            ("pr.repo", self.pr.repo.as_str()),
            ("pr.url", self.pr.url.as_str()),
            ("pr.title", self.pr.title.as_str()),
            ("pr.body", self.pr.body.as_str()),
            ("pr.author.login", self.pr.author.login.as_str()),
            ("pr.author.url", self.pr.author.url.as_str()),
            ("pr.base.ref", self.pr.base.ref_name.as_str()),
            ("pr.base.sha", self.pr.base.sha.as_str()),
            ("pr.head.ref", self.pr.head.ref_name.as_str()),
            ("pr.head.sha", self.pr.head.sha.as_str()),
            ("overviewMarkdown", self.overview_markdown.as_str()),
        ];
        for file in &self.files {
            strings.extend([
                ("file.key", file.key.as_str()),
                ("file.oldPath", file.old_path.as_str()),
                ("file.newPath", file.new_path.as_str()),
            ]);
            check_string("file.diff", &file.diff, MAX_FILE_DIFF_BYTES)?;
        }
        for discussion in &self.discussions {
            strings.extend([
                ("discussion.id", discussion.id.as_str()),
                ("discussion.author.login", discussion.author.login.as_str()),
                ("discussion.author.url", discussion.author.url.as_str()),
                ("discussion.url", discussion.url.as_str()),
                ("discussion.body", discussion.body.as_str()),
            ]);
            for comment in &discussion.comments {
                strings.extend([
                    (
                        "discussion.comment.author.login",
                        comment.author.login.as_str(),
                    ),
                    ("discussion.comment.author.url", comment.author.url.as_str()),
                    ("discussion.comment.url", comment.url.as_str()),
                    ("discussion.comment.body", comment.body.as_str()),
                ]);
            }
        }
        for thread in &self.seed_threads {
            strings.extend([
                ("seedThread.discussionId", thread.discussion_id.as_str()),
                ("seedThread.fileKey", thread.file_key.as_str()),
                (
                    "seedThread.githubThreadNodeId",
                    thread.github_thread_node_id.as_str(),
                ),
                ("seedThread.path", thread.path.as_str()),
                ("seedThread.commitId", thread.commit_id.as_str()),
                ("seedThread.body", thread.body.as_str()),
                ("seedThread.url", thread.url.as_str()),
                ("seedThread.author.login", thread.author.login.as_str()),
                ("seedThread.author.url", thread.author.url.as_str()),
            ]);
        }
        for (field, value) in strings {
            check_string(field, value, MAX_STRING_BYTES)?;
        }
        let size = serde_json::to_vec(self)
            .map_err(|error| config_error(format!("could not size PR import bundle: {error}")))?
            .len();
        if size > MAX_IMPORT_BYTES {
            return Err(config_error(format!(
                "PR import is {size} bytes, exceeding the {MAX_IMPORT_BYTES}-byte limit; narrow the PR before reviewing it"
            )));
        }
        Ok(())
    }
}

/// Pull-request metadata and exact repository identity.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedPullRequest {
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub url: String,
    pub title: String,
    pub body: String,
    pub state: PullRequestState,
    pub is_draft: bool,
    pub author: GithubAuthor,
    pub base: GithubRef,
    pub head: GithubRef,
}

/// GitHub pull-request state returned by `gh pr view`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PullRequestState {
    Open,
    Closed,
    Merged,
}

/// Minimal GitHub actor identity retained for display and provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubAuthor {
    pub login: String,
    pub url: String,
}

/// A branch name and immutable 40-hex commit SHA.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GithubRef {
    #[serde(rename = "ref")]
    pub ref_name: String,
    pub sha: String,
}

/// Metadata describing the context supplied by the single diff fetch.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DiffContext {
    pub context_lines: Option<u32>,
    pub context_source: DiffContextSource,
}

/// Source of unchanged lines in an imported diff.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiffContextSource {
    #[serde(rename = "git-unified-10")]
    GitUnified10,
}

/// One changed file and its complete git-style diff block.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedFile {
    pub key: String,
    pub old_path: String,
    pub new_path: String,
    pub status: DiffStatus,
    pub binary: bool,
    pub diff: String,
}

/// Kind of discussion rendered in the PR overview.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DiscussionKind {
    IssueComment,
    ReviewSummary,
    ReviewThread,
}

/// An overview discussion entry with GitHub provenance.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedDiscussion {
    pub id: String,
    pub kind: DiscussionKind,
    pub author: GithubAuthor,
    pub created_at: DateTime<Utc>,
    pub url: String,
    pub body: String,
    pub review_id: Option<u64>,
    pub root_comment_id: Option<u64>,
    pub resolved: Option<bool>,
    pub outdated: Option<bool>,
    pub comments: Vec<ImportedDiscussionComment>,
}

/// A GitHub comment nested under a discussion entry.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportedDiscussionComment {
    pub id: u64,
    pub author: GithubAuthor,
    pub created_at: DateTime<Utc>,
    pub url: String,
    pub body: String,
}

/// A GitHub review thread that can seed a prepopulated local thread.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeedReviewThread {
    pub discussion_id: String,
    pub file_key: String,
    pub root_comment_id: u64,
    pub github_thread_node_id: String,
    pub path: String,
    pub line: u32,
    pub side: DiffSide,
    pub start_line: Option<u32>,
    pub start_side: Option<DiffSide>,
    pub commit_id: String,
    pub body: String,
    pub url: String,
    pub author: GithubAuthor,
    pub created_at: DateTime<Utc>,
    pub resolved: bool,
    pub outdated: bool,
}

fn deserialize_schema_version<'de, D>(deserializer: D) -> std::result::Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    let version = u32::deserialize(deserializer)?;
    if version != PR_IMPORT_SCHEMA_VERSION {
        return Err(serde::de::Error::custom(format!(
            "unsupported PR import schemaVersion {version}; expected {PR_IMPORT_SCHEMA_VERSION}"
        )));
    }
    Ok(version)
}

fn validate_sha(field: &str, value: &str) -> Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(config_error(format!(
            "PR import `{field}` must be exactly 40 hexadecimal characters"
        )));
    }
    Ok(())
}

fn check_count(field: &str, actual: usize, maximum: usize) -> Result<()> {
    if actual > maximum {
        return Err(config_error(format!(
            "PR import `{field}` has {actual} entries, exceeding the limit of {maximum}"
        )));
    }
    Ok(())
}

fn check_string(field: &str, value: &str, maximum: usize) -> Result<()> {
    if value.len() > maximum {
        return Err(config_error(format!(
            "PR import `{field}` is {} bytes, exceeding the {maximum}-byte limit",
            value.len()
        )));
    }
    Ok(())
}

fn require_nonempty(field: &str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(config_error(format!(
            "PR import `{field}` must not be empty"
        )));
    }
    Ok(())
}

fn config_error(message: impl Into<String>) -> DiscussError {
    DiscussError::ConfigError {
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;
    use serde_json::json;

    use super::*;

    fn author() -> GithubAuthor {
        GithubAuthor {
            login: "octocat".to_string(),
            url: "https://github.com/octocat".to_string(),
        }
    }

    fn timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).single().unwrap()
    }

    fn bundle() -> PrImportBundle {
        PrImportBundle {
            schema_version: 1,
            import_id: format!("acme/project#51@{}", "b".repeat(40)),
            pr: ImportedPullRequest {
                owner: "acme".to_string(),
                repo: "project".to_string(),
                number: 51,
                url: "https://github.com/acme/project/pull/51".to_string(),
                title: "Improve it".to_string(),
                body: "Description".to_string(),
                state: PullRequestState::Open,
                is_draft: false,
                author: author(),
                base: GithubRef {
                    ref_name: "main".to_string(),
                    sha: "a".repeat(40),
                },
                head: GithubRef {
                    ref_name: "feature".to_string(),
                    sha: "b".repeat(40),
                },
            },
            overview_markdown: "# PR #51".to_string(),
            diff: DiffContext {
                context_lines: Some(10),
                context_source: DiffContextSource::GitUnified10,
            },
            files: vec![ImportedFile {
                key: "src/lib.rs".to_string(),
                old_path: "src/lib.rs".to_string(),
                new_path: "src/lib.rs".to_string(),
                status: DiffStatus::Modified,
                binary: false,
                diff: "diff --git a/src/lib.rs b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n".to_string(),
            }],
            discussions: vec![ImportedDiscussion {
                id: "review-thread:456".to_string(),
                kind: DiscussionKind::ReviewThread,
                author: author(),
                created_at: timestamp(),
                url: "https://github.com/acme/project/pull/51#discussion_r456".to_string(),
                body: "Please fix".to_string(),
                review_id: Some(123),
                root_comment_id: Some(456),
                resolved: Some(false),
                outdated: Some(false),
                comments: vec![],
            }],
            seed_threads: vec![SeedReviewThread {
                discussion_id: "review-thread:456".to_string(),
                file_key: "src/lib.rs".to_string(),
                root_comment_id: 456,
                github_thread_node_id: "PRRT_node".to_string(),
                path: "src/lib.rs".to_string(),
                line: 1,
                side: DiffSide::Right,
                start_line: None,
                start_side: None,
                commit_id: "b".repeat(40),
                body: "Please fix".to_string(),
                url: "https://github.com/acme/project/pull/51#discussion_r456".to_string(),
                author: author(),
                created_at: timestamp(),
                resolved: false,
                outdated: false,
            }],
        }
    }

    #[test]
    fn schema_round_trips_and_rejects_unknown_fields_or_versions() {
        let original = bundle();
        let value = serde_json::to_value(&original).unwrap();
        assert_eq!(value["schemaVersion"], 1);
        assert_eq!(
            serde_json::from_value::<PrImportBundle>(value.clone()).unwrap(),
            original
        );

        let mut unknown = value.clone();
        unknown["extra"] = json!(true);
        assert!(serde_json::from_value::<PrImportBundle>(unknown).is_err());
        let mut nested_unknown = value.clone();
        nested_unknown["pr"]["extra"] = json!(true);
        assert!(serde_json::from_value::<PrImportBundle>(nested_unknown).is_err());
        let mut wrong_version = value;
        wrong_version["schemaVersion"] = json!(2);
        assert!(serde_json::from_value::<PrImportBundle>(wrong_version).is_err());
    }

    #[test]
    fn validates_complete_bundle() {
        bundle()
            .validate(&GithubPrUrl::parse("https://github.com/acme/project/pull/51").unwrap())
            .unwrap();
    }

    #[test]
    fn rejects_identity_mismatch_and_invalid_sha() {
        let mut value = bundle();
        let expected = GithubPrUrl::parse("https://github.com/acme/project/pull/52").unwrap();
        assert!(
            value
                .validate(&expected)
                .unwrap_err()
                .to_string()
                .contains("identity")
        );

        value.pr.number = 51;
        value.pr.base.sha = "not-a-sha".to_string();
        let expected = GithubPrUrl::parse("https://github.com/acme/project/pull/51").unwrap();
        assert!(
            value
                .validate(&expected)
                .unwrap_err()
                .to_string()
                .contains("40 hexadecimal")
        );
    }

    #[test]
    fn rejects_bounds_and_empty_overview() {
        let expected = GithubPrUrl::parse("https://github.com/acme/project/pull/51").unwrap();
        let mut value = bundle();
        value.overview_markdown = " ".to_string();
        assert!(
            value
                .validate(&expected)
                .unwrap_err()
                .to_string()
                .contains("overviewMarkdown")
        );

        let mut value = bundle();
        value.pr.body = "x".repeat(MAX_STRING_BYTES + 1);
        assert!(
            value
                .validate(&expected)
                .unwrap_err()
                .to_string()
                .contains("limit")
        );
    }

    #[test]
    fn rejects_duplicate_keys_paths_discussions_and_roots() {
        let expected = GithubPrUrl::parse("https://github.com/acme/project/pull/51").unwrap();
        let mut duplicate_file = bundle();
        duplicate_file.files.push(duplicate_file.files[0].clone());
        assert!(
            duplicate_file
                .validate(&expected)
                .unwrap_err()
                .to_string()
                .contains("duplicate file key")
        );

        let mut duplicate_path = bundle();
        let mut second = duplicate_path.files[0].clone();
        second.key = "other".to_string();
        duplicate_path.files.push(second);
        assert!(
            duplicate_path
                .validate(&expected)
                .unwrap_err()
                .to_string()
                .contains("duplicate old path")
        );

        let mut duplicate_discussion = bundle();
        duplicate_discussion
            .discussions
            .push(duplicate_discussion.discussions[0].clone());
        assert!(
            duplicate_discussion
                .validate(&expected)
                .unwrap_err()
                .to_string()
                .contains("duplicate discussion")
        );

        let mut duplicate_root = bundle();
        duplicate_root
            .seed_threads
            .push(duplicate_root.seed_threads[0].clone());
        assert!(
            duplicate_root
                .validate(&expected)
                .unwrap_err()
                .to_string()
                .contains("unique")
        );
    }

    #[test]
    fn seed_threads_must_reference_consistent_review_thread_metadata() {
        let expected = GithubPrUrl::parse("https://github.com/acme/project/pull/51").unwrap();

        let mut wrong_kind = bundle();
        wrong_kind.discussions[0].kind = DiscussionKind::IssueComment;
        assert!(wrong_kind.validate(&expected).is_err());

        let mut wrong_state = bundle();
        wrong_state.seed_threads[0].outdated = true;
        assert!(wrong_state.validate(&expected).is_err());

        let mut wrong_path = bundle();
        wrong_path.seed_threads[0].path = "other.rs".to_string();
        assert!(wrong_path.validate(&expected).is_err());

        let mut missing_node = bundle();
        missing_node.seed_threads[0].github_thread_node_id.clear();
        assert!(missing_node.validate(&expected).is_err());
    }

    #[test]
    fn binary_and_textual_files_without_hunks_are_valid() {
        let expected = GithubPrUrl::parse("https://github.com/acme/project/pull/51").unwrap();
        let mut value = bundle();
        value.files[0].binary = true;
        value.files[0].diff = "diff --git a/src/lib.rs b/src/lib.rs\nBinary files a/src/lib.rs and b/src/lib.rs differ\n".to_string();
        value.validate(&expected).unwrap();

        value.files[0].binary = false;
        value.files[0].diff =
            "diff --git a/src/lib.rs b/src/lib.rs\nold mode 100644\nnew mode 100755\n".to_string();
        value.validate(&expected).unwrap();
    }
}
