---
name: discuss
description: Launch the discuss CLI on a markdown file, stream the event log via Monitor, and participate in the review by posting "takes" (agent views) on threads the user opens. Use when invoked as /discuss <markdown-path>.
allowed-tools: Bash, Monitor, Read, ToolSearch
---

# discuss — Interactive markdown review session

Open a markdown file in `discuss`, watch the user drop comments and replies, and respond with *takes* — the agent's view on each question or thread. Takes are semantically distinct from replies: the human types replies in the browser; the agent posts takes via the API.

## Arguments

- `$ARGUMENTS` — Path to the markdown file to review. Required. If missing, ask the user which file and stop.

## Preflight: Ensure `discuss` is installed

Run `command -v discuss` (via Bash). If it resolves to a path, skip ahead to Step 0.

If it doesn't resolve, the binary isn't on PATH. Ask the user:

> `discuss` isn't on your PATH. Install it now? (runs `curl -sSL https://raw.githubusercontent.com/codesoda/discuss-cli/main/install.sh | sh`)

On yes, run the install command via Bash. On completion, retry `command -v discuss`.

If it still doesn't resolve, fall back to the absolute install path: `~/.discuss/bin/discuss`. Check it exists and is executable — if so, use that path for every subsequent call to `discuss` in this session. If it also doesn't exist, report the install failed and stop.

If the user declines the install, stop.

## Step 0: Load deferred tool schemas

`Monitor` may be a deferred tool. Before calling it, load its schema:

```
ToolSearch(query: "select:Monitor", max_results: 1)
```

## Step 1: Launch

Start `discuss` as a background Bash task. Redirect stderr only — stdout (newline-delimited JSON events) must flow into the task output buffer so Monitor can stream it.

```bash
discuss "$ARGUMENTS" 2> /tmp/discuss-stderr.log
```

Use `run_in_background: true`. Record the returned task ID (e.g., `b3mvlm9a4`).

No further preflight — if the port is already bound or the file doesn't exist, discuss will exit with a clear error. Let that surface naturally and report it. Optionally `Read` the markdown source afterward for context on anchor snippets.

## Step 2: Confirm startup and capture URL

Call Monitor on the task ID and wait for the first line. It should be:

```json
{"kind":"session.started","at":"...","payload":{"url":"http://127.0.0.1:<port>","source_file":"...","started_at":"..."}}
```

Parse `url` from the payload — **use this URL for every subsequent API call**. The port is configurable (`--port`, config file), so don't hardcode `7777`.

If the first line is an error on stderr instead (bind failure, file not found), the task will exit. Report the failure and stop.

Post a short message to chat:

> Session open at `<url>` — watching for threads. Anchor a comment on any part of the doc and I'll post a take.

## Step 3: Event loop

Keep calling Monitor on the task. Each stdout line is one JSON event.

### Events to ignore

- `draft.updated` — fires per keystroke, pure noise. Discard immediately.
- `draft.cleared` — informational, no action needed.
- `take.added` or `thread.created` whose `id` is in your *self-posted set* (see Step 4).

### Events to act on

**`thread.created`** (new thread opened by the user)

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

6. Record the returned `id` (e.g., `t-1`) in your self-posted set.

**`reply.added`** (the user replied in a thread)

Replies come only from the human (the API uses `/replies` for humans, `/takes` for you). Any `reply.added` event is a new user message.

1. Fetch full state: `curl -s "$URL/api/state"` — parse the thread and all its replies/takes in order.
2. Read the latest reply in context.
3. Decide: is this a question, a challenge, or a genuine opening for more commentary? If yes, post a follow-up take. If it's closure ("thanks", "got it", "makes sense"), stay silent.
4. If responding, POST another take to the same thread and record the ID.

**`take.added`** (self-echo of your own post)

Your own POSTs to `/takes` are broadcast back to stdout. If the `id` is in your self-posted set, drop it. Otherwise it came from somewhere else — investigate, do not assume.

**`thread.resolved`** / **`thread.deleted`**

Acknowledge in chat ("`u-3` resolved" / "`u-2` deleted") but do not post anything to the thread.

## Step 4: Self-echo tracking

The server broadcasts every mutation to stdout regardless of source, so every take or thread you POST will echo back as an event. To avoid responding to yourself:

- Maintain an in-memory set of IDs you posted (`t-1`, `t-2`, ...).
- Add the `id` from each POST response to the set before you process the next Monitor event.
- On every `take.added` / `thread.created` / `reply.added` event, skip if `id` is in the set.

## Step 5: Stop conditions

End the session and shut down when any of these happen:

- The user types "stop", "end session", "kill it", or similar in chat.
- The Monitor-watched task exits (user Ctrl+C'd the terminal, browser quit, or `discuss` otherwise shut down). Monitor will return without a new line.
- The user starts a new unrelated task — don't linger.

On stop:

1. Kill the background task.
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

- `session.started` → `{url, source_file, started_at}`
- `thread.created` → `{id, kind, anchorStart, anchorEnd, snippet, text, breadcrumb, createdAt}`
- `reply.added` → `{id, threadId, text, createdAt}` — human reply
- `take.added` → `{id, threadId, text, createdAt}` — agent take (possibly your own echo)
- `thread.resolved` → `{threadId, resolution: {decision, resolvedAt}}`
- `thread.deleted` → `{threadId}`
- `draft.updated` / `draft.cleared` → `{scope, ...}` — ignore

## Tone for takes

- Be specific to the anchored content, not generic.
- Push back when you disagree; don't flatter.
- Cite the source doc when relevant ("line 24 says X, but...").
- Short is better than long — one or two focused paragraphs beats an essay.
- If you don't know, say so. Don't speculate.
