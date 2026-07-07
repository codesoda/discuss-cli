---
name: discuss
description: Launch the discuss CLI on a markdown file (or piped stdin), stream the event log via Monitor, and participate in the review by posting "takes" (agent views) on threads the user opens. Use when invoked as /discuss <markdown-path> or when the user wants to review markdown content piped from another command.
allowed-tools: Bash, Monitor, TaskStop, Read, ToolSearch
---

# discuss — Interactive markdown review session

Open markdown content in `discuss`, watch the user drop comments and replies, and respond with *takes* — the agent's view on each question or thread. Takes are semantically distinct from replies: the human types replies in the browser; the agent posts takes via the API.

The source can be either a file on disk or markdown piped in on stdin (e.g. an ad-hoc summary of a staged diff that the agent generates and pipes straight into discuss without writing to disk).

## Arguments

- `$ARGUMENTS` — Either a path to the markdown file to review, OR markdown content the user wants to review without writing it to disk. If missing and the user has not described the content, ask which file/content and stop.

### Stdin mode

When you have markdown content already in hand (e.g. a generated summary of staged changes) and don't need it on disk, pipe it in instead of writing a temp file:

- `discuss -` reads markdown from stdin explicitly.
- `<some-command> | discuss` also reads stdin (auto-detected when no file arg is given and stdin is not a TTY).

In stdin mode, the `session.started` event reports `source_file: "<stdin>"` and history archives are written under `.../unnamed/` since there is no source path to derive a folder name from.

### Multi-file mode

Pass several paths to review them together in one session with a file sidebar:

```
discuss plan.md design.md notes.md
```

