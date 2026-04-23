# PRD: discuss-cli v1

## 1. Introduction / Overview

`discuss` is a Rust CLI that turns any markdown file into a live, bidirectional review session between a human (in a browser) and the AI agent that launched it (over stdout/HTTP).

Today, when an AI coding agent wants a structured human review of a plan or document, the agent has to copy the existing `discuss.html` template, embed the document content as HTML, write the file somewhere, and ask the user to open it. The user comments in the browser, clicks "Copy All," and pastes the export back into chat. There is no live channel: the agent can't react to a comment until the user finishes the entire review and pastes.

`discuss <file.md>` collapses that workflow into one command. The CLI:

1. Renders the markdown through the existing two-pane template
2. Serves it on a local HTTP server
3. Opens the user's browser
4. Streams every user action (comment, draft, reply, resolve) as JSON lines on stdout, where the launching agent reads them
5. Accepts model "takes" via HTTP, pushes them to the browser via SSE, so the user sees the agent's reply in place
6. On "Done," dumps the consolidated transcript to stdout, archives a JSON copy to `~/.discuss/history/`, and exits cleanly

The result: a single command in, a live bidirectional review, a clean structured dump out — no copy/paste step, no per-review HTML file generation.

## 2. Goals

- **Single-command launch.** `discuss path/to/file.md` is the only invocation needed.
- **Live bidirectional review.** Agent and user see each other's input within seconds, not at the end.
- **Zero data loss in-session.** Drafts, comments, replies, takes, resolutions all persist server-side and survive browser reloads.
- **Clean machine-parseable transcript.** Done emits a structured JSON document the agent consumes directly — no HTML scraping or copy/paste.
- **Self-contained binary.** No internet required after install. HTML template, mermaid.js, and all assets bundled at compile time.
- **Bugatti-mold shipping.** install.sh dual-mode (clone-build + curl|sh), GitHub Actions CI + Releases workflow, `discuss update` subcommand, structured logging — production-ready from v1.
- **Agent-friendly defaults.** stdout reserved for machine-readable events; stderr for human status; help text links to docs and `llms.txt`; cascading config so agents and users can both customize.

## 3. Definition of Done

**Definition of Done (applies to all stories):**
- All acceptance criteria met
- `cargo fmt --check` passes
- `RUSTFLAGS="-D warnings" cargo check` passes
- `cargo clippy --all-targets -- -D warnings` passes
- `RUSTFLAGS="-D warnings" cargo test` passes
- Tests written for any new pure logic (rendering, state mutation, transcript building)

## 4. User Stories

### US-001: CLI scaffolding

**Description:** As a developer, I want a Cargo project with a clean clap-based CLI shape so that argument parsing, help text, and exit codes are consistent from day one.

**Acceptance Criteria:**
- [ ] `cargo new discuss` initialized with appropriate `Cargo.toml` (edition 2021, license, repository, description)
- [ ] `clap` (with `derive` feature) wires up `discuss <file>` plus subcommand `discuss update`
- [ ] `arg_required_else_help = true` so a bare `discuss` shows help instead of erroring
- [ ] `--help` includes the documented exit codes (FR-23) and a `Docs:` / `LLM ref:` block (per bugatti pattern)
- [ ] `--version` works and reports the crate version
- [ ] Top-level error type via `thiserror` (`DiscussError` enum) covering: file not found, file not readable, port in use, config parse error, render error, server bind error
- [ ] Every error message names what went wrong + (where applicable) the path it checked + a suggested next step
- [ ] `main()` maps every error to the documented exit code and writes a human-readable line to stderr

### US-002: Layered config + tracing logging

**Description:** As a user (or agent), I want config to cascade from sensible defaults through user/project files to CLI flags so I can pin behavior at whichever layer makes sense.

**Acceptance Criteria:**
- [ ] Config struct (`serde` + `deny_unknown_fields`) with at minimum: `port` (Option<u16>), `auto_open` (bool, default true), `idle_timeout_secs` (u64, default 600), `history_dir` (Option<PathBuf>), `no_save` (bool, default false)
- [ ] Resolution order (lowest → highest priority): defaults → `~/.discuss/discuss.config.toml` → `./discuss.config.toml` → `DISCUSS_*` env vars → CLI flags
- [ ] Missing config files are silently ignored (defaults apply); malformed config files produce a `ConfigError::ParseError` with path + line/col
- [ ] `tracing` initialized to write to `~/.discuss/logs/discuss-YYYY-MM-DD.log` with daily rotation, no ANSI codes in file output
- [ ] `DISCUSS_LOG` env var overrides log filter (e.g. `DISCUSS_LOG=debug`); falls back to config `log_level`, then `info`
- [ ] Logging never writes to stdout (which is reserved for the JSON event stream)

