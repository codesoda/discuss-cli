//! Confirmed GitHub review publication through the authenticated `gh` CLI.

use std::ffi::OsStr;
use std::process::Stdio;

use serde::Deserialize;
use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::{
    PrPublicationError, PrPublicationResult, PrPublicationStatus, PrPublishedLink, PrPublishedReply,
};

#[derive(Clone, Debug)]
pub struct DirectPublication {
    pub request_id: String,
    pub owner: String,
    pub repo: String,
    pub number: u64,
    pub head_sha: String,
    pub review: Option<Value>,
    pub replies: Vec<DirectReply>,
}

#[derive(Clone, Debug)]
pub struct DirectReply {
    pub operation_id: String,
    pub root_comment_id: u64,
    pub request: Value,
}

/// Publishes only the operations authorized by the exact confirmed preview.
pub async fn publish(plan: DirectPublication) -> PrPublicationResult {
    publish_with_program(plan, OsStr::new("gh")).await
}

async fn publish_with_program(plan: DirectPublication, program: &OsStr) -> PrPublicationResult {
    let mut completed = Vec::new();
    let mut published_review = None;
    let mut published_replies = Vec::new();

    let head_endpoint = format!("repos/{}/{}/pulls/{}", plan.owner, plan.repo, plan.number);
    let head = match run_gh(
        program,
        &[
            OsStr::new("api"),
            OsStr::new("--hostname"),
            OsStr::new("github.com"),
            OsStr::new(&head_endpoint),
            OsStr::new("--jq"),
            OsStr::new(".head.sha"),
        ],
        None,
    )
    .await
    {
        Ok(output) => String::from_utf8_lossy(&output).trim().to_string(),
        Err(error) => {
            return failed(
                plan.request_id,
                "head_recheck_failed",
                error.message,
                completed,
                Vec::new(),
                published_review,
                published_replies,
            );
        }
    };
    if head != plan.head_sha {
        return failed(
            plan.request_id,
            "stale_pr_head",
            format!(
                "the PR head changed from {} to {}; reload the review before publishing",
                plan.head_sha, head
            ),
            completed,
            Vec::new(),
            published_review,
            published_replies,
        );
    }

    if let Some(request) = plan.review {
        let endpoint = format!(
            "repos/{}/{}/pulls/{}/reviews",
            plan.owner, plan.repo, plan.number
        );
        match post_json(program, &endpoint, &request).await {
            Ok(created) => {
                completed.push("review".to_string());
                published_review = Some(PrPublishedLink {
                    id: Some(created.id),
                    url: created.html_url,
                });
            }
            Err(error) => {
                let unknown = error
                    .unknown
                    .then(|| "review".to_string())
                    .into_iter()
                    .collect();
                return failed(
                    plan.request_id,
                    "review_publication_failed",
                    error.message,
                    completed,
                    unknown,
                    published_review,
                    published_replies,
                );
            }
        }
    }

    for reply in plan.replies {
        let endpoint = format!(
            "repos/{}/{}/pulls/{}/comments/{}/replies",
            plan.owner, plan.repo, plan.number, reply.root_comment_id
        );
        match post_json(program, &endpoint, &reply.request).await {
            Ok(created) => {
                completed.push(reply.operation_id.clone());
                published_replies.push(PrPublishedReply {
                    operation_id: reply.operation_id,
                    root_comment_id: reply.root_comment_id,
                    id: Some(created.id),
                    url: created.html_url,
                });
            }
            Err(error) => {
                let unknown = error
                    .unknown
                    .then(|| reply.operation_id.clone())
                    .into_iter()
                    .collect();
                return failed(
                    plan.request_id,
                    "review_reply_failed",
                    error.message,
                    completed,
                    unknown,
                    published_review,
                    published_replies,
                );
            }
        }
    }

    PrPublicationResult {
        request_id: plan.request_id,
        status: PrPublicationStatus::Succeeded,
        review: published_review,
        replies: published_replies,
        completed_operations: completed,
        unknown_operations: Vec::new(),
        error: None,
    }
}

async fn post_json(
    program: &OsStr,
    endpoint: &str,
    request: &Value,
) -> Result<GithubCreated, GhFailure> {
    let input = serde_json::to_vec(request).map_err(|error| GhFailure {
        message: format!("could not encode the confirmed GitHub request: {error}"),
        unknown: false,
    })?;
    let output = run_gh(
        program,
        &[
            OsStr::new("api"),
            OsStr::new("--hostname"),
            OsStr::new("github.com"),
            OsStr::new("--method"),
            OsStr::new("POST"),
            OsStr::new(endpoint),
            OsStr::new("--input"),
            OsStr::new("-"),
        ],
        Some(input),
    )
    .await?;
    let created: GithubCreated = serde_json::from_slice(&output).map_err(|error| GhFailure {
        message: format!("GitHub returned an invalid publication response through `gh`: {error}"),
        // A successful command with an unreadable creation response may have
        // created the object, so retrying is unsafe until reconciled.
        unknown: true,
    })?;
    let valid_url = url::Url::parse(&created.html_url).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("github.com")
            && url.username().is_empty()
            && url.password().is_none()
    });
    if created.id == 0 || !valid_url {
        return Err(GhFailure {
            message: "GitHub returned invalid publication provenance through `gh`".to_string(),
            unknown: true,
        });
    }
    Ok(created)
}

