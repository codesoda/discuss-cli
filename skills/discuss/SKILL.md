---
name: discuss
description: Launch the discuss CLI on markdown, diff, image, or HTML prototype files (or piped markdown) through a monitor-type background tool, stream its event log, and participate by posting "takes" on threads the user opens.
allowed-tools: Bash, Monitor, TaskStop, monitor_start, monitor_stop, Read, ToolSearch
---

# discuss — Interactive review session

Open markdown, diffs, images, or HTML prototypes in `discuss`, watch the user drop comments and replies, and respond with *takes* — the agent's view on each question or thread. Takes are semantically distinct from replies: the human types replies in the browser; the agent posts takes via the API.

The source can be either a file on disk or markdown piped in on stdin (e.g. an ad-hoc summary of a staged diff that the agent generates and pipes straight into discuss without writing to disk).

## Arguments

- `$ARGUMENTS` — A path (or paths) to Markdown, diff, image, or HTML prototype files, OR Markdown content to review without writing it to disk. If missing and the user has not described the content, ask which file/content and stop.

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

### Image review mode

Pass an image path directly, alone or mixed with text files:

```
discuss mockup.png
discuss plan.md mockup.png
```

PNG, JPEG, GIF, WebP, and SVG files render through an `<img>` element. The reviewer drops numbered pins; image threads carry `imageAnchor: {xPct, yPct}` in basis points (`4200` = 42.00%) and use the pin number in `anchorStart`/`anchorEnd`. On `thread.created`, read the image from the event's `fileId` path in `/api/state.files`, inspect the pinned region at those percentage coordinates, and post takes through the same `/api/threads/{id}/takes` endpoint. `POST /api/source` is not supported for image files. Unsubmitted pin text is local-only and is lost on reload in this first version.

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

### HTML prototype mode

Pass `.html` or `.htm` files directly:

```
discuss prototype.html
```

The browser renders each prototype in a sandboxed iframe and offers **Inspect** mode. HTML `thread.created` events carry `elementAnchor` with `selector`, ordered `fallbacks`, `tag`, optional `textDigest`, and a truncated `outerHtml` snippet. Use `breadcrumb` and `snippet` for readable context; inspect `outerHtml` when the visual element is ambiguous. Agent takes still use `POST /api/threads/{id}/takes`.

Prototype-relative assets are served from the HTML file's directory. Root-absolute URLs are not rewritten. `POST /api/source` and live file watching are not supported for HTML files in this version.

### Verdict options

When the user wants an explicit final review decision (for example, "review this plan, then tell me approved or declined"), pass `--verdict-options` before any `diff` subcommand. Keep it to 2-4 short labels; decline/blocker-style options should usually require feedback with `!` so the transcript explains why.

DSL grammar: `id[:label][:style][!]` separated by `|`.

- `id` is required and must match `[a-z0-9_-]+`; it becomes `optionId`.
- `label` defaults to the title-cased id.
- `style` is `positive`, `neutral`, or `negative`; default is `neutral`.
- trailing `!` makes feedback required for that option.
- specs need at least 2 options; duplicate ids and case-insensitive duplicate labels are rejected with exit code 2.

```
discuss --verdict-options 'approved:Approve|declined:Decline:negative!' plan.md
```

Use `--verdict-prompt "..."` only when the default finish-review prompt needs project-specific wording; without `--verdict-options` it warns on stderr and does nothing. Shell-quote the options because the DSL uses `|` between choices and `!` for required feedback, and both are shell metacharacters.

When `session.done` arrives, read `payload.verdict` if present: `optionId` is the stable choice id, `label` is the displayed button text, `feedback` is the human explanation when supplied, and `decidedAt` is the decision timestamp.

## Preflight: Ensure `discuss` is installed

Run `command -v discuss` (via Bash). If it resolves to a path, skip ahead to Step 0.

If it doesn't resolve, the binary isn't on PATH. Ask the user:

> `discuss` isn't on your PATH. Install it now? (runs `curl -sSL https://raw.githubusercontent.com/codesoda/discuss-cli/main/install.sh | sh`)

On yes, run the install command via Bash. On completion, retry `command -v discuss`.

If it still doesn't resolve, fall back to the absolute install path: `~/.discuss/bin/discuss`. Check it exists and is executable — if so, use that path for every subsequent call to `discuss` in this session. If it also doesn't exist, report the install failed and stop.

If the user declines the install, stop.

## Step 0: Find the monitor-type tool

**discuss must be launched through a monitor-type tool whenever one exists.** A monitor-type tool is any primitive that (a) runs a long-lived command in the background and (b) delivers each stdout line back to you as a notification. That is exactly the contract discuss is built for: the process stays up for the whole review, and every newline-delimited JSON event it prints wakes you with the user's latest thread or reply. No polling, no log scraping, no blocked turn.

Harnesses name these tools differently. Look for whichever pair exists in the current context:

