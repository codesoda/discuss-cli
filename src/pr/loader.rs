//! Automatic, read-only pull-request loading through the authenticated `gh` CLI.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::Stdio;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::Command;

use super::{
    DiffContext, DiffContextSource, DiffMap, DiffSide, DiscussionKind, GithubAuthor, GithubPrUrl,
    GithubRef, ImportedDiscussion, ImportedDiscussionComment, ImportedFile, ImportedPullRequest,
    MAX_IMPORT_BYTES, PR_IMPORT_SCHEMA_VERSION, PrImportBundle, PullRequestState, SeedReviewThread,
};
use crate::{DiscussError, Result};

const GH_PR_FIELDS: &str =
    "number,title,body,state,isDraft,author,baseRefName,baseRefOid,headRefName,headRefOid,url";
const REVIEW_THREADS_QUERY: &str = "query($owner:String!,$repo:String!,$number:Int!,$endCursor:String){repository(owner:$owner,name:$repo){pullRequest(number:$number){reviewThreads(first:100,after:$endCursor){nodes{id isResolved isOutdated comments(first:1){nodes{databaseId}}}pageInfo{hasNextPage endCursor}}}}}";
const SMALL_OUTPUT_LIMIT: usize = 2 * 1024 * 1024;
const STDERR_LIMIT: usize = 64 * 1024;