### US-003: Markdown rendering pipeline

**Description:** As a user, I want my markdown — including GFM tables, task lists, fenced code blocks, and mermaid diagrams — rendered into the commentable two-pane UI exactly as the template expects.

**Acceptance Criteria:**
- [ ] `comrak` renders the source markdown to HTML with GFM extensions enabled (tables, strikethrough, autolink, task lists, footnotes)
- [ ] Output uses native semantic elements (`h1`–`h5`, `p`, `ul/ol/li`, `blockquote`, `pre`) so the template's `assignAnchorIndices()` finds them via the existing `COMMENTABLE_SELECTOR` without modification
- [ ] The current `discuss.html` template is bundled into the binary via `include_str!` and served as the page shell, with rendered markdown injected into `#doc-content` server-side before the page is sent
- [ ] A small JS shim (~30 lines, bundled or appended at render time) finds `<pre><code class="language-mermaid">...</code></pre>` blocks and replaces their contents with a mermaid SVG render
- [ ] `mermaid.js` is bundled into the binary via `include_str!` (~600KB binary growth accepted) and only loaded by the page if at least one mermaid block is present in the rendered HTML
- [ ] Mermaid blocks remain a single commentable `<pre>` anchor (so a user can comment on the diagram as a unit) — anchor indices are stable regardless of mermaid's SVG insertion
- [ ] Render is a pure function: `(markdown: &str) -> String` — unit tests cover headings, lists, blockquotes, code blocks, tables, mermaid passthrough

### US-004: HTTP server bootstrap + in-memory state model

**Description:** As a developer, I want a minimal HTTP server hosting the rendered template plus a typed in-memory state store, so subsequent stories can wire endpoints into a single source of truth.