| Harness | Launch | Stop |
|---|---|---|
| Claude Code | `Monitor` | `TaskStop` |
| pi | `monitor_start` | `monitor_stop` |
| other | any "run in background + stream stdout to me" tool | its stop/kill call |

In Claude Code, `Monitor` and `TaskStop` may be deferred tools. Load their schemas before calling them:

```
ToolSearch(query: "select:Monitor,TaskStop", max_results: 2)
```

## Step 1: Launch discuss and choose an event strategy

Always launch through the monitor-type tool first. Only if no such tool is available in the current context (e.g. ToolSearch finds nothing and invoking it returns a tool-not-enabled error) fall back to the **polling fallback** described below. Do not use the poller when a monitor-type tool exists — it delivers events push-style with no polling latency. The rest of the steps are the same once you have events flowing.

### Option A — monitor-type tool (preferred)

Run `discuss` directly as the monitor's command. Two things NOT to do:

- Do NOT launch it with a plain blocking Bash call — discuss is a server that runs until the user finishes the review, so the call would hold your turn hostage for the entire session.
- Do NOT launch it via Bash with `run_in_background` — you would then have to poll a log file for events, which is the thing the monitor exists to avoid.

The monitor treats each stdout line from its command as an event notification delivered to chat, which is exactly how discuss's newline-delimited JSON events are meant to be consumed.

**Never pass `--no-open`** — the browser must open by default; the human reviews there. If a session seems to have started silently, check the flags before assuming a server problem.

**The command string must start with `discuss`** — commands beginning with `discuss` are pre-approved and start immediately; any prefix (`cd … && discuss`, `VAR=x discuss`, `git … | discuss`) requires human approval before the monitor can start. Never prefix with `cd`: the monitor already runs in the session's working directory, so launch from the right cwd and pass repo-relative or absolute paths instead. When piping content in, prefer the heredoc form (`discuss - <<'EOF' … EOF`, which starts with `discuss`) over an upstream-command pipe.

**File mode** (Claude Code):

```
Monitor(
  description: "discuss events for <file>",
  command: "discuss \"$ARGUMENTS\"",
  persistent: true
)
```

The same launch in pi — same command string, same `persistent`, plus an `instruction` that rides along with every wake-up:

```
monitor_start(
  description: "discuss events for <file>",
  command: "discuss \"$ARGUMENTS\"",
  persistent: true,
  instruction: "Each line is a discuss event. Post a take on thread.created and a follow-up take on reply.added, per the discuss skill."
)
```

**Stdin mode** — pipe the markdown content into `discuss -`. Use a heredoc to keep the content readable in the monitor command:

```
Monitor(
  description: "discuss events for staged-diff review",
  command: "discuss - <<'DISCUSS_EOF'\n# Staged Diff Review\n\n## src/foo.rs\n\n... markdown body ...\nDISCUSS_EOF",
  persistent: true
)
```

Avoid piping another command's output in (`git diff … | discuss -`) — the command no longer starts with `discuss` and needs human approval. For diffs use `discuss diff` (starts with `discuss`, pre-approved); otherwise capture the content first and use the heredoc form.

Notes:

- `persistent: true` is required — discuss is a long-running server that only exits when the user is done. Without it the monitor will time out mid-review and take discuss down with it.
- Do NOT redirect stderr. Monitor-type tools keep stderr out of the event stream (Claude Code writes it to the task output file, pi to a temp log), so discuss's `listening on …` stderr line can't pollute the JSON events — but `2>&1` would fold it in.
- Record the id returned by the launch call (`task_id` from Monitor, monitor id from `monitor_start`) — you need it to stop the session later.
- If the port is already bound or the file doesn't exist, discuss exits immediately and the monitor ends without ever emitting a `session.started` event. Read the monitor's stderr log to surface the error, then stop.
- In stdin mode, you typically already have the markdown in hand (you generated it). Keep a copy in your scratchpad if you need it later for anchor snippets — there's no file to re-read.

### Option B — Polling fallback (only when no monitor-type tool is available)

Use this only when no monitor-type background tool is enabled in the current context. If one is available under any name, use Option A.

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

Handle every `thread.created` and `thread.updated` line exactly as you would `thread.created` and `reply.added` monitor events (see Step 3). On exit 2 the last line is `{"event": "session.done"}` — treat it as the signal to stop and summarize.

**Baseline handling:** always pass the `baseline` object from the `snapshot` line to the next poller invocation — do NOT re-fetch state to rebuild it yourself, or events that arrive in between will be silently dropped. If you post a reply or take while handling an event, bump that thread's count in the baseline first so your own post doesn't re-fire:

```bash
BASELINE=$(echo "$BASELINE" | jq -c --arg id "$THREAD_ID" '.[$id] += 1')
```

Optionally `Read` the markdown source afterward for context on anchor snippets (file mode only).

## Step 2: Confirm startup and capture URL

The first notification from the monitor should be a `session.started` event:

```json
{"kind":"session.started","at":"...","payload":{"url":"http://127.0.0.1:<port>","apiBaseUrl":"http://127.0.0.1:<port>","endpoints":{"state":"http://127.0.0.1:<port>/api/state","events":"http://127.0.0.1:<port>/api/events","createThread":"http://127.0.0.1:<port>/api/threads","addTakeTemplate":"http://127.0.0.1:<port>/api/threads/{threadId}/takes","done":"http://127.0.0.1:<port>/api/done"},"agentInstructions":["Use payload.endpoints; do not assume port 7777.","On thread.created, POST a take to addTakeTemplate with {threadId} replaced.","Stop when session.done is received."],"mode":"markdown","source_file":"...","files_count":1,"started_at":"..."}}
```

Parse `url` from the payload — **use this URL for every subsequent API call**. The port is configurable (`--port`, config file), so don't hardcode `7777`.

If the monitor ends without emitting `session.started`, discuss failed to start. Read its stderr log for the error, report it, and stop.

Post a short message to chat:

> Session open at `<url>` — watching for threads. Anchor a comment on any part of the doc and I'll post a take.

## Step 3: Event loop

Notifications arrive on the monitor's own schedule — you don't poll. Each notification line is one JSON event. Takes and drafts are broadcast via SSE only (not stdout), so your own `/takes` writes never echo back — no self-echo tracking needed.

Actionable events: `thread.created`, `reply.added`, `thread.resolved`, `thread.deleted`. Lifecycle events (`session.started`, `session.done`, `thread.unresolved`, `prompt.suggest_done`) are informational — acknowledge in chat if useful but don't post to the API.

### `thread.created` (new thread opened by the user)

1. Read `anchorStart`, `anchorEnd`, `snippet`, `text`, and optional `imageAnchor` / `elementAnchor` from the payload.
2. For markdown/diff threads, locate the anchored region using `snippet`. For image threads, resolve `fileId` through `/api/state.files` and inspect `imageAnchor`'s percentage coordinates. For HTML threads, use `breadcrumb`, `elementAnchor.selector`, and `elementAnchor.outerHtml` to identify the reviewed DOM element.
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
- The monitored task exits on its own (user quit the browser, server crashed, `session.done` event arrived). No further notifications will arrive.
- The user starts a new unrelated task — don't linger.

On stop:

1. Stop the monitored task so discuss shuts down with it — `TaskStop(task_id: <id>)` in Claude Code, `monitor_stop(id: <id>)` in pi.
2. Summarize: each thread, a one-line takeaway, resolution state.

## API reference

All endpoints at the `url` from `session.started`. Request/response is JSON.

| Method | Path | Body | Purpose |
|---|---|---|---|
| GET | `/api/state` | — | Full snapshot: threads, replies, takes, drafts, verdictConfig |
| GET | `/api/events` | — | SSE stream (alternative to stdout) |
| GET | `/api/files/{fileId}/raw` | — | Startup-stable bytes for an image file |
| GET | `/files/{fileId}` | — | Served HTML prototype document |
| POST | `/api/anchors/resolve` | `{fileId?, detachedThreadIds}` | Browser reports detached HTML anchors |
| POST | `/api/threads` | Text: `{fileId?, anchorStart, anchorEnd, snippet, text}`; image adds `{imageAnchor}`; HTML adds `{breadcrumb, elementAnchor}` and uses zero numeric anchors | Create a thread. Rare — usually the user does this. `fileId` required with multiple files. |
| DELETE | `/api/threads/{id}` | — | Soft delete (`kind="user"` only; prepopulated returns 403) |
| POST | `/api/threads/{id}/replies` | `{text}` | **Human** reply. Do NOT use as the agent. |
| POST | `/api/threads/{id}/takes` | `{text}` | **Agent** take. This is your primary tool. |
| POST | `/api/threads/{id}/resolve` | `{decision?}` | Resolve a thread |
| POST | `/api/threads/{id}/unresolve` | — | Unresolve |
| POST | `/api/source` | `{markdown, fileId?, threadAnchors}` | Live source update with re-anchoring (see below) |
| POST | `/api/done` | `{verdict: {optionId, feedback?}}` when verdict options are configured; otherwise optional/ignored | Finish the review. With verdict options, missing body is `400 bad_request`; unknown `optionId` or missing required feedback is `400 validation_error`. |

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

- `session.started` → `{url, apiBaseUrl, proxyUrl?, endpoints: {state, events, createThread, addTakeTemplate, done}, agentInstructions, mode, source_file, files_count, started_at, git_args?}`
- `session.done` → final transcript payload with optional `verdict: {optionId, label, feedback?, decidedAt}`
- `thread.created` → `{id, fileId, kind, anchorStart, anchorEnd, imageAnchor?, elementAnchor?, snippet, text, breadcrumb, createdAt}`; image breadcrumbs identify pin coordinates, while HTML anchors include selector fallbacks and `outerHtml` context
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