/// Verifies the required GitHub CLI exists before a PR session binds or opens.
pub async fn ensure_gh_available() -> Result<()> {
    let mut command = configured_command("gh", &[OsStr::new("--version")]);
    command.stdout(Stdio::null()).stderr(Stdio::null());
    match command.status().await {
        Ok(status) if status.success() => Ok(()),
        Ok(_) => Err(missing_gh_error()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Err(missing_gh_error()),
        Err(error) => Err(DiscussError::PrError {
            message: format!("could not run `gh --version`: {error}"),
        }),
    }
}

/// Loads one complete PR review bundle using read-only `gh` and `git` commands.
pub async fn load_pr(identity: &GithubPrUrl, unified: u32) -> Result<PrImportBundle> {
    ensure_gh_available().await?;
    ensure_gh_authentication().await?;

    let temp = tempfile::tempdir().map_err(|error| DiscussError::PrError {
        message: format!("could not create a temporary directory for the PR: {error}"),
    })?;
    let repo_path = temp.path().join("repo");
    let canonical_url = identity.canonical_url().to_string();
    let number = identity.number().to_string();
    let repo_api = format!("repos/{}/{}", identity.owner(), identity.repo());
    let changed_files_endpoint = format!("{repo_api}/pulls/{number}/files?per_page=100");
    let issue_comments_endpoint = format!("{repo_api}/issues/{number}/comments?per_page=100");
    let reviews_endpoint = format!("{repo_api}/pulls/{number}/reviews?per_page=100");
    let review_comments_endpoint = format!("{repo_api}/pulls/{number}/comments?per_page=100");
    let clone_repo = format!("github.com/{}/{}", identity.owner(), identity.repo());
    let graph_owner = format!("owner={}", identity.owner());
    let graph_repo = format!("repo={}", identity.repo());
    let graph_number = format!("number={number}");
    let graph_query = format!("query={REVIEW_THREADS_QUERY}");

    let metadata_args = [
        OsStr::new("pr"),
        OsStr::new("view"),
        OsStr::new(&canonical_url),
        OsStr::new("--json"),
        OsStr::new(GH_PR_FIELDS),
    ];
    let metadata = run_command(
        "gh",
        &metadata_args,
        "fetch PR metadata",
        SMALL_OUTPUT_LIMIT,
    );
    let changed_files = gh_api_pages(&changed_files_endpoint, "fetch PR changed files");
    let issue_comments = gh_api_pages(&issue_comments_endpoint, "fetch PR issue comments");
    let reviews = gh_api_pages(&reviews_endpoint, "fetch PR review summaries");
    let review_comments = gh_api_pages(&review_comments_endpoint, "fetch PR review comments");
    let review_thread_args = [
        OsStr::new("api"),
        OsStr::new("--hostname"),
        OsStr::new("github.com"),
        OsStr::new("graphql"),
        OsStr::new("--paginate"),
        OsStr::new("--slurp"),
        OsStr::new("-f"),
        OsStr::new(&graph_owner),
        OsStr::new("-f"),
        OsStr::new(&graph_repo),
        OsStr::new("-F"),
        OsStr::new(&graph_number),
        OsStr::new("-f"),
        OsStr::new(&graph_query),
    ];
    let review_threads = run_command(
        "gh",
        &review_thread_args,
        "fetch PR review threads",
        MAX_IMPORT_BYTES,
    );
    let clone_args = [
        OsStr::new("repo"),
        OsStr::new("clone"),
        OsStr::new(&clone_repo),
        repo_path.as_os_str(),
        OsStr::new("--no-upstream"),
        OsStr::new("--"),
        OsStr::new("--filter=blob:none"),
        OsStr::new("--no-checkout"),
    ];
    let clone = run_command(
        "gh",
        &clone_args,
        "clone PR repository data",
        SMALL_OUTPUT_LIMIT,
    );

    let (metadata, changed_files, issue_comments, reviews, review_comments, review_threads, _) = tokio::try_join!(
        metadata,
        changed_files,
        issue_comments,
        reviews,
        review_comments,
        review_threads,
        clone,
    )?;

    let metadata: GhPullRequest = parse_json(&metadata, "PR metadata")?;
    validate_metadata_identity(identity, &metadata)?;
    let changed_files: Vec<Vec<GhChangedFile>> = parse_json(&changed_files, "changed files")?;
    let issue_comments: Vec<Vec<GhIssueComment>> = parse_json(&issue_comments, "issue comments")?;
    let reviews: Vec<Vec<GhReview>> = parse_json(&reviews, "review summaries")?;
    let review_comments: Vec<Vec<GhReviewComment>> =
        parse_json(&review_comments, "review comments")?;
    let review_threads: Vec<GhGraphqlPage> = parse_json(&review_threads, "review threads")?;

    fetch_pr_head(&repo_path, identity.number()).await?;
    verify_commit(&repo_path, &metadata.base_ref_oid, "base").await?;
    verify_commit_ref(
        &repo_path,
        "refs/discuss/pr-head^{commit}",
        &metadata.head_ref_oid,
        "head",
    )
    .await?;
    let aggregate_diff = git_diff(
        &repo_path,
        &metadata.base_ref_oid,
        &metadata.head_ref_oid,
        unified,
    )
    .await?;

    build_bundle(
        identity,
        metadata,
        changed_files.into_iter().flatten().collect(),
        GhDiscussionData {
            issue_comments: issue_comments.into_iter().flatten().collect(),
            reviews: reviews.into_iter().flatten().collect(),
            review_comments: review_comments.into_iter().flatten().collect(),
            review_thread_pages: review_threads,
        },
        &aggregate_diff,
        unified,
    )
}

async fn ensure_gh_authentication() -> Result<()> {
    let output = run_command_status(
        "gh",
        &[
            OsStr::new("auth"),
            OsStr::new("status"),
            OsStr::new("--hostname"),
            OsStr::new("github.com"),
        ],
        SMALL_OUTPUT_LIMIT,
    )
    .await?;
    if output.status.success() {
        return Ok(());
    }
    Err(DiscussError::ConfigError {
        message: "GitHub CLI (`gh`) is not authenticated for github.com. Run `gh auth login --hostname github.com`, then retry `discuss pr`.".to_string(),
    })
}

async fn gh_api_pages(endpoint: &str, label: &'static str) -> Result<Vec<u8>> {
    run_command(
        "gh",
        &[
            OsStr::new("api"),
            OsStr::new("--hostname"),
            OsStr::new("github.com"),
            OsStr::new("--paginate"),
            OsStr::new("--slurp"),
            OsStr::new(endpoint),
        ],
        label,
        MAX_IMPORT_BYTES,
    )
    .await
}

async fn fetch_pr_head(repo_path: &Path, number: u64) -> Result<()> {
    let refspec = format!("refs/pull/{number}/head:refs/discuss/pr-head");
    run_command(
        "git",
        &[
            OsStr::new("-C"),
            repo_path.as_os_str(),
            OsStr::new("fetch"),
            OsStr::new("--no-tags"),
            OsStr::new("origin"),
            OsStr::new(&refspec),
        ],
        "fetch immutable PR head",
        SMALL_OUTPUT_LIMIT,
    )
    .await?;
    Ok(())
}

async fn verify_commit(repo_path: &Path, sha: &str, label: &'static str) -> Result<()> {
    verify_commit_ref(repo_path, &format!("{sha}^{{commit}}"), sha, label).await
}

async fn verify_commit_ref(
    repo_path: &Path,
    revision: &str,
    expected: &str,
    label: &'static str,
) -> Result<()> {
    let output = run_command(
        "git",
        &[
            OsStr::new("-C"),
            repo_path.as_os_str(),
            OsStr::new("rev-parse"),
            OsStr::new(revision),
        ],
        "verify PR commit",
        1024,
    )
    .await?;
    let actual = utf8(&output, "verified commit SHA")?.trim();
    if actual != expected {
        return Err(DiscussError::PrError {
            message: format!(
                "GitHub {label} commit verification failed: expected {expected}, got {actual}"
            ),
        });
    }
    Ok(())
}

async fn git_diff(
    repo_path: &Path,
    base_sha: &str,
    head_sha: &str,
    unified: u32,
) -> Result<String> {
    let range = format!("{base_sha}...{head_sha}");
    let unified_arg = format!("--unified={unified}");
    let output = run_command(
        "git",
        &[
            OsStr::new("-C"),
            repo_path.as_os_str(),
            OsStr::new("diff"),
            OsStr::new("--no-color"),
            OsStr::new("--no-ext-diff"),
            OsStr::new("--no-textconv"),
            OsStr::new("--find-renames"),
            OsStr::new(&unified_arg),
            OsStr::new(&range),
        ],
        "generate PR diff",
        MAX_IMPORT_BYTES,
    )
    .await?;
    Ok(utf8(&output, "PR diff")?.to_string())
}

fn build_bundle(
    identity: &GithubPrUrl,
    metadata: GhPullRequest,
    changed_files: Vec<GhChangedFile>,
    discussion_data: GhDiscussionData,
    aggregate_diff: &str,
    unified: u32,
) -> Result<PrImportBundle> {
    let pr = ImportedPullRequest {
        owner: identity.owner().to_string(),
        repo: identity.repo().to_string(),
        number: metadata.number,
        url: metadata.url,
        title: metadata.title,
        body: metadata.body.unwrap_or_default(),
        state: metadata.state,
        is_draft: metadata.is_draft,
        author: graphql_author(metadata.author.as_ref()),
        base: GithubRef {
            ref_name: metadata.base_ref_name,
            sha: metadata.base_ref_oid,
        },
        head: GithubRef {
            ref_name: metadata.head_ref_name,
            sha: metadata.head_ref_oid,
        },
    };
    let files = split_diff(&changed_files, aggregate_diff)?;
    let (mut discussions, seed_threads) = build_discussions(
        &files,
        discussion_data.issue_comments,
        discussion_data.reviews,
        discussion_data.review_comments,
        discussion_data.review_thread_pages,
    );
    discussions.sort_by_key(|discussion| discussion.created_at);
    let overview_markdown = render_overview(&pr, &discussions);
    let bundle = PrImportBundle {
        schema_version: PR_IMPORT_SCHEMA_VERSION,
        import_id: format!("{}/{}#{}@{}", pr.owner, pr.repo, pr.number, pr.head.sha),
        pr,
        overview_markdown,
        diff: DiffContext {
            context_lines: Some(unified),
            context_source: if unified == 10 {
                DiffContextSource::GitUnified10
            } else {
                DiffContextSource::GitUnified
            },
        },
        files,
        discussions,
        seed_threads,
    };
    bundle.validate(identity)?;
    Ok(bundle)
}

fn split_diff(expected: &[GhChangedFile], aggregate_diff: &str) -> Result<Vec<ImportedFile>> {
    let mut starts = aggregate_diff
        .match_indices("diff --git ")
        .filter_map(|(index, _)| {
            (index == 0 || aggregate_diff.as_bytes().get(index.wrapping_sub(1)) == Some(&b'\n'))
                .then_some(index)
        })
        .collect::<Vec<_>>();
    starts.push(aggregate_diff.len());

    let mut by_path = HashMap::new();
    for pair in starts.windows(2) {
        let block = aggregate_diff[pair[0]..pair[1]].to_string();
        let map = DiffMap::parse(&block)?;
        let key = if map.status == super::DiffStatus::Deleted {
            map.old_path.clone()
        } else {
            map.new_path.clone()
        };
        let imported = ImportedFile {
            key: key.clone(),
            old_path: map.old_path,
            new_path: map.new_path,
            status: map.status,
            binary: map.binary,
            diff: block,
        };
        if by_path.insert(key.clone(), imported).is_some() {
            return Err(pr_error(format!(
                "the aggregate PR diff contains duplicate path {key:?}"
            )));
        }
    }

    let mut files = Vec::with_capacity(expected.len());
    for expected_file in expected {
        let Some(file) = by_path.remove(&expected_file.path) else {
            return Err(pr_error(format!(
                "GitHub listed changed path {:?}, but it was absent from the aggregate diff",
                expected_file.path
            )));
        };
        files.push(file);
    }
    if !by_path.is_empty() {
        let mut unexpected = by_path.into_keys().collect::<Vec<_>>();
        unexpected.sort();
        return Err(pr_error(format!(
            "the aggregate diff contains paths not listed by GitHub: {}",
            unexpected.join(", ")
        )));
    }
    Ok(files)
}

fn build_discussions(
    files: &[ImportedFile],
    issue_comments: Vec<GhIssueComment>,
    reviews: Vec<GhReview>,
    review_comments: Vec<GhReviewComment>,
    review_thread_pages: Vec<GhGraphqlPage>,
) -> (Vec<ImportedDiscussion>, Vec<SeedReviewThread>) {
    let mut discussions = issue_comments
        .into_iter()
        .map(|comment| ImportedDiscussion {
            id: format!("issue-comment:{}", comment.id),
            kind: DiscussionKind::IssueComment,
            author: rest_author(comment.user.as_ref()),
            created_at: comment.created_at,
            url: comment.html_url,
            body: comment.body.unwrap_or_default(),
            review_id: None,
            root_comment_id: None,
            resolved: None,
            outdated: None,
            comments: Vec::new(),
        })
        .collect::<Vec<_>>();

    discussions.extend(reviews.into_iter().filter_map(|review| {
        let submitted_at = review.submitted_at?;
        Some(ImportedDiscussion {
            id: format!("review-summary:{}", review.id),
            kind: DiscussionKind::ReviewSummary,
            author: rest_author(review.user.as_ref()),
            created_at: submitted_at,
            url: review.html_url,
            body: review.body.unwrap_or_default(),
            review_id: Some(review.id),
            root_comment_id: None,
            resolved: None,
            outdated: None,
            comments: Vec::new(),
        })
    }));

    let thread_state = review_thread_pages
        .into_iter()
        .flat_map(|page| page.data.repository.pull_request.review_threads.nodes)
        .filter_map(|thread| {
            let root = thread.comments.nodes.first()?.database_id?;
            Some((root, thread))
        })
        .collect::<HashMap<_, _>>();
    let mut replies = HashMap::<u64, Vec<GhReviewComment>>::new();
    let mut roots = Vec::new();
    for comment in review_comments {
        if let Some(root) = comment.in_reply_to_id {
            replies.entry(root).or_default().push(comment);
        } else {
            roots.push(comment);
        }
    }
    roots.sort_by_key(|comment| comment.created_at);

    let mut seeds = Vec::new();
    for root in roots {
        let graph = thread_state.get(&root.id);
        let resolved = graph.map(|thread| thread.is_resolved);
        let outdated = graph.map(|thread| thread.is_outdated);
        let discussion_id = format!("review-thread:{}", root.id);
        let mut nested = replies.remove(&root.id).unwrap_or_default();
        nested.sort_by_key(|comment| comment.created_at);
        let comments = nested
            .into_iter()
            .map(|comment| ImportedDiscussionComment {
                id: comment.id,
                author: rest_author(comment.user.as_ref()),
                created_at: comment.created_at,
                url: comment.html_url,
                body: comment.body.unwrap_or_default(),
            })
            .collect();
        discussions.push(ImportedDiscussion {
            id: discussion_id.clone(),
            kind: DiscussionKind::ReviewThread,
            author: rest_author(root.user.as_ref()),
            created_at: root.created_at,
            url: root.html_url.clone(),
            body: root.body.clone().unwrap_or_default(),
            review_id: root.pull_request_review_id,
            root_comment_id: Some(root.id),
            resolved,
            outdated,
            comments,
        });

        let Some(graph) = graph else { continue };
        let (Some(review_id), Some(path), Some(line), Some(side), Some(commit_id)) = (
            root.pull_request_review_id,
            root.path.as_ref(),
            root.line,
            root.side,
            root.commit_id.as_ref(),
        ) else {
            continue;
        };
        if review_id == 0 || line == 0 || commit_id.len() != 40 {
            continue;
        }
        if root.start_line.is_some() != root.start_side.is_some() {
            continue;
        }
        let Some(file_key) = files
            .iter()
            .find(|file| file.old_path == *path || file.new_path == *path)
            .map(|file| file.key.clone())
        else {
            continue;
        };
        seeds.push(SeedReviewThread {
            discussion_id,
            file_key,
            root_comment_id: root.id,
            github_thread_node_id: graph.id.clone(),
            path: path.clone(),
            line,
            side,
            start_line: root.start_line,
            start_side: root.start_side,
            commit_id: commit_id.clone(),
            body: root.body.unwrap_or_default(),
            url: root.html_url,
            author: rest_author(root.user.as_ref()),
            created_at: root.created_at,
            resolved: graph.is_resolved,
            outdated: graph.is_outdated,
        });
    }

    // A deleted or otherwise unavailable root can leave a reply unmatched in
    // the REST result. Preserve it as linked read-only context rather than
    // inventing a reply destination.
    let mut unmatched_replies = replies.into_values().flatten().collect::<Vec<_>>();
    unmatched_replies.sort_by_key(|comment| comment.created_at);
    discussions.extend(
        unmatched_replies
            .into_iter()
            .map(|comment| ImportedDiscussion {
                id: format!("review-comment:{}", comment.id),
                kind: DiscussionKind::ReviewThread,
                author: rest_author(comment.user.as_ref()),
                created_at: comment.created_at,
                url: comment.html_url,
                body: comment.body.unwrap_or_default(),
                review_id: comment.pull_request_review_id,
                root_comment_id: None,
                resolved: None,
                outdated: None,
                comments: Vec::new(),
            }),
    );
    (discussions, seeds)
}

fn render_overview(pr: &ImportedPullRequest, discussions: &[ImportedDiscussion]) -> String {
    let state = match pr.state {
        PullRequestState::Open => "Open",
        PullRequestState::Closed => "Closed",
        PullRequestState::Merged => "Merged",
    };
    let draft = if pr.is_draft { " (Draft)" } else { "" };
    let body = if pr.body.trim().is_empty() {
        "_No description provided._"
    } else {
        &pr.body
    };
    let mut markdown = format!(
        "# PR #{}: {}\n\n- **Author:** [{}]({})\n- **State:** {state}{draft}\n- **Base:** `{}`\n- **Head:** `{}`\n- **GitHub:** {}\n- **Files:** {}/files\n\n## Description\n\n{}\n\n## Existing discussion\n",
        pr.number,
        pr.title,
        pr.author.login,
        pr.author.url,
        pr.base.ref_name.replace('`', "\\`"),
        pr.head.ref_name.replace('`', "\\`"),
        pr.url,
        pr.url,
        body,
    );
    if discussions.is_empty() {
        markdown.push_str("\n_No existing issue comments, review summaries, or review threads._\n");
        return markdown;
    }
    for discussion in discussions {
        let (kind, link_label) = match discussion.kind {
            DiscussionKind::IssueComment => ("Issue comment", "comment"),
            DiscussionKind::ReviewSummary => ("Review summary", "review"),
            DiscussionKind::ReviewThread => ("Review thread", "thread"),
        };
        let body = if discussion.body.trim().is_empty() {
            "_No text provided._"
        } else {
            &discussion.body
        };
        markdown.push_str(&format!(
            "\n### {kind} — [{}]({}) — {}\n\n{}\n\n[Open {link_label} on GitHub]({})\n",
            discussion.author.login,
            discussion.author.url,
            discussion.created_at.to_rfc3339(),
            body,
            discussion.url,
        ));
        for comment in &discussion.comments {
            let body = if comment.body.trim().is_empty() {
                "_No text provided._"
            } else {
                &comment.body
            };
            markdown.push_str(&format!(
                "\n#### Reply — [{}]({}) — {}\n\n{}\n\n[Open reply on GitHub]({})\n",
                comment.author.login,
                comment.author.url,
                comment.created_at.to_rfc3339(),
                body,
                comment.url,
            ));
        }
    }
    markdown
}

fn validate_metadata_identity(identity: &GithubPrUrl, metadata: &GhPullRequest) -> Result<()> {
    let fetched = GithubPrUrl::parse(&metadata.url).map_err(|error| {
        pr_error(format!(
            "GitHub returned an invalid pull-request URL: {error}"
        ))
    })?;
    if metadata.number != identity.number() || fetched.canonical_url() != identity.canonical_url() {
        return Err(pr_error(format!(
            "GitHub returned PR identity {}, expected {}",
            fetched, identity
        )));
    }
    Ok(())
}

fn graphql_author(author: Option<&GhGraphqlActor>) -> GithubAuthor {
    let login = author
        .map(|author| author.login.as_str())
        .filter(|login| !login.is_empty())
        .unwrap_or("ghost")
        .to_string();
    GithubAuthor {
        url: format!("https://github.com/{login}"),
        login,
    }
}

fn rest_author(author: Option<&GhRestActor>) -> GithubAuthor {
    let login = author
        .map(|author| author.login.as_str())
        .filter(|login| !login.is_empty())
        .unwrap_or("ghost")
        .to_string();
    GithubAuthor {
        url: author
            .map(|author| author.html_url.clone())
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| format!("https://github.com/{login}")),
        login,
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8], label: &str) -> Result<T> {
    serde_json::from_slice(bytes).map_err(|error| {
        pr_error(format!(
            "GitHub returned invalid {label} JSON through `gh`: {error}"
        ))
    })
}

