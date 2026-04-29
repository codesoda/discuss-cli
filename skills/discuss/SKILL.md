---
name: discuss
description: Launch the discuss CLI on a markdown file (or piped stdin), stream the event log via Monitor, and participate in the review by posting "takes" (agent views) on threads the user opens. Use when invoked as /discuss <markdown-path> or when the user wants to review markdown content piped from another command.
allowed-tools: Bash, Monitor, TaskStop, Read, ToolSearch
---

# discuss — Interactive markdown review session

Open markdown content (or a git diff) in `discuss`, watch the user drop comments and replies, and respond with *takes* — the agent's view on each question or thread. Takes are semantically distinct from replies: the human types replies in the browser; the agent posts takes via the API.

The source can be:

- A markdown file on disk (`discuss plan.md`)
- Multiple markdown files in one session (`discuss plan.md design.md notes.md`) — each renders in one scrollable column with a left-rail file tree
- Markdown piped in on stdin (`<cmd> | discuss` or `discuss -`)
- **A git diff via the built-in subcommand** — `discuss diff` (staged), `discuss diff --unstaged`, or `discuss diff <range>`. Use this instead of regenerating the diff inside markdown — it's the supported path and avoids prompt drift / wasted tokens.

## Arguments

- `$ARGUMENTS` — Either a path to the markdown file to review, OR markdown content the user wants to review without writing it to disk. If missing and the user has not described the content, ask which file/content and stop.

### Stdin mode

When you have markdown content already in hand (e.g. a generated summary of staged changes) and don't need it on disk, pipe it in instead of writing a temp file:

- `discuss -` reads markdown from stdin explicitly.
- `<some-command> | discuss` also reads stdin (auto-detected when no file arg is given and stdin is not a TTY).

In stdin mode, the `session.started` event reports `source_file: "<stdin>"` and history archives are written under `.../unnamed/` since there is no source path to derive a folder name from.

### Diff review mode

When the user wants to review a git diff, use the `discuss diff` subcommand directly instead of generating a markdown wrapper. Each changed file becomes a `kind: "diff"` file in the session, the v2 UI (open with `?ui=v2`) renders one Prism-highlighted block per hunk, and threads anchor at hunk granularity.

```
Monitor(
  description: "discuss diff (staged) review",
  command: "discuss diff",
  persistent: true
)
```

Variants:

- `discuss diff` — staged diff (`git diff --cached`); this is the default.
- `discuss diff --unstaged` — working tree.
- `discuss diff HEAD~3..HEAD` (or `main...feature`, single commit, etc.) — any range arg is forwarded to `git diff`.

In diff mode the `session.started` payload looks like:

```json
{"kind":"session.started","at":"...","payload":{"url":"http://127.0.0.1:7777","mode":"diff","source_file":"git diff --no-color --no-ext-diff --cached","files_count":4,"git_args":["diff","--no-color","--no-ext-diff","--cached"],"started_at":"..."}}
```

If the working tree / index is clean, `discuss diff` exits before binding with `diff error: no changes to review` — Monitor ends without a `session.started` event. Read the Monitor output file to surface that error and stop.

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

## Step 1: Launch as a persistent Monitor

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

Optionally `Read` the markdown source afterward for context on anchor snippets (file mode only).

## Step 2: Confirm startup and capture URL

The first Monitor notification should be a `session.started` event:

```json
{"kind":"session.started","at":"...","payload":{"url":"http://127.0.0.1:<port>","mode":"markdown","source_file":"...","files_count":1,"started_at":"..."}}
```

`mode` is `"markdown"` for file / stdin / multi-file sessions and `"diff"` for `discuss diff` (which adds a `git_args` array). `files_count` is the number of files in the session.

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
| POST | `/api/threads` | `{anchorStart, anchorEnd, snippet, text}` | Create a thread. Rare — usually the user does this. |
| DELETE | `/api/threads/{id}` | — | Soft delete (`kind="user"` only; prepopulated returns 403) |
| POST | `/api/threads/{id}/replies` | `{text}` | **Human** reply. Do NOT use as the agent. |
| POST | `/api/threads/{id}/takes` | `{text}` | **Agent** take. This is your primary tool. |
| POST | `/api/threads/{id}/resolve` | `{decision?}` | Resolve a thread |
| POST | `/api/threads/{id}/unresolve` | — | Unresolve |

## Stdout event kinds

- `session.started` → `{url, mode, source_file, files_count, started_at}` (plus `git_args` in diff mode)
- `session.done` → `{}` — emitted when discuss exits cleanly
- `thread.created` → `{id, kind, anchorStart, anchorEnd, snippet, text, breadcrumb, createdAt}`
- `thread.resolved` → `{threadId, resolution: {decision, resolvedAt}}`
- `thread.unresolved` → `{threadId}`
- `thread.deleted` → `{threadId}`
- `reply.added` → `{id, threadId, text, createdAt}` — human reply
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
