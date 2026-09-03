/// Checks that `gh` has usable GitHub authentication.
pub const GH_AUTH_STATUS_COMMAND: &str = "gh auth status --hostname github.com";

/// Fetches all PR metadata needed by the version-one import bundle.
pub const GH_PR_VIEW_COMMAND: &str = "gh pr view \"$PR_URL\" --json number,title,body,state,isDraft,author,baseRefName,baseRefOid,headRefName,headRefOid,url,files";

/// Fetches every PR issue comment through REST pagination.
pub const GH_ISSUE_COMMENTS_COMMAND: &str = "gh api --hostname github.com --paginate --slurp \"repos/$OWNER/$REPO/issues/$NUMBER/comments?per_page=100\"";

/// Fetches every review summary through REST pagination.
pub const GH_REVIEWS_COMMAND: &str = "gh api --hostname github.com --paginate --slurp \"repos/$OWNER/$REPO/pulls/$NUMBER/reviews?per_page=100\"";

/// Fetches every inline review comment through REST pagination.
pub const GH_REVIEW_COMMENTS_COMMAND: &str = "gh api --hostname github.com --paginate --slurp \"repos/$OWNER/$REPO/pulls/$NUMBER/comments?per_page=100\"";

/// Fetches review-thread node state and root comment IDs through GraphQL pagination.
pub const GH_REVIEW_THREADS_COMMAND: &str = r#"gh api --hostname github.com graphql --paginate --slurp -f owner="$OWNER" -f repo="$REPO" -F number="$NUMBER" -f query='query($owner:String!,$repo:String!,$number:Int!,$endCursor:String){repository(owner:$owner,name:$repo){pullRequest(number:$number){reviewThreads(first:100,after:$endCursor){nodes{id isResolved isOutdated comments(first:1){nodes{databaseId}}}pageInfo{hasNextPage endCursor}}}}}'"#;

/// Clones PR commit/tree data through the authenticated `gh` transport.
pub const GH_PR_CLONE_COMMAND: &str = "gh repo clone \"github.com/$OWNER/$REPO\" \"$PR_REPO\" --no-upstream -- --filter=blob:none --no-checkout";

/// Fetches the immutable pull-request head ref into the temporary clone.
pub const GH_PR_HEAD_FETCH_COMMAND: &str =
    "git -C \"$PR_REPO\" fetch --no-tags origin \"refs/pull/$NUMBER/head:refs/discuss/pr-head\"";

/// Generates the complete aggregate diff exactly once with ten context lines.
pub const GH_PR_DIFF_COMMAND: &str = "git -C \"$PR_REPO\" diff --no-color --no-ext-diff --no-textconv --find-renames --unified=10 \"$BASE_SHA...$HEAD_SHA\"";

/// Rechecks the immutable PR head before any publication.
pub const GH_HEAD_RECHECK_COMMAND: &str =
    "gh api --hostname github.com \"repos/$OWNER/$REPO/pulls/$NUMBER\" --jq .head.sha";

/// Publishes one grouped review from a JSON request on stdin.
pub const GH_GROUPED_REVIEW_COMMAND: &str = "gh api --hostname github.com --method POST \"repos/$OWNER/$REPO/pulls/$NUMBER/reviews\" --input -";

/// Replies to one existing review-comment thread from JSON on stdin.
pub const GH_REVIEW_REPLY_COMMAND: &str = "gh api --hostname github.com --method POST \"repos/$OWNER/$REPO/pulls/$NUMBER/comments/$ROOT_ID/replies\" --input -";

/// Returns the private-first agent protocol after Discuss automatically loads a PR.
pub fn agent_instructions() -> &'static str {
    r##"GitHub pull-request review summary and publication protocol

Automatic read-only import
- Discuss itself loads the PR through the user's authenticated `gh` CLI. Do not fetch, clone, build an import bundle, or call `/api/pr/import`.
- Wait for `pr.imported`. Its payload identifies the `pr-overview` file and every changed-file ID. Diff `anchorStart`/`anchorEnd` values select a rendered hunk; `lineRange` selects rows within that hunk.
- Imported `gh-review-thread-*` IDs retain their immutable GitHub root-comment reply targets. Issue comments and review summaries remain linked, read-only context.