fn utf8<'a>(bytes: &'a [u8], label: &str) -> Result<&'a str> {
    std::str::from_utf8(bytes)
        .map_err(|error| pr_error(format!("{label} was not valid UTF-8: {error}")))
}

fn missing_gh_error() -> DiscussError {
    DiscussError::ConfigError {
        message: "GitHub CLI (`gh`) is required for `discuss pr`, but it was not found in PATH.\nInstall it, then authenticate and retry:\n  macOS:   brew install gh\n  Windows: winget install --id GitHub.cli\n  Linux:   https://github.com/cli/cli/blob/trunk/docs/install_linux.md\n  Other:   https://cli.github.com/\n  Auth:    gh auth login --hostname github.com".to_string(),
    }
}

fn pr_error(message: impl Into<String>) -> DiscussError {
    DiscussError::PrError {
        message: message.into(),
    }
}

fn configured_command(program: &str, args: &[&OsStr]) -> Command {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    command
}

async fn run_command(
    program: &str,
    args: &[&OsStr],
    label: &'static str,
    limit: usize,
) -> Result<Vec<u8>> {
    let output = run_command_status(program, args, limit).await?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        let detail = detail.trim();
        return Err(pr_error(if detail.is_empty() {
            format!("failed to {label}: command exited with {}", output.status)
        } else {
            format!("failed to {label}: {detail}")
        }));
    }
    if output.stdout_overflow {
        return Err(pr_error(format!(
            "failed to {label}: command output exceeded the {limit}-byte safety limit"
        )));
    }
    Ok(output.stdout)
}

