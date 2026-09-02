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
pub const GH_REVIEW_THREADS_COMMAND: &str = r#"gh api --hostname github.com graphql --paginate -f owner="$OWNER" -f repo="$REPO" -F number="$NUMBER" -f query='query($owner:String!,$repo:String!,$number:Int!,$endCursor:String){repository(owner:$owner,name:$repo){pullRequest(number:$number){reviewThreads(first:100,after:$endCursor){nodes{id isResolved isOutdated comments(first:1){nodes{databaseId}}}pageInfo{hasNextPage endCursor}}}}}'"#;

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

/// Returns the complete private-first agent protocol for a PR session.
pub fn agent_instructions() -> &'static str {
    r##"GitHub pull-request review import and publication protocol

Security and privacy
- Use only the already-authenticated `gh` CLI. Do not request, print, persist, or POST a GitHub token to discuss.
- The session is private-first. Fetching and importing are read-only. Keep every new question, local thread, answer, summary, and draft local unless the reviewer explicitly includes it and confirms the final preview with OK.
- Never create a standalone PR comment. Issue comments and review summaries are linked, read-only context, not reply destinations.
- Publish a reply only when an imported review thread has a positive root comment database ID. Publish a new inline comment only when its file, side, and changed line resolve confidently. Keep binary, no-hunk, outdated, detached, or otherwise ambiguous targets unpublished and report why; never guess a destination or create a duplicate.
- Never interpolate review text, summary text, or reply text into shell flags. Pass exact structured JSON through stdin with `--input -`.

Environment placeholders
- PR_URL is the strict CLI URL.
- OWNER, REPO, and NUMBER come from that URL and must exactly match fetched metadata.
- BASE_REF, BASE_SHA, and HEAD_SHA come from the immutable metadata response. PR_REPO is a fresh temporary directory removed after a successful import.
- Import the final JSON bundle at <IMPORT_ENDPOINT> with `Authorization: Bearer <SESSION_SECRET>`. Treat both placeholders as values supplied by `session.started`; do not invent or log the secret.

Fetch and import
1. Verify authentication:
   gh auth status --hostname github.com
2. Fetch PR metadata once:
   gh pr view "$PR_URL" --json number,title,body,state,isDraft,author,baseRefName,baseRefOid,headRefName,headRefOid,url,files
3. Fetch all issue comments, review summaries, and review comments:
   gh api --hostname github.com --paginate --slurp "repos/$OWNER/$REPO/issues/$NUMBER/comments?per_page=100"
   gh api --hostname github.com --paginate --slurp "repos/$OWNER/$REPO/pulls/$NUMBER/reviews?per_page=100"
   gh api --hostname github.com --paginate --slurp "repos/$OWNER/$REPO/pulls/$NUMBER/comments?per_page=100"
4. Fetch GraphQL review-thread state and root database IDs, joining it to the complete REST review-comment data:
   gh api --hostname github.com graphql --paginate -f owner="$OWNER" -f repo="$REPO" -F number="$NUMBER" -f query='query($owner:String!,$repo:String!,$number:Int!,$endCursor:String){repository(owner:$owner,name:$repo){pullRequest(number:$number){reviewThreads(first:100,after:$endCursor){nodes{id isResolved isOutdated comments(first:1){nodes{databaseId}}}pageInfo{hasNextPage endCursor}}}}}'
5. Fetch immutable repository data through authenticated `gh`, verify both commits, then generate the complete aggregate diff exactly once with ten unchanged context lines:
   PR_REPO="$(mktemp -d)/repo"
   gh repo clone "github.com/$OWNER/$REPO" "$PR_REPO" --no-upstream -- --filter=blob:none --no-checkout
   git -C "$PR_REPO" fetch --no-tags origin "refs/pull/$NUMBER/head:refs/discuss/pr-head"
   test "$(git -C "$PR_REPO" rev-parse "$BASE_SHA^{commit}")" = "$BASE_SHA"
   test "$(git -C "$PR_REPO" rev-parse 'refs/discuss/pr-head^{commit}')" = "$HEAD_SHA"
   git -C "$PR_REPO" diff --no-color --no-ext-diff --no-textconv --find-renames --unified=10 "$BASE_SHA...$HEAD_SHA"

Capture that single `git diff` stdout as the aggregate diff. Do not run `gh pr diff`, refetch per file, or run a second diff. Record `contextLines: 10` and `contextSource: "git-unified-10"` in the import bundle. Remove the temporary clone after the import succeeds.

Build schemaVersion 1 JSON with exact PR identity and metadata, nonempty overviewMarkdown, diff context metadata, one complete aggregate `diff --git` block per changed file, all discussion entries, and seedThreads for resolvable review threads. Preserve GitHub IDs, node IDs, URLs, timestamps, paths, commit SHAs, line/start-line sides, resolved state, and outdated state. Split only the single fetched aggregate diff; do not refetch per file.

Generate overviewMarkdown as GitHub-Flavored Markdown with this concrete structure:
- `# PR #NUMBER: TITLE`
- Author, state/draft status, `BASE` and `HEAD` branches, the PR URL, and links to both the PR conversation and `/files` diff page.
- `## Description` followed by the unmodified PR body (or an explicit empty-description note).
- `## Existing discussion`, with separate chronological entries headed `### Issue comment — AUTHOR — TIMESTAMP`, `### Review summary — AUTHOR — TIMESTAMP`, or `### Review thread — AUTHOR — TIMESTAMP`. Preserve each body and add an `Open ... on GitHub` link using its original URL. Never flatten the three types together or drop IDs/targets from the structured discussion data.