- Files are identified by `fileId` (`f-1`, `f-2`, … in CLI order). `/api/state` includes a `files` array (`{id, path, kind}`).
- Every `thread.created` payload carries a `fileId`. When you create threads or push source updates in a multi-file session, `fileId` is **required** — omitting it returns `400 missing_file_id`.
- Anchor indices are per-file (1-based commentable blocks within that file's document).
- `session.started` gains `files_count`, and `source_file` becomes `multi-<N>-files`.

### Diff review mode

**Prefer `discuss diff` over generating a markdown wrapper of a git diff.** It skips the summarize-and-fence round trip entirely — the binary runs `git diff`, splits it per file, and renders each hunk as a `diff-<lang>` block with line-anchored threads working out of the box:

```
discuss diff                  # staged (git diff --cached)
discuss diff --unstaged       # working tree
discuss diff HEAD~3..HEAD     # arbitrary range forwarded to git diff
discuss plan.md diff          # markdown file(s) + diff in one session
```

- `session.started` gains `mode` (`"markdown"` / `"diff"` / `"mixed"`) and `git_args` so you know what's under review.
- Each changed file is its own sidebar entry with its own `fileId`; per-file prose is optional — post takes on file threads when intent needs explaining, stay silent on mechanical changes.
- Diff output is capped at 5 MB (`--max-diff-bytes` / `DISCUSS_MAX_DIFF_BYTES` / `max_diff_bytes` config to override; `0` disables).

## Preflight: Ensure `discuss` is installed

Run `command -v discuss` (via Bash). If it resolves to a path, skip ahead to Step 0.

If it doesn't resolve, the binary isn't on PATH. Ask the user:

> `discuss` isn't on your PATH. Install it now? (runs `curl -sSL https://raw.githubusercontent.com/codesoda/discuss-cli/main/install.sh | sh`)

On yes, run the install command via Bash. On completion, retry `command -v discuss`.

If it still doesn't resolve, fall back to the absolute install path: `~/.discuss/bin/discuss`. Check it exists and is executable — if so, use that path for every subsequent call to `discuss` in this session. If it also doesn't exist, report the install failed and stop.

If the user declines the install, stop.

## Step 0: Load deferred tool schemas

`Monitor` and `TaskStop` may be deferred tools. Before calling them, load their schemas:

```
ToolSearch(query: "select:Monitor,TaskStop", max_results: 2)
```

## Step 1: Launch discuss and choose an event strategy

Always try Monitor (or an equivalent background-monitoring tool) first. Only if no such tool is available in the current context (e.g. ToolSearch finds nothing and invoking Monitor returns a tool-not-enabled error) fall back to the **polling fallback** described below. Do not use the poller when a monitor-type tool exists — Monitor delivers events push-style with no polling latency. The rest of the steps are the same once you have events flowing.

### Option A — Monitor (preferred)

Run `discuss` directly as the Monitor command — do NOT launch it via Bash with `run_in_background`. Monitor treats each stdout line from its command as an event notification delivered to chat, which is exactly how discuss's newline-delimited JSON events are meant to be consumed.

**File mode:**

```
Monitor(
  description: "discuss events for <file>",
  command: "discuss \"$ARGUMENTS\"",
  persistent: true
)
```

**Stdin mode** — pipe the markdown content into `discuss -`. Use a heredoc to keep the content readable in the Monitor command:

```
Monitor(
  description: "discuss events for staged-diff review",
  command: "discuss - <<'DISCUSS_EOF'\n# Staged Diff Review\n\n## src/foo.rs\n\n... markdown body ...\nDISCUSS_EOF",
  persistent: true
)
```

Or pipe the output of another command:

```
Monitor(
  description: "discuss events for staged-diff review",
  command: "git diff --cached -U10 | render-as-markdown | discuss -",
  persistent: true
)
```

Notes:

- `persistent: true` is required — discuss is a long-running server that only exits when the user is done.
- Do NOT redirect stderr. Monitor sends stderr to the output file (readable via Read) and it never triggers notifications, so discuss's `listening on …` stderr line can't pollute the event stream.
- Record the `task_id` returned by Monitor — you'll need it for `TaskStop` later.
- If the port is already bound or the file doesn't exist, discuss exits immediately and Monitor ends without ever emitting a `session.started` event. Read the Monitor output file to surface the error, then stop.
- In stdin mode, you typically already have the markdown in hand (you generated it). Keep a copy in your scratchpad if you need it later for anchor snippets — there's no file to re-read.

### Option B — Polling fallback (only when no monitor-type tool is available)

Use this only when no Monitor-type background monitoring tool is enabled in the current context. If Monitor (or equivalent) is available, use Option A.

**1. Start discuss in the background:**

```bash
discuss "$ARGUMENTS" --port <port> > /tmp/discuss-startup.log 2>&1 &
sleep 2
curl -s http://127.0.0.1:<port>/api/state | jq -e 'has("threads")' > /dev/null \
  || { cat /tmp/discuss-startup.log; exit 1; }
```

Pick a free port by checking which of 7777–7782 isn't already bound (`curl -s http://127.0.0.1:<port>/api/state`). If all are in use, discuss is already running — attach to the existing one.

**2. Enter the event loop — blocking poller:**

This skill's directory (the directory containing this SKILL.md) also contains `poller.sh`. Call it via Bash (blocking, timeout 600000ms). It polls `/api/state` every 5 seconds and exits as soon as something changes:

```bash
bash <skill-dir>/poller.sh "http://127.0.0.1:<port>"
```

On the first invocation, pass no baseline — the poller snapshots current state itself. On every subsequent invocation, pass the baseline captured from the previous run's `snapshot` line (see below).

- Exit 0 → one or more new events; parse stdout (one JSON object per line), handle each, then **immediately re-invoke the poller** with the new baseline.
- Exit 1 → error (API unreachable); report to user and stop.
- Exit 2 → session ended (discuss exited); summarize threads and stop.
- Bash tool timeout → not an error; the session is just quiet. Re-invoke the poller with the same baseline.

**3. Handling events from the poller:**

On exit 0, stdout contains one line per changed thread, followed by a final `snapshot` line:

```json
{"event": "thread.created", "thread": { ...full thread object... }}
{"event": "thread.updated", "thread": { ...full thread object... }, "prev_count": 1, "current_count": 2}
{"event": "snapshot", "baseline": {"<thread-id>": 2, "<thread-id>": 0}}
```

Handle every `thread.created` and `thread.updated` line exactly as you would `thread.created` and `reply.added` Monitor events (see Step 3). On exit 2 the last line is `{"event": "session.done"}` — treat it as the signal to stop and summarize.

**Baseline handling:** always pass the `baseline` object from the `snapshot` line to the next poller invocation — do NOT re-fetch state to rebuild it yourself, or events that arrive in between will be silently dropped. If you post a reply or take while handling an event, bump that thread's count in the baseline first so your own post doesn't re-fire:

```bash
BASELINE=$(echo "$BASELINE" | jq -c --arg id "$THREAD_ID" '.[$id] += 1')
```

Optionally `Read` the markdown source afterward for context on anchor snippets (file mode only).

## Step 2: Confirm startup and capture URL

The first Monitor notification should be a `session.started` event:

```json
{"kind":"session.started","at":"...","payload":{"url":"http://127.0.0.1:<port>","source_file":"...","started_at":"..."}}
```

Parse `url` from the payload — **use this URL for every subsequent API call**. The port is configurable (`--port`, config file), so don't hardcode `7777`.

If Monitor ends without emitting `session.started`, discuss failed to start. Read the Monitor output file for the stderr error, report it, and stop.

Post a short message to chat:

> Session open at `<url>` — watching for threads. Anchor a comment on any part of the doc and I'll post a take.

## Step 3: Event loop

Monitor notifications arrive on their own schedule — you don't poll. Each notification line is one JSON event. Takes and drafts are broadcast via SSE only (not stdout), so your own `/takes` writes never echo back — no self-echo tracking needed.

Actionable events: `thread.created`, `reply.added`, `thread.resolved`, `thread.deleted`. Lifecycle events (`session.started`, `session.done`, `thread.unresolved`, `prompt.suggest_done`) are informational — acknowledge in chat if useful but don't post to the API.

### `thread.created` (new thread opened by the user)

1. Read `anchorStart`, `anchorEnd`, `snippet`, `text` from the payload.
2. Locate the anchored region in the markdown source — the `snippet` is a reliable search key for the rendered paragraph.
3. Read the user's comment in `text`.
4. Form a substantive take — answer the question, critique the anchored text, or add the missing piece. Be specific. Reference the anchored content, not just the question in isolation.
5. Post it as a **take**, not a reply (substitute the URL from `session.started`):

```bash
curl -s -X POST "$URL/api/threads/<thread-id>/takes" \
  -H 'Content-Type: application/json' \
  -d '{"text":"..."}'
```

### `reply.added` (the user replied in a thread)

Replies come only from the human (the API uses `/replies` for humans, `/takes` for you). Any `reply.added` event is a new user message.

1. Fetch full state: `curl -s "$URL/api/state"` — parse the thread and all its replies/takes in order.
2. Read the latest reply in context.
3. Decide: is this a question, a challenge, or a genuine opening for more commentary? If yes, post a follow-up take. If it's closure ("thanks", "got it", "makes sense"), stay silent.
4. If responding, POST another take to the same thread.

### `thread.resolved` / `thread.deleted`

Acknowledge in chat ("`u-3` resolved" / "`u-2` deleted") but do not post anything to the thread.

## Step 4: Stop conditions

End the session and shut down when any of these happen:

- The user types "stop", "end session", "kill it", or similar in chat.
- The Monitor task exits on its own (user quit the browser, server crashed, `session.done` event arrived). No further notifications will arrive.
- The user starts a new unrelated task — don't linger.

On stop:

1. Call `TaskStop(task_id: <monitor-task-id>)` to terminate the Monitor task (which in turn kills discuss).
2. Summarize: each thread, a one-line takeaway, resolution state.

## API reference

All endpoints at the `url` from `session.started`. Request/response is JSON.

| Method | Path | Body | Purpose |
|---|---|---|---|
| GET | `/api/state` | — | Full snapshot: threads, replies, takes, drafts |
| GET | `/api/events` | — | SSE stream (alternative to stdout) |
| POST | `/api/threads` | `{fileId?, anchorStart, anchorEnd, snippet, text}` | Create a thread. Rare — usually the user does this. `fileId` required with multiple files. |
| DELETE | `/api/threads/{id}` | — | Soft delete (`kind="user"` only; prepopulated returns 403) |
| POST | `/api/threads/{id}/replies` | `{text}` | **Human** reply. Do NOT use as the agent. |
| POST | `/api/threads/{id}/takes` | `{text}` | **Agent** take. This is your primary tool. |
| POST | `/api/threads/{id}/resolve` | `{decision?}` | Resolve a thread |
| POST | `/api/threads/{id}/unresolve` | — | Unresolve |
| POST | `/api/source` | `{markdown, fileId?, threadAnchors}` | Live source update with re-anchoring (see below) |

### Live source updates (`POST /api/source`)

If you regenerate the markdown mid-session (e.g. the user fixed code under review and you rebuilt the diff summary), push the new source into the running session instead of restarting it. You own the re-anchor decision: send the full new markdown plus one entry per **active** thread **on that file** — either its new anchor position or `"orphaned": true` if its content no longer exists. Coverage is strict and scoped per file; the request is rejected (and nothing changes) if any of that file's active threads is missing, or if you reference a thread from another file. In multi-file sessions pass `fileId`; single-file sessions default to the only file.

```json
{
  "markdown": "...entire new document...",
  "threadAnchors": [
    { "threadId": "u-1", "anchorStart": 4, "anchorEnd": 4, "snippet": "optional refreshed snippet" },
    { "threadId": "u-2", "orphaned": true }
  ]
}
```

Anchors are 1-based indices of commentable block elements (headings, paragraphs, list items, code blocks) in document order — the same units as `anchorStart` on `thread.created`. On success the server re-renders, bumps `sourceVersion` (visible in `/api/state`), and broadcasts `source.updated` on SSE and stdout; the browser swaps the document in place and keeps every conversation. Orphaned threads stay visible to the user, flagged as orphaned. You may pass `sourceVersion` when creating threads via `POST /api/threads` to get a `409 stale_source_version` instead of anchoring against a document that changed under you.

## Stdout event kinds

- `session.started` → `{url, mode, source_file, files_count, started_at, git_args?}`
- `session.done` → `{}` — emitted when discuss exits cleanly
- `thread.created` → `{id, fileId, kind, anchorStart, anchorEnd, snippet, text, breadcrumb, createdAt}`
- `thread.resolved` → `{threadId, resolution: {decision, resolvedAt}}`
- `thread.unresolved` → `{threadId}`
- `thread.deleted` → `{threadId}`
- `reply.added` → `{id, threadId, text, createdAt}` — human reply
- `source.updated` → `{markdown, fileId, renderedHtml, threadAnchors, orphanedThreadIds, sourceVersion}` — a live source update was applied (echo of your own `POST /api/source`, or another agent's)
- `prompt.suggest_done` → lifecycle; informational

**Not on stdout:** `take.added`, `draft.updated`, `draft.cleared` — these are SSE-only (browser UI), so they never surface here.

## Authoring markdown for syntax highlighting

When you generate the markdown to review (especially in stdin mode), tag every code fence with a language so the browser can highlight it. Untagged fences render as plain text.

**Common languages:** `rust`, `typescript`, `tsx`, `jsx`, `javascript`, `python`, `go`, `java`, `c`, `cpp`, `csharp`, `ruby`, `php`, `swift`, `kotlin`, `bash`, `shell`, `json`, `toml`, `yaml`, `markdown`, `html`, `css`, `scss`, `sql`, `hcl`, `dockerfile`, `nginx`, `ini`, `xml`, `regex`, `graphql`.

**Diffs:** use `diff` for plain diffs, or `diff-<language>` (e.g. `diff-rust`, `diff-typescript`) for language-aware highlighting on top of the +/- gutter.

**Anything else:** Prism supports ~300 languages. If you need one not listed above, check [prismjs.com/#supported-languages](https://prismjs.com/#supported-languages) — discuss loads grammars on demand. The list above is curated; the website is authoritative and may include languages added after this skill was written.

## Tone for takes

- Be specific to the anchored content, not generic.
- Push back when you disagree; don't flatter.
- Cite the source doc when relevant ("line 24 says X, but...").
- Short is better than long — one or two focused paragraphs beats an essay.
- If you don't know, say so. Don't speculate.