#[derive(Debug)]
struct CapturedOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_overflow: bool,
}

async fn run_command_status(
    program: &str,
    args: &[&OsStr],
    stdout_limit: usize,
) -> Result<CapturedOutput> {
    let mut command = configured_command(program, args);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound && program == "gh" {
            missing_gh_error()
        } else {
            pr_error(format!("could not start {program}: {error}"))
        }
    })?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let (status, stdout, stderr) = tokio::join!(
        child.wait(),
        read_capped(stdout, stdout_limit),
        read_capped(stderr, STDERR_LIMIT),
    );
    let status =
        status.map_err(|error| pr_error(format!("could not wait for {program}: {error}")))?;
    let (stdout, stdout_overflow) =
        stdout.map_err(|error| pr_error(format!("could not read {program} stdout: {error}")))?;
    let (stderr, _) =
        stderr.map_err(|error| pr_error(format!("could not read {program} stderr: {error}")))?;
    Ok(CapturedOutput {
        status,
        stdout,
        stderr,
        stdout_overflow,
    })
}

async fn read_capped<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> io::Result<(Vec<u8>, bool)> {
    let mut retained = Vec::new();
    let mut overflow = false;
    let mut chunk = [0u8; 8192];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(retained.len());
        retained.extend_from_slice(&chunk[..read.min(remaining)]);
        overflow |= read > remaining;
    }
    Ok((retained, overflow))
}