The overview file ID is always `pr-overview`. The import response returns every concrete deterministic `pr-file-...` ID next to its changed-file key/path; retain that mapping for all later thread/file work. Each diff file is represented by its raw aggregate `diff --git` block. Discuss renders every hunk as a GitHub-colored `diff-<language>` fence: `anchorStart`/`anchorEnd` identify the hunk block and `lineRange` identifies rows within that fence. Imported resolvable review threads use `gh-review-thread-{rootCommentId}` and retain the root comment ID as their immutable reply target; their GitHub URL remains visible as the escape hatch. Unresolvable discussion stays linked in the overview and must not become a guessed local publication target.

The import body must use this exact camelCase shape (repeat files/discussions/seedThreads as needed; use JSON null, not omitted keys, for nullable fields):
{
  "schemaVersion": 1,
  "importId": "OWNER/REPO#NUMBER@HEAD_SHA",
  "pr": {
    "owner": "OWNER", "repo": "REPO", "number": NUMBER, "url": "PR_URL",
    "title": "...", "body": "...", "state": "OPEN|CLOSED|MERGED", "isDraft": false,
    "author": {"login": "...", "url": "https://github.com/..."},
    "base": {"ref": "BASE_REF", "sha": "40_HEX_SHA"},
    "head": {"ref": "HEAD_REF", "sha": "40_HEX_SHA"}
  },
  "overviewMarkdown": "# PR ...",
  "diff": {"contextLines": 10, "contextSource": "git-unified-10"},
  "files": [{
    "key": "unique changed-path key", "oldPath": "old/path", "newPath": "new/path",
    "status": "modified|added|deleted|renamed", "binary": false,
    "diff": "one complete diff --git block including its headers and all hunks"
  }],
  "discussions": [{
    "id": "stable typed GitHub id", "kind": "issueComment|reviewSummary|reviewThread",
    "author": {"login": "...", "url": "https://github.com/..."},
    "createdAt": "RFC3339", "url": "original GitHub URL", "body": "...",
    "reviewId": null, "rootCommentId": null, "resolved": null, "outdated": null,
    "comments": [{
      "id": 123, "author": {"login": "...", "url": "https://github.com/..."},
      "createdAt": "RFC3339", "url": "original GitHub URL", "body": "..."
    }]
  }],
  "seedThreads": [{
    "discussionId": "matching reviewThread id", "fileKey": "matching file key",
    "rootCommentId": 123, "githubThreadNodeId": "PRRT_...", "path": "path",
    "line": 42, "side": "RIGHT|LEFT", "startLine": null, "startSide": null,
    "commitId": "40_HEX_SHA", "body": "...", "url": "original GitHub URL",
    "author": {"login": "...", "url": "https://github.com/..."},
    "createdAt": "RFC3339", "resolved": false, "outdated": false
  }]
}
For added/deleted files, keep the real git header paths rather than `/dev/null`; use status plus the hunk headers to convey addition/deletion. Mode-only and binary files still need their one aggregate `diff --git` block and remain unanchorable. `seedThreads` contains review threads only; issue comments and review summaries have no reply endpoint in v1.

POST the bundle as JSON stdin, not a shell-expanded argument:
   curl --fail-with-body --request POST --header 'Content-Type: application/json' --header 'Authorization: Bearer <SESSION_SECRET>' --data-binary @- '<IMPORT_ENDPOINT>'

Prepare and summary
- The browser starts prepare. On `pr.summary.requested`, generate only the requested editable review summary from the supplied local conversations.
- POST `{ "requestId": "...", "summary": "..." }` as JSON stdin to the event's `summaryUrl`, using the session bearer secret.
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
    fn instructions_include_exact_fetch_and_publish_contract() {
        let instructions = agent_instructions();
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
            GH_HEAD_RECHECK_COMMAND,
            GH_GROUPED_REVIEW_COMMAND,
            GH_REVIEW_REPLY_COMMAND,
        ] {
            assert!(instructions.contains(command), "missing command: {command}");
        }
        assert!(GH_PR_DIFF_COMMAND.contains("--unified=10"));
        assert_eq!(instructions.matches(GH_PR_DIFF_COMMAND).count(), 1);
        assert!(!instructions.contains("gh pr diff \"$PR_URL\""));
        assert!(instructions.contains("complete aggregate diff"));
        assert!(instructions.contains("--unified=10"));
        assert!(instructions.contains("contextSource: \"git-unified-10\""));
        assert!(instructions.contains("<IMPORT_ENDPOINT>"));
        assert!(instructions.contains("<SESSION_SECRET>"));
        assert!(instructions.contains("private-first"));
        assert!(instructions.contains("# PR #NUMBER: TITLE"));
        assert!(instructions.contains("### Issue comment — AUTHOR — TIMESTAMP"));
        assert!(instructions.contains("overview file ID is always `pr-overview`"));
        assert!(instructions.contains("lineRange"));
        assert!(instructions.contains("stale_pr_head"));
        assert!(instructions.contains("--input -"));
    }
}