**Acceptance Criteria:**
- [ ] HTTP server (`axum` recommended) binds to `127.0.0.1` on the configured port (default 7777, fallback if taken — see US-011)
- [ ] `GET /` returns the bundled template with the rendered markdown injected into `#doc-content`, the current state seeded into the page (so reloads pick up the latest threads/drafts)
- [ ] Static asset routes serve any bundled JS/CSS (mermaid shim, mermaid.js) under stable paths
- [ ] In-memory state struct (typed, `Arc<RwLock<...>>` or actor): `threads: Vec<Thread>`, `replies: HashMap<ThreadId, Vec<Reply>>`, `takes: HashMap<ThreadId, Vec<Take>>`, `resolutions: HashMap<ThreadId, Resolution>`, `drafts: { new_thread: HashMap<(usize, usize), Draft>, followup: HashMap<ThreadId, Draft> }`
- [ ] Thread/reply/take/resolution structs serialize to JSON shapes that match what the page already expects (compatible with the current template's render functions)
- [ ] State is process-local only — no sidecar, no disk persistence during the session
- [ ] Server graceful shutdown: a shutdown signal closes all SSE streams, then exits the bind

### US-005: Thread mutation REST API

**Description:** As the browser (or agent), I want REST endpoints for every thread state change so all mutations flow through the server.

**Acceptance Criteria:**
- [ ] `POST /api/threads` — create user thread; body `{ anchorStart, anchorEnd, snippet, text }`; returns `{ id, createdAt }`
- [ ] `POST /api/threads/:id/replies` — append a user reply; body `{ text }`; returns the new reply
- [ ] `POST /api/threads/:id/takes` — append a model take (used by the agent); body `{ text }`; returns the new take
- [ ] `POST /api/threads/:id/resolve` — body `{ decision: Option<String> }`; sets resolution
- [ ] `POST /api/threads/:id/unresolve` — clears resolution
- [ ] `DELETE /api/threads/:id` — soft-delete a user thread (prepopulated/seeded threads cannot be deleted, return 403)
- [ ] All mutation endpoints update in-memory state under a write lock, then trigger one stdout JSON event (US-008) and one SSE broadcast (US-007)
- [ ] All endpoints validate input; malformed JSON returns 400 with a structured error body
- [ ] Endpoints are idempotent where it matters (resolve on already-resolved is a no-op success)

### US-006: Draft mutation REST API

**Description:** As the browser, I want server-side draft persistence so a click-off or reload doesn't lose in-progress text.

**Acceptance Criteria:**
- [ ] `POST /api/drafts/new-thread` — body `{ anchorStart, anchorEnd, text }`; upserts the draft for that anchor range
- [ ] `DELETE /api/drafts/new-thread` — body `{ anchorStart, anchorEnd }`; clears the draft
- [ ] `POST /api/drafts/followup` — body `{ threadId, text }`; upserts the draft on that thread
- [ ] `DELETE /api/drafts/followup` — body `{ threadId }`; clears the draft
- [ ] Empty/whitespace-only drafts are treated as "clear" rather than "save"
- [ ] Draft mutations emit stdout events (`draft.updated`, `draft.cleared`) and SSE broadcasts so other tabs (or a reload) see the same state

### US-007: SSE event stream

**Description:** As the browser, I want a server-sent-events stream so the page reflects changes the agent makes (and reflects state across reloads/tabs) without polling.

**Acceptance Criteria:**
- [ ] `GET /api/events` returns an SSE stream
- [ ] Every state mutation (thread create/delete/resolve/unresolve, reply, take, draft change) broadcasts one event with `kind` + payload
- [ ] Browser code in the bundled template subscribes on load, applies incremental updates: add new threads, append replies/takes to existing thread cards, update resolution banners, update draft markers
- [ ] SSE handler tolerates client disconnects without leaking memory; reconnects pick up the current state via the initial page render (no event replay needed for v1)
- [ ] Heartbeat comment line every 15s keeps proxies/connections alive

### US-008: stdout JSON event stream

**Description:** As the launching agent, I want every mutation to arrive as one JSON line on the CLI's stdout so I can react to user input in real time.

**Acceptance Criteria:**
- [ ] One newline-delimited JSON object per event, written to stdout immediately on commit (no buffering beyond a line flush)
- [ ] Event kinds at minimum: `session.started`, `thread.created`, `thread.deleted`, `thread.resolved`, `thread.unresolved`, `reply.added`, `take.added`, `draft.updated`, `draft.cleared`, `prompt.suggest_done`, `session.done`
- [ ] Every event includes: `kind`, `at` (ISO timestamp), and a `payload` with the relevant ids/text/anchor info
- [ ] stdout is reserved exclusively for these events — all human-readable status (server URL on launch, "browser opened," shutdown notice) goes to stderr
- [ ] When stdout is not a TTY, formatting is unchanged (no color, no decoration)

### US-009: Migrate browser from localStorage to API

**Description:** As the page, I want all reads/writes to flow through the server instead of localStorage so state is shared across reloads, tabs, and the agent.

**Acceptance Criteria:**
- [ ] `loadState()` is replaced with an initial GET that hydrates threads/replies/takes/resolutions/drafts from the server
- [ ] Every existing `saveState()` call site is replaced with the corresponding REST API call (US-005, US-006)
- [ ] `localStorage` is no longer touched by the page (removing the `STORAGE_KEY` constant)
- [ ] On page load, all existing UI behaviors (thread markers, draft markers, resolution banner, "show all" toggle, copy-all-replaced-by-Done — see US-013) work end-to-end against the server
- [ ] Network failures on a mutation surface a non-blocking inline error (e.g. "couldn't save — retry?") without losing the user's input

### US-010: Visual distinction for model takes vs user replies

**Description:** As the user, I want to tell at a glance whether a comment in a thread came from me or from the agent so the conversation reads naturally.

**Acceptance Criteria:**
- [ ] Model takes render in the existing prepopulated/blue style (reusing the template's blue accent treatment)
- [ ] User replies render in the existing user/pink style
- [ ] When a thread has both, takes and replies render in chronological order (timestamp-sorted) so the conversation is readable top-to-bottom
- [ ] Thread marker color reflects whether a thread is purely user, purely model-pending, or mixed (define the precedence in the spec — recommend: resolved > mixed-with-take > user > pending)
- [ ] Tooltip on a thread marker indicates the latest contributor ("you" vs "agent") in addition to the existing snippet preview

### US-011: Single-instance lock + auto-open browser

**Description:** As a user, I want one `discuss <file>` invocation to launch the browser automatically and prevent accidentally running two sessions on the same file.

**Acceptance Criteria:**
- [ ] On startup, attempt to bind the configured port (default 7777). If the port is in use, exit with `DiscussError::PortInUse` and a message naming the port + suggesting `--port` or stopping the other instance (exit code 3)
- [ ] After a successful bind, log the listening URL to stderr ("listening on http://127.0.0.1:7777")
- [ ] Auto-open the user's default browser at `http://127.0.0.1:<port>/` using `open` (or equivalent crate) — macOS `open`, Linux `xdg-open`
- [ ] `--no-open` flag suppresses the auto-open (for headless / CI / agent contexts)
- [ ] `--port <N>` overrides the configured/default port for this invocation
- [ ] No internal "find a free port" fallback in v1 — predictable port behavior matters for the agent's curl URLs

### US-012: Heartbeat + idle prompt.suggest_done event

**Description:** As the agent, I want a stdout signal when the browser has gone quiet for a while so I can prompt the user to wrap up rather than hanging forever.

**Acceptance Criteria:**
- [ ] Browser sends a `POST /api/heartbeat` every 30s while the page is open (silent, no event broadcast)
- [ ] Server tracks the last-heartbeat timestamp; if no heartbeat AND no mutation for `idle_timeout_secs` (config-driven, default 600 = 10 min), emit a single `prompt.suggest_done` stdout event with `{ idle_for_secs }`
- [ ] After emission, the timer resets — the next idle window is another 10 min
- [ ] If the page is closed entirely (heartbeat stops cold), the same idle event fires after the timeout — the agent decides what to do (e.g. ask the user via chat, then call Done themselves via `POST /api/done`)
- [ ] Idle detection can be disabled by setting `idle_timeout_secs = 0` in config

### US-013: Done button + transcript + clean exit

**Description:** As the user (or agent), I want one click to seal the review, hand the transcript back to the agent, and shut everything down cleanly.

**Acceptance Criteria:**
- [ ] The header's "Copy All" button is renamed to "Done — send to chat" and replaces the clipboard-copy behavior
- [ ] Clicking Done issues `POST /api/done` (the agent can also call this directly)
- [ ] Server builds the consolidated transcript: every thread in document order, with `id`, `anchorStart`, `anchorEnd`, `snippet`, `breadcrumb`, `replies[]`, `takes[]`, `resolution` (or null), all timestamps preserved
- [ ] Transcript is written as a single `session.done` JSON event to stdout (the same shape used by US-014's history archive)
- [ ] HTTP response to `POST /api/done` returns 200 with a small confirmation payload; the page shows a "You can close this tab" banner
- [ ] Server initiates graceful shutdown after the response is fully written; process exits with code 0
- [ ] Subsequent API calls during shutdown return 503 cleanly rather than panicking

### US-014: History auto-save + --no-save flag

**Description:** As a user, I want every completed review automatically archived to disk so I can revisit it later, with an opt-out for ephemeral sessions.

**Acceptance Criteria:**
- [ ] On Done, the same JSON transcript emitted to stdout is written to `~/.discuss/history/<source-name>/<ISO8601-timestamp>.json`
- [ ] `<source-name>` derived from the input file's basename without extension; non-filename-safe characters (slashes, colons, etc.) are replaced with `_`; falls back to `unnamed` if the input was stdin or unresolvable
- [ ] Parent directory is created on demand (`fs::create_dir_all`); write failures log a warning to stderr but do **not** fail the session (the agent already has the transcript on stdout)
- [ ] `--no-save` CLI flag and `no_save = true` config option both suppress the archive write
- [ ] `--history-dir <path>` CLI flag overrides the default `~/.discuss/history/` location for this invocation

### US-015: install.sh dual-mode

**Description:** As a user, I want to install discuss either by curling a script or by cloning and running `./install.sh` so that "how do I install this?" has a one-line answer either way.

**Acceptance Criteria:**
- [ ] `install.sh` detects whether it's running from a cloned repo (presence of `Cargo.toml` next to it) or via `curl | sh` (no Cargo.toml present)
- [ ] Clone-build path: `cargo build --release`, copy `target/release/discuss` to `$INSTALL_DIR` (default `/usr/local/bin`, override via `DISCUSS_INSTALL_DIR`)
- [ ] curl-download path: detect platform (`uname -s` / `uname -m` → target triple), fetch `discuss-vX.Y.Z-<target>.tar.gz` from GitHub Releases, extract, install
- [ ] macOS codesign / quarantine flag handled (or documented if requiring user action)
- [ ] PATH check at the end — if `$INSTALL_DIR` not on PATH, print a clear instruction
- [ ] Script respects `NO_COLOR` and detects whether stdout is a TTY (per CLI talk patterns)
- [ ] Script exits non-zero on failure with a clear error

### US-016: GitHub Actions CI

**Description:** As a maintainer, I want every push and PR to run formatting, lint, build, and test gates with warnings treated as errors so regressions can't land.

**Acceptance Criteria:**
- [ ] `.github/workflows/ci.yml` runs on `push` to `main` and on `pull_request`
- [ ] Concurrency group cancels stale runs on the same ref
- [ ] Steps (in order): checkout, install stable rust toolchain (rustfmt + clippy), `Swatinem/rust-cache@v2`, `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo build --all-targets`, `cargo test`
- [ ] `RUSTFLAGS: "-D warnings"` and `CARGO_INCREMENTAL: "0"` set at the workflow level
- [ ] CI passes on the initial commit that introduces the workflow

### US-017: GitHub Actions release workflow

**Description:** As a maintainer, I want pushing a `vX.Y.Z` tag to build platform binaries, generate checksums, extract release notes, and publish a GitHub Release automatically.

**Acceptance Criteria:**
- [ ] `.github/workflows/release.yml` triggers on tags matching `v*`
- [ ] Build matrix covers at minimum `aarch64-apple-darwin` (macOS Apple Silicon); commented placeholders for `x86_64-apple-darwin` and `x86_64-unknown-linux-gnu` to add as needed
- [ ] Each matrix job builds with `--release --target <triple>` and packages as `discuss-<tag>-<target>.tar.gz`
- [ ] After all builds succeed, a publish job: collects artifacts, generates `checksums-sha256.txt`, extracts the section for this version from `CHANGELOG.md`, creates a GitHub Release with notes + assets
- [ ] `CHANGELOG.md` exists in the repo with at least an `## [Unreleased]` section so the workflow has something to extract on first release
- [ ] Workflow does not require any secrets beyond `GITHUB_TOKEN`

### US-018: discuss update subcommand

**Description:** As a user, I want a one-shot way to check for and install a newer version of discuss without leaving the terminal.

**Acceptance Criteria:**
- [ ] `discuss update --check` queries the GitHub `/releases/latest` endpoint via the redirect-trick (no API token, reads `Location` header without following), compares against the running version using `semver`, prints status to stderr (current vs latest)
- [ ] `discuss update -y` (alias `--yes`) downloads the platform-appropriate tarball from the latest release, verifies its sha256 against the published checksums file, and atomically swaps the running binary using `self-replace`
- [ ] Without `-y`, the user is prompted before download (skip if non-TTY → require explicit `-y`)
- [ ] All update operations have a 3s connect timeout; failures print actionable errors and exit non-zero
- [ ] No passive / automatic update check anywhere in the codebase — the user must run `discuss update` explicitly
- [ ] `BUGATTI`-style env opt-out is **not** needed for v1 since there's no passive check; document this in the subcommand's `--help`

## 5. Functional Requirements

### Rendering & content

- **FR-1:** Render any valid markdown (CommonMark + GFM tables, task lists, strikethrough, autolinks, footnotes) to HTML server-side via comrak.
- **FR-2:** Output HTML must use semantic elements (`h1`–`h5`, `p`, `ul/ol/li`, `blockquote`, `pre`) so the existing template's `COMMENTABLE_SELECTOR` matches without modification.
- **FR-3:** Mermaid code blocks (` ```mermaid `) are rendered client-side via a bundled mermaid.js, hydrated by a small JS shim that runs after page load.
- **FR-4:** mermaid.js is bundled into the binary at compile time; no network access required to render diagrams.
- **FR-5:** The bundled `discuss.html` template is the single page shell — no per-invocation template generation.

### HTTP server

- **FR-6:** Bind `127.0.0.1` only (no external interface in v1); default port 7777, override via config or `--port`.
- **FR-7:** Single instance per port — second `discuss` invocation on the same port exits with a clear error (exit code 3).
- **FR-8:** Endpoints: `GET /`, `GET /api/state` (initial hydration), `POST /api/threads`, `POST /api/threads/:id/replies`, `POST /api/threads/:id/takes`, `POST /api/threads/:id/resolve`, `POST /api/threads/:id/unresolve`, `DELETE /api/threads/:id`, `POST /api/drafts/new-thread`, `DELETE /api/drafts/new-thread`, `POST /api/drafts/followup`, `DELETE /api/drafts/followup`, `GET /api/events`, `POST /api/heartbeat`, `POST /api/done`.
- **FR-9:** Every state-mutating endpoint, on success, both broadcasts an SSE event and writes one stdout JSON line.
- **FR-10:** No authentication on local API in v1 (rely on `127.0.0.1` binding).

### State

- **FR-11:** All session state lives in process memory; no sidecar file is read or written during the session.
- **FR-12:** Drafts (new-thread + followup) persist server-side and survive page reloads within a session.
- **FR-13:** State is shared across browser tabs / reloads via the SSE stream + initial GET.

### stdout event protocol

- **FR-14:** stdout emits newline-delimited JSON only — no decoration, color, or human-readable text.
- **FR-15:** Event types: `session.started`, `thread.created`, `thread.deleted`, `thread.resolved`, `thread.unresolved`, `reply.added`, `take.added`, `draft.updated`, `draft.cleared`, `prompt.suggest_done`, `session.done`.
- **FR-16:** Every event has `kind`, `at` (ISO 8601 UTC), and a `payload` object specific to the event type.
- **FR-17:** stderr carries human-readable status (URL, browser open notice, shutdown, errors).

### Lifecycle

- **FR-18:** On startup: bind port → render markdown → seed state → start server → emit `session.started` → auto-open browser (unless `--no-open`).
- **FR-19:** On Done: build transcript → emit `session.done` to stdout → write history archive (unless `--no-save`) → respond 200 → graceful shutdown → exit 0.
- **FR-20:** On idle (no heartbeat AND no mutation for `idle_timeout_secs`): emit `prompt.suggest_done` to stdout, reset the idle timer.

### Configuration

- **FR-21:** Config resolution order, lowest → highest: defaults → `~/.discuss/discuss.config.toml` → `./discuss.config.toml` → `DISCUSS_*` env vars → CLI flags.
- **FR-22:** Strict parsing (`deny_unknown_fields`); typos surface as parse errors with file path.

### Exit codes

- **FR-23:** Exit codes:
  - `0` Clean exit (Done, or update completed)
  - `1` Generic failure (file not found, render error, etc.)
  - `2` Configuration / parse error
  - `3` Port already in use (or other server bind failure)
  - `5` Interrupted (Ctrl+C)

### History archive

- **FR-24:** Archive path: `~/.discuss/history/<source-basename>/<ISO8601-timestamp>.json`; override base via `--history-dir` or config; opt out entirely via `--no-save` or `no_save = true`.
- **FR-25:** Archive write failure is logged but never fails the session.

## 6. Non-Goals (Out of Scope for v1)

- **No sidecar / on-disk session state.** State is purely in-memory; crash mid-session loses unflushed work. Done writes to history only.
- **No multiple concurrent sessions.** Single-port lock means one `discuss` per port. Power users can `--port` for parallel sessions but the tool doesn't manage them.
- **No multi-template system.** The bundled `discuss.html` is the only template. `--template named_template` and `~/.discuss/templates/` are post-v1.
- **No streaming markdown rendering.** Markdown is rendered once at startup; later edits to the source file are not reflected. Streamdown / live re-render is post-v1.
- **No syntax highlighting in code blocks.** Plain `<pre><code>` only; syntect integration is post-v1.
- **No Windows support in v1.** macOS + Linux only (matches bugatti's v1 scope).
- **No passive / automatic update checks.** Updates only via explicit `discuss update`.
- **No authentication on the local API.** Relies on 127.0.0.1 binding; token / random-path schemes are post-v1 if cross-tool isolation becomes a concern.
- **No edit/delete on prepopulated threads.** v1 source has no seeded threads (the `--threads prepopulated.json` flag from notes.md is deferred); related delete-prevention logic ships, but the seed flag does not.

## 7. Design Considerations

- The existing `discuss.html` template is the design canon. v1 changes to the template are limited to:
  - Removing `localStorage` reads/writes (US-009)
  - Renaming "Copy All" → "Done — send to chat" and changing its handler (US-013)
  - Adding the SSE consumer (US-007)
  - Adding the heartbeat ping (US-012)
  - Adjusting thread-marker rendering to reflect mixed take/reply threads (US-010)
- No new visual primitives are introduced. Model takes reuse the existing prepopulated/blue style; user replies reuse the user/pink style.
- The mermaid hydration shim is deliberately tiny (~30 lines) so the template stays single-file and inspectable.

## 8. Technical Considerations

- **Crates** (proposed; agent may substitute equivalents): `clap` (derive), `serde` + `serde_json` + `toml`, `thiserror`, `tracing` + `tracing-appender`, `tracing-subscriber` (with `env-filter`), `axum` + `tokio`, `tower-http` (for static files / cors if needed), `comrak`, `reqwest` (blocking, for `update`), `semver`, `self-replace`, `open` (for browser launch), `chrono` (or `time`), `directories` (for `~/.discuss/`).
- **Bundling:** `include_str!` for `discuss.html`, the mermaid hydration shim, and `mermaid.min.js`. Binary size budget: <10MB after `strip = true`, `lto = true` in `[profile.release]`.
- **Concurrency:** `Arc<RwLock<State>>` is fine for v1 — single-instance, low write rate. Move to actor model if that proves contentious.
- **SSE broadcast:** `tokio::sync::broadcast` channel held by the server; each `GET /api/events` handler subscribes to it.
- **Browser launch:** `open` crate handles platform differences. Failures are logged but do not exit non-zero (the user can navigate manually using the URL on stderr).
- **Async vs sync:** server is async (axum/tokio). The `update` subcommand uses blocking `reqwest` — it's a one-shot operation that doesn't share a runtime.
- **Source projects to mirror for plumbing:** `bugatti-cli` (CI, release workflow, update subcommand, install.sh), `agentmark` (logging, layered config, error types).

## 9. Success Metrics

- `discuss <file.md>` → browser-open in under 1 second on a typical laptop, for a markdown file under 100KB.
- An agent can drive a full review loop (read user comment → post take → see user reply → see resolution) using only the documented HTTP endpoints + stdout events, with no other channels needed.
- Zero data loss during a normal session: all drafts, comments, replies, takes, and resolutions present at Done are reflected in the stdout transcript and the history archive.
- Binary size under 10MB after release-profile build with mermaid.js bundled.
- Tool works fully offline once installed (no runtime CDN fetches, no required network calls during a session).
- `cargo install` from source AND `curl -sSL <release-script> | sh` both succeed on macOS Apple Silicon and Linux x86_64.

## 10. Open Questions / Future Work

These are deliberately out of v1 scope. File each as a GitHub issue when v1 ships:

- **Multi-template support.** `~/.discuss/templates/<name>.html` + `--template named_template` flag, allowing different templates for different file types (e.g. a code-review template for `.rs` files, a default for `.md`).
- **Streaming markdown rendering.** Integrate Streamdown (or equivalent) for live LLM-generated content where the markdown grows over time without losing comment anchors. Likely requires a stable-anchor strategy that survives content insertion.
- **Syntax highlighting in code blocks.** Either server-side via `syntect` or client-side via `prism` / `highlight.js`. Decide based on binary-size budget at the time.
- **Windows support.** Cross-compile target, `install.ps1`, `cfg(target_os)` shims for browser launch and paths.
- **Passive update check.** Add the bugatti-style background check after a successful Done, with `DISCUSS_NO_UPDATE_CHECK=1` opt-out and 24h throttle.
- **Cross-session local-API auth.** Random token in the launch URL, required on every API call — only worth doing if cross-tab / cross-process exposure on localhost becomes a real concern.
- **Sidecar persistence.** Reconsider if real-world crash-mid-session loss turns out to hurt; would re-introduce the `<file>.discuss.json` design from notes.md.
- **`--threads prepopulated.json` seed flag.** Allow agents to pre-seed threads with their initial takes before the user opens the page.