struct GhDiscussionData {
    issue_comments: Vec<GhIssueComment>,
    reviews: Vec<GhReview>,
    review_comments: Vec<GhReviewComment>,
    review_thread_pages: Vec<GhGraphqlPage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPullRequest {
    number: u64,
    title: String,
    body: Option<String>,
    state: PullRequestState,
    is_draft: bool,
    author: Option<GhGraphqlActor>,
    base_ref_name: String,
    base_ref_oid: String,
    head_ref_name: String,
    head_ref_oid: String,
    url: String,
}

#[derive(Debug, Deserialize)]
struct GhGraphqlActor {
    login: String,
}

#[derive(Debug, Deserialize)]
struct GhChangedFile {
    #[serde(alias = "path", rename = "filename")]
    path: String,
}

#[derive(Debug, Deserialize)]
struct GhRestActor {
    login: String,
    html_url: String,
}

#[derive(Debug, Deserialize)]
struct GhIssueComment {
    id: u64,
    user: Option<GhRestActor>,
    created_at: DateTime<Utc>,
    html_url: String,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhReview {
    id: u64,
    user: Option<GhRestActor>,
    submitted_at: Option<DateTime<Utc>>,
    html_url: String,
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhReviewComment {
    id: u64,
    user: Option<GhRestActor>,
    created_at: DateTime<Utc>,
    html_url: String,
    body: Option<String>,
    pull_request_review_id: Option<u64>,
    in_reply_to_id: Option<u64>,
    path: Option<String>,
    line: Option<u32>,
    side: Option<DiffSide>,
    start_line: Option<u32>,
    start_side: Option<DiffSide>,
    commit_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhGraphqlPage {
    data: GhGraphqlData,
}

#[derive(Debug, Deserialize)]
struct GhGraphqlData {
    repository: GhGraphqlRepository,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhGraphqlRepository {
    pull_request: GhGraphqlPullRequest,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhGraphqlPullRequest {
    review_threads: GhGraphqlReviewThreads,
}

#[derive(Debug, Deserialize)]
struct GhGraphqlReviewThreads {
    nodes: Vec<GhGraphqlReviewThread>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhGraphqlReviewThread {
    id: String,
    is_resolved: bool,
    is_outdated: bool,
    comments: GhGraphqlThreadComments,
}

#[derive(Debug, Deserialize)]
struct GhGraphqlThreadComments {
    nodes: Vec<GhGraphqlRootComment>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhGraphqlRootComment {
    database_id: Option<u64>,
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn identity() -> GithubPrUrl {
        GithubPrUrl::parse("https://github.com/acme/project/pull/51").unwrap()
    }

    fn metadata() -> GhPullRequest {
        GhPullRequest {
            number: 51,
            title: "Improve parser".to_string(),
            body: Some("Details".to_string()),
            state: PullRequestState::Open,
            is_draft: false,
            author: Some(GhGraphqlActor {
                login: "octocat".to_string(),
            }),
            base_ref_name: "main".to_string(),
            base_ref_oid: "a".repeat(40),
            head_ref_name: "feature".to_string(),
            head_ref_oid: "b".repeat(40),
            url: identity().canonical_url().to_string(),
        }
    }

    #[test]
    fn builds_valid_bundle_and_overview_from_gh_shapes() {
        let time = Utc.with_ymd_and_hms(2026, 9, 1, 12, 0, 0).single().unwrap();
        let actor = || GhRestActor {
            login: "reviewer".to_string(),
            html_url: "https://github.com/reviewer".to_string(),
        };
        let issue = GhIssueComment {
            id: 1,
            user: Some(actor()),
            created_at: time,
            html_url: "https://github.com/acme/project/pull/51#issuecomment-1".to_string(),
            body: Some("Issue note".to_string()),
        };
        let root = GhReviewComment {
            id: 456,
            user: Some(actor()),
            created_at: time,
            html_url: "https://github.com/acme/project/pull/51#discussion_r456".to_string(),
            body: Some("Please fix".to_string()),
            pull_request_review_id: Some(123),
            in_reply_to_id: None,
            path: Some("src/lib.rs".to_string()),
            line: Some(1),
            side: Some(DiffSide::Right),
            start_line: None,
            start_side: None,
            commit_id: Some("b".repeat(40)),
        };
        let graph = GhGraphqlPage {
            data: GhGraphqlData {
                repository: GhGraphqlRepository {
                    pull_request: GhGraphqlPullRequest {
                        review_threads: GhGraphqlReviewThreads {
                            nodes: vec![GhGraphqlReviewThread {
                                id: "PRRT_node".to_string(),
                                is_resolved: false,
                                is_outdated: false,
                                comments: GhGraphqlThreadComments {
                                    nodes: vec![GhGraphqlRootComment {
                                        database_id: Some(456),
                                    }],
                                },
                            }],
                        },
                    },
                },
            },
        };
        let diff = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-old\n+new\n";
        let bundle = build_bundle(
            &identity(),
            metadata(),
            vec![GhChangedFile {
                path: "src/lib.rs".to_string(),
            }],
            GhDiscussionData {
                issue_comments: vec![issue],
                reviews: Vec::new(),
                review_comments: vec![root],
                review_thread_pages: vec![graph],
            },
            diff,
            10,
        )
        .unwrap();

        assert_eq!(bundle.files.len(), 1);
        assert_eq!(bundle.seed_threads.len(), 1);
        assert_eq!(bundle.seed_threads[0].root_comment_id, 456);
        assert!(bundle.overview_markdown.contains("### Issue comment"));
        assert!(bundle.overview_markdown.contains("### Review thread"));
        assert!(bundle.overview_markdown.contains("Open thread on GitHub"));
    }

    #[test]
    fn split_diff_preserves_github_file_order_and_rejects_mismatch() {
        let expected = vec![
            GhChangedFile {
                path: "b.rs".to_string(),
            },
            GhChangedFile {
                path: "a.rs".to_string(),
            },
        ];
        let diff = "diff --git a/a.rs b/a.rs\n@@ -1 +1 @@\n-a\n+A\ndiff --git a/b.rs b/b.rs\n@@ -1 +1 @@\n-b\n+B\n";
        let files = split_diff(&expected, diff).unwrap();
        assert_eq!(
            files
                .iter()
                .map(|file| file.key.as_str())
                .collect::<Vec<_>>(),
            vec!["b.rs", "a.rs"]
        );

        let missing = vec![GhChangedFile {
            path: "missing.rs".to_string(),
        }];
        assert!(
            split_diff(&missing, diff)
                .unwrap_err()
                .to_string()
                .contains("absent")
        );
    }

    #[tokio::test]
    async fn missing_gh_error_contains_install_and_auth_instructions() {
        let error = run_command_status("this-gh-command-does-not-exist-discuss-test", &[], 1024)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("could not start"));

        let message = missing_gh_error().to_string();
        for expected in [
            "brew install gh",
            "winget install",
            "install_linux.md",
            "gh auth login",
        ] {
            assert!(
                message.contains(expected),
                "missing {expected:?} in {message:?}"
            );
        }
    }
}