async fn run_gh(
    program: &OsStr,
    args: &[&OsStr],
    input: Option<Vec<u8>>,
) -> Result<Vec<u8>, GhFailure> {
    let mut command = Command::new(program);
    command
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command.spawn().map_err(|error| GhFailure {
        message: format!("could not start `gh`: {error}"),
        unknown: false,
    })?;
    if let Some(input) = input {
        let mut stdin = child.stdin.take().expect("piped gh stdin");
        if let Err(error) = stdin.write_all(&input).await {
            return Err(GhFailure {
                message: format!("could not send the confirmed request to `gh`: {error}"),
                unknown: true,
            });
        }
    }
    let output = child.wait_with_output().await.map_err(|error| GhFailure {
        message: format!("could not wait for `gh`: {error}"),
        unknown: true,
    })?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    let message = if detail.is_empty() {
        format!("`gh` exited with {}", output.status)
    } else {
        detail.chars().take(4096).collect()
    };
    Err(GhFailure {
        message,
        unknown: !definite_rejection(detail),
    })
}

fn definite_rejection(stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    lower.contains("http 400")
        || lower.contains("http 401")
        || lower.contains("http 403")
        || lower.contains("http 404")
        || lower.contains("http 409")
        || lower.contains("http 422")
}

fn failed(
    request_id: String,
    code: &str,
    message: String,
    completed_operations: Vec<String>,
    unknown_operations: Vec<String>,
    review: Option<PrPublishedLink>,
    replies: Vec<PrPublishedReply>,
) -> PrPublicationResult {
    PrPublicationResult {
        request_id,
        status: PrPublicationStatus::Failed,
        review,
        replies,
        completed_operations,
        unknown_operations,
        error: Some(PrPublicationError {
            code: code.to_string(),
            message,
        }),
    }
}

#[derive(Debug, Deserialize)]
struct GithubCreated {
    id: u64,
    html_url: String,
}

#[derive(Debug)]
struct GhFailure {
    message: String,
    unknown: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn classifies_explicit_github_rejections_as_safe_to_retry() {
        assert!(definite_rejection("gh: Validation Failed (HTTP 422)"));
        assert!(definite_rejection("HTTP 401: Bad credentials"));
        assert!(!definite_rejection("connection reset by peer"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn publishes_grouped_review_and_reply_through_gh() {
        let directory = tempfile::tempdir().unwrap();
        let gh = directory.path().join("gh");
        fs::write(
            &gh,
            r##"#!/bin/sh
if [ "$4" != "--method" ]; then
  printf '%s\n' 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb'
  exit 0
fi
IFS= read -r input || true
case "$6" in
  */reviews) printf '%s\n' '{"id":7,"html_url":"https://github.com/acme/project/pull/1#pullrequestreview-7"}' ;;
  */replies) printf '%s\n' '{"id":8,"html_url":"https://github.com/acme/project/pull/1#discussion_r8"}' ;;
  *) exit 1 ;;
esac
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&gh).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&gh, permissions).unwrap();
        let result = publish_with_program(
            DirectPublication {
                request_id: "request-1".to_string(),
                owner: "acme".to_string(),
                repo: "project".to_string(),
                number: 1,
                head_sha: "b".repeat(40),
                review: Some(serde_json::json!({
                    "event": "COMMENT",
                    "body": "Summary",
                    "comments": [],
                })),
                replies: vec![DirectReply {
                    operation_id: "reply:u-1".to_string(),
                    root_comment_id: 6,
                    request: serde_json::json!({ "body": "Reply" }),
                }],
            },
            gh.as_os_str(),
        )
        .await;

        assert_eq!(result.status, PrPublicationStatus::Succeeded);
        assert_eq!(result.completed_operations, ["review", "reply:u-1"]);
        assert_eq!(result.review.unwrap().id, Some(7));
        assert_eq!(result.replies[0].id, Some(8));
    }

    #[test]
    fn failed_result_preserves_completed_and_unknown_operations() {
        let result = failed(
            "request-1".to_string(),
            "review_reply_failed",
            "network failed".to_string(),
            vec!["review".to_string()],
            vec!["reply:u-1".to_string()],
            Some(PrPublishedLink {
                id: Some(7),
                url: "https://github.com/acme/repo/pull/1#pullrequestreview-7".to_string(),
            }),
            Vec::new(),
        );
        assert_eq!(result.status, PrPublicationStatus::Failed);
        assert_eq!(result.completed_operations, ["review"]);
        assert_eq!(result.unknown_operations, ["reply:u-1"]);
    }
}