Security and privacy
- The session is private-first. Keep every new question, local thread, answer, summary, and draft local unless the reviewer explicitly includes it and confirms the final preview with OK.
- Never create a standalone PR comment. Publish a reply only when an imported review thread has a positive root comment database ID. Publish a new inline comment only when its file, side, and changed line resolve confidently.
- Keep binary, no-hunk, outdated, detached, or otherwise ambiguous targets unpublished and report why; never guess a destination or create a duplicate.
- Never request, print, persist, or POST a GitHub token to Discuss. Never interpolate review text, summary text, or reply text into shell flags; pass exact structured JSON through stdin with `--input -`.

Prepare and summary
- The browser starts preparation. On `pr.summary.requested`, generate only the requested editable review summary from the supplied local conversations.
- POST `{ "requestId": "...", "summary": "..." }` as JSON stdin to the event's `summaryUrl`, using `Authorization: Bearer <SESSION_SECRET>`.
- Do not choose inclusion or publication destinations. The reviewer owns action, summary edits, include/exclude choices, comment edits, confirmation, and cancellation. Include controls default off.
- Cancel or Go back is not approval and must not publish anything.

Publish and result
- Act only on one authoritative `pr.publish.requested` event emitted after OK. Use the exact confirmed body and targets in that event.
- Immediately recheck the head SHA:
   gh api --hostname github.com "repos/$OWNER/$REPO/pulls/$NUMBER" --jq .head.sha
  If it differs from the imported/event head SHA, publish nothing and report `stale_pr_head` to `resultUrl`.
- Group all new inline comments into exactly one review request. Pipe `payload.review.githubRequest` unchanged through stdin; `operationId` and `commentOperations` are local bookkeeping and must not be sent to GitHub:
   gh api --hostname github.com --method POST "repos/$OWNER/$REPO/pulls/$NUMBER/reviews" --input -
- Send each selected existing-thread reply separately. Set ROOT_ID from its immutable `rootCommentId` and pipe only that entry's `githubRequest` unchanged through stdin; keep its `operationId` for the result callback:
   gh api --hostname github.com --method POST "repos/$OWNER/$REPO/pulls/$NUMBER/comments/$ROOT_ID/replies" --input -
- Never convert a failed reply or unanchorable inline item into a standalone comment. Track operation IDs. Preserve completed and unknown-outcome operations so retries cannot duplicate them.
- POST one structured succeeded or failed publication result, with `requestId`, created GitHub IDs/URLs, completed operations, unknown operations, or a safe error code/message, as JSON stdin to the event's `resultUrl` using `Authorization: Bearer <SESSION_SECRET>`.
- If an operation outcome is unknown, publication remains blocked. Reconcile that exact operation against GitHub without creating anything new, then POST a corrected result with the same requestId: mark it completed with its GitHub ID/URL if found, or remove it from unknownOperations only after proving it is safe for a reviewer-driven retry.
- On failure, wait for a reviewer-driven retry; do not publish automatically. On success, stop when `session.done` is received.
"##
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_delegate_only_summary_and_confirmed_publication() {
        let instructions = agent_instructions();
        for command in [
            GH_HEAD_RECHECK_COMMAND,
            GH_GROUPED_REVIEW_COMMAND,
            GH_REVIEW_REPLY_COMMAND,
        ] {
            assert!(instructions.contains(command), "missing command: {command}");
        }
        for command in [
            GH_AUTH_STATUS_COMMAND,
            GH_PR_VIEW_COMMAND,
            GH_PR_CLONE_COMMAND,
            GH_PR_HEAD_FETCH_COMMAND,
            GH_ISSUE_COMMENTS_COMMAND,
            GH_REVIEWS_COMMAND,
            GH_REVIEW_COMMENTS_COMMAND,
            GH_REVIEW_THREADS_COMMAND,
            GH_PR_DIFF_COMMAND,
        ] {
            assert!(
                !instructions.contains(command),
                "automatic import command leaked into agent instructions: {command}"
            );
        }
        assert!(instructions.contains("Discuss itself loads the PR"));
        assert!(instructions.contains("Do not fetch, clone, build an import bundle"));
        assert!(instructions.contains("Wait for `pr.imported`"));
        assert!(instructions.contains("private-first"));
        assert!(instructions.contains("<SESSION_SECRET>"));
        assert!(instructions.contains("stale_pr_head"));
        assert!(instructions.contains("--input -"));
    }
}
