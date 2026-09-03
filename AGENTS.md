Behavioral guidelines to reduce common LLM coding mistakes. Merge with project-specific instructions as needed.

**Tradeoff:** These guidelines bias toward caution over speed. For trivial tasks, use judgment.

## 1. Think Before Coding

**Don't assume. Don't hide confusion. Surface tradeoffs.**

Before implementing:

- State your assumptions explicitly. If uncertain, ask.
- If multiple interpretations exist, present them - don't pick silently.
- If a simpler approach exists, say so. Push back when warranted.
- If something is unclear, stop. Name what's confusing. Ask.

## 2. Simplicity First

**Minimum code that solves the problem. Nothing speculative.**

- No features beyond what was asked.
- No abstractions for single-use code.
- No "flexibility" or "configurability" that wasn't requested.
- No error handling for impossible scenarios.
- If you write 200 lines and it could be 50, rewrite it.

Ask yourself: "Would a senior engineer say this is overcomplicated?" If yes, simplify.

## 3. Surgical Changes

**Touch only what you must. Clean up only your own mess.**

When editing existing code:

- Don't "improve" adjacent code, comments, or formatting.
- Don't refactor things that aren't broken.
- Match existing style, even if you'd do it differently.
- If you notice unrelated dead code, mention it - don't delete it.

When your changes create orphans:

- Remove imports/variables/functions that YOUR changes made unused.
- Don't remove pre-existing dead code unless asked.

The test: Every changed line should trace directly to the user's request.

## 4. Goal-Driven Execution

**Define success criteria. Loop until verified.**

Transform tasks into verifiable goals:

- "Add validation" → "Write tests for invalid inputs, then make them pass"
- "Fix the bug" → "Write a test that reproduces it, then make it pass"
- "Refactor X" → "Ensure tests pass before and after"

For multi-step tasks, state a brief plan:

```
1. [Step] → verify: [check]
2. [Step] → verify: [check]
3. [Step] → verify: [check]
```

Strong success criteria let you loop independently. Weak criteria ("make it work") require constant clarification.

## Rust Patterns

- Shared fallible APIs should use `discuss::DiscussError` and `discuss::Result`, re-exported from `src/lib.rs`.
- Keep clap argument definitions in `src/cli.rs`; `src/main.rs` should stay thin and map app errors through `discuss::exit_code_for_error`.
- Config defaults and TOML parsing live in `src/config.rs`; use `Config::from_toml_str` when parsing file contents so errors preserve the config path and line/column location.
- Use `Config::resolve(ConfigOverrides)` for full layered config resolution. Internally, file/env layers are partial so omitted keys do not reset values from lower-priority layers.
- Runtime launch resolves `--port`/`--no-open`/`--history-dir`/`--no-save` through `ConfigOverrides`. Explicit nonzero CLI/env/TOML ports bind exactly and fail on collision; an unspecified port binds `127.0.0.1:0` directly for OS assignment. Never scan-then-bind. Use `bind_loopback_listeners` to acquire every required loopback listener before readiness, and `serve_listener` to serve an acquired API listener; partial acquisition failures must drop all listeners.
- Browser launch/status helpers live in `src/launch.rs`; write the single `review UI/API: http://127.0.0.1:<actual-port>` line to stderr after bind, call `open::that` with that exact URL only when `auto_open` remains true, and log browser-open failures with tracing warnings instead of failing the session.
- `discuss <file>` emits `session.started` from `src/lib.rs` inside the `serve_with_ready` readiness callback after all binds and before stderr announcement/browser open. Preserve existing payload fields and include `apiBaseUrl`, `endpoints` from `launch::session_endpoints`, and `agentInstructions`; omit optional `proxyUrl` unless a secondary proxy listener exists.
- Tracing initialization lives in `src/logging.rs`; `run` resolves `Config` and calls `init_tracing`, which must write only to the rolling log file because stdout is reserved for JSON events.
- Markdown rendering lives in `src/render.rs` as pure `render(&str) -> String`; configure Comrak there, and keep the dependency on `default-features = false` unless a future story explicitly needs CLI/syntax-highlighting features.
- Bundled page-shell rendering lives in `src/template.rs`; call `render_page(rendered_markdown, initial_state_json)` after markdown rendering to preserve `discuss.html` while injecting `#doc-content` and seeding `window.__DISCUSS_INITIAL_STATE__`.
- `src/template.rs` must target the real `#doc-content` section after `<body>` because the template's top instructional comment also mentions `<section id="doc-content">`.
- Bundled browser assets live in `assets/` and are exposed through `src/assets.rs`; `render_page` inlines the Mermaid shim, while `assets::mermaid_js()` provides the minified asset for later static routes. Demo-session fixtures are the one carve-out: they are session *inputs*, not browser assets, so they live in `assets/demo/` and are embedded by `src/server/demo.rs` (which also embeds `docs/demo.gif` directly, rather than duplicating a 3 MB blob — see decision D11 in `docs/plans/demo-mode.md`). Keep them out of `src/assets.rs`.
- Static browser asset routes live in `src/server.rs`; keep them exact-path, serve from `src/assets.rs`, and include `Cache-Control: public, max-age=86400`.
- Image files are read once as bytes during session loading, keep `File.content` empty, and are served only through `GET /api/files/{id}/raw` with the extension-derived image MIME type, an execution-blocking `Content-Security-Policy: sandbox`, and the standard one-day cache header. Rendered image URLs include a content-hash query so reused ports/file ids cannot show stale bytes across sessions. Unknown and non-image ids return structured 404 JSON.
- State protocol types live in `src/state/types.rs`; keep serde field names camelCase, serialize `ThreadId` transparently as a string, and encode new-thread draft anchor ranges as `"start-end"` JSON object keys.
- Image thread coordinates use `ImageAnchor { x_pct, y_pct }` basis points (`0..=10000`) serialized as `imageAnchor: {xPct, yPct}`. Require them exactly for image files, reject them for text/diff files, and assign a monotonic per-file pin number (including soft-deleted history) into both paragraph anchor fields while holding the state write lock.
- Process-local review state lives in `src/state/store.rs`; use `State::new_shared()` for `Arc<RwLock<State>>`, mutate through typed accessors, and call `snapshot()` for the active browser/API state while soft-deleted threads stay preserved internally.
- Transcript building lives in `src/transcript.rs`; use `build_transcript(&State)` for Done/history payloads so all threads, including soft-deleted ones, are emitted in document order.
- Stdout JSON events live in `src/events.rs`; route all machine-readable stdout writes through `EventEmitter` and keep human-readable output on stderr or tracing.
- Browser SSE broadcasts live in `src/sse.rs`; use `EventBus` as an `Arc`-friendly Tokio broadcast wrapper alongside `SharedState`, and keep `BroadcastEvent` distinct from stdout `Event`.
- Axum server bootstrap lives in `src/server.rs`; use `AppState::for_process()` for production shared state and `serve(addr, app_state, shutdown)` for the 127.0.0.1-only graceful server wrapper.
- Attach the already-read markdown source to server state with `AppState::with_markdown_source(...)`; `GET /` renders that source and seeds the page from `State::snapshot()` on each request. `POST /api/source` must reject image targets with `validation_error`; live image replacement is not supported.
- Browser state API routes should serialize `State::snapshot()` directly so `GET /api/state` and initial page hydration share the same JSON shape.
- `discuss.html` hydrates state seed-first from `window.__DISCUSS_INITIAL_STATE__`, falling back to `GET /api/state`; keep `normalizeState` adapting server `StateSnapshot` fields into the legacy `userThreads`/`followups` renderer shape until the REST/SSE frontend migration is complete.
- `discuss.html` may use `localStorage` for UI preferences only (theme, Cmd+Enter hint, file-sidebar collapse — gated by the allowlist in `src/template.rs::bundled_template_hydrates_state_from_seed_or_api`), never review state; review state flows through the server seed, REST mutation helpers, SSE updates, and the in-memory `currentState` mirror only, and `STORAGE_KEY = 'discuss-state'` must stay removed.
- `discuss.html` starts its `/api/events` `EventSource` only after hydration and initial render; incremental event handlers must be idempotent because the mutating tab receives its own SSE echo after optimistic UI updates.
- Server-backed thread mutations in `discuss.html` should use `apiJson` and `threadApiPath` with optimistic state snapshots and rollback on HTTP failure; do not reintroduce `saveState` for threads/replies/resolutions/deletes.
- Browser REST mutation failures in `discuss.html` should restore user input, rollback optimistic state, and call `showMutationError(...)` with a Retry closure instead of using `alert()` or dropping text.
- There is no v1 delete-reply endpoint, so the browser should not offer local-only follow-up deletion unless a matching REST API is added.
- Browser SSE streaming lives in `src/server.rs` at `GET /api/events`; subscribe to `AppState.bus`, emit `BroadcastEvent.kind` as the SSE event name with the JSON payload as `data`, and break the stream when `AppState::subscribe_shutdown()` fires.
- Browser heartbeat lives in `src/server.rs` at `POST /api/heartbeat` and updates `AppState::last_heartbeat_at()` silently; do not emit SSE broadcasts or stdout events for heartbeat pings.
- Idle detection is configured on `AppState` with `with_idle_timeout_secs(...)`; `0` disables the server-start idle task, and `prompt.suggest_done` is stdout-only rather than an SSE broadcast.
- `serve_with_ready` exits on either its external shutdown future or the internal `AppState` shutdown signal; routes hit after shutdown is signaled should return structured 503 when the listener has not already closed.
- Successful state-mutation handlers in `src/server.rs` must call the shared mutation timestamp updater after writing state so idle prompts are delayed by user/agent activity.
- HTTP mutation handlers live in `src/server.rs`; on a successful state write they should always publish the matching `BroadcastEvent`, while stdout `Event` emission is reserved for agent-relevant events such as session lifecycle and user-originated review changes.
- `POST /api/done` in `src/server.rs` builds transcripts with `build_transcript(&State)`, emits the `session.done` stdout event, then signals internal graceful shutdown; future Done/history work should preserve that ordering around any archive write.
- A rejected verdict on `/api/done` must be a total no-op — it must not build the transcript, emit `session.done`, write history, or signal internal shutdown; validation lives server-side in `validate_done_verdict` so scripted clients get the same policy as the browser modal.
- History archiving lives in `src/history.rs`; Done emits `session.done` first, then writes the transcript payload JSON to `<history_dir>/<sanitized-source-stem>/<ISO8601>.json` unless `AppState::with_no_save(true)` disables archives, and archive failures must warn to tracing/stderr without changing the `/api/done` response or shutdown.
- Browser Done completion uses `#finish-review`; without verdict options it calls `/api/done` with no body, while verdict-enabled sessions branch through the modal before `markReviewComplete()` closes SSE/heartbeat, disables controls, shows `#done-banner`, and suppresses further mutations/events.
- New-thread draft mutation routes use `/api/drafts/new-thread`; payloads include `scope: "newThread"` plus `anchorStart`/`anchorEnd`, whitespace-only POST delegates to clear, and idempotent clears still emit `draft.cleared` over SSE even though draft events do not go to stdout.
- Follow-up draft mutation routes use `/api/drafts/followup`; validate `threadId` against active `State::get_threads()` before upsert/clear, payloads include `scope: "followup"` plus `threadId`, and idempotent clears still emit `draft.cleared` over SSE even though draft events do not go to stdout.
- Browser draft writes in `discuss.html` should go through `setNewThreadDraft`/`setFollowupDraft`; those helpers optimistically update local state and queue REST writes per draft key so a later clear cannot be overwritten by an older in-flight save.
- Axum dynamic routes in `src/server.rs` must use 0.8 `{id}` syntax (for example `/api/threads/{id}/replies`), not legacy `:id`.
- Child thread mutation handlers should validate against `State::get_threads()` before mutating; unknown or soft-deleted thread IDs return structured 404 JSON.
- Resolution mutation event payloads must include `threadId`; `thread.resolved` also nests the stored `resolution` object so clients can update state without rehydrating.
- Thread deletion is a soft delete for `kind = "user"` and `kind = "agent"` threads; `kind = "prepopulated"` returns structured 403 code `prepopulated_thread`, and `thread.deleted` event payloads use `{ "threadId": ... }`.
- Root `install.sh` is dual-mode and POSIX `sh` compatible: source checkout mode requires `install.sh` plus `Cargo.toml` next to each other and builds with warnings denied, while curl/download mode resolves the latest GitHub Release asset for the detected target triple before installing to `~/.discuss/bin/discuss` and symlinking `~/.local/bin/discuss`.
- Update logic lives in `src/update.rs`; installs stay explicit-only (the browser badge only checks), fail fast on non-TTY installs unless `-y/--yes` is passed, resolve the latest tag via the GitHub `/releases/latest` redirect, verify `checksums-sha256.txt` against the downloaded tarball, and only then call `self_replace::self_replace`.
- GitHub Actions CI lives in `.github/workflows/ci.yml`; keep workflow-level `RUSTFLAGS="-D warnings"` and `CARGO_INCREMENTAL="0"`, and preserve the Rust step order `fmt -> clippy --all-targets -> build --all-targets -> test`.
- GitHub Actions release packaging lives in `.github/workflows/release.yml`; publish `discuss-<tag>-<target>.tar.gz` plus `checksums-sha256.txt`, and extract release notes by matching either `## [vX.Y.Z]` or `## [X.Y.Z]` changelog headings so Keep a Changelog sections stay compatible with `v` tags.
- `CHANGELOG.md` follows Keep a Changelog headings; note that the repo's `awk '/^## \\[Unreleased\\]/,/^## \\[/' CHANGELOG.md` smoke check only proves the `## [Unreleased]` heading exists, because the end pattern matches that same line.
- Browser code-fence syntax highlighting is wired through Prism loaded from unpkg in `src/template.rs`; bump the single `PRISM_VERSION` constant to refresh and let the autoloader resolve grammars on demand. When generating markdown to review, tag every fence with a language — see `skills/discuss/SKILL.md` for the curated list and the link to Prism's full set. Diff colors must remain defined locally for both themes, including the prefix-based fallback used when Prism's diff grammar is unavailable.
- Multi-file sidebar paths are grouped by `buildFileTree`/`appendFileTree` into independently collapsible folders. Keep full paths in titles/accessible names, keep active-file ancestors open, aggregate open-thread badges into folders, and preserve the global collapsed icon rail as a flat all-files view. PR diff files expose server-backed viewed state through `/api/pr/files/{id}/viewed`; retain `{fileId, viewedAt, headSha}` in `PrReviewState`, use a non-approval eye treatment in the sidebar, and advance only to the next unviewed diff file.
- Private-first PR review contracts live in `src/pr/`, with browser/agent lifecycle handlers in `src/server/pr.rs`. `discuss pr` accepts only a full public-GitHub PR URL; Discuss never fetches GitHub or receives its token. The authenticated agent imports one aggregate diff through the bearer-protected endpoint, and only an explicit OK may emit `pr.publish.requested`. Include defaults stay false, destinations are server-derived/immutable, ambiguous targets remain unpublished, and a successful agent callback emits `session.done` before archive/shutdown.
- HTML prototype review uses `FileKind::Html`, serves the document at `GET /files/{id}` with CSP-meta removal plus `<base>`/inspector injection, and serves relative assets only after canonical containment checks under `/files/{id}/assets/{*path}`. Keep the static-prototype iframe sandbox at exactly `allow-scripts allow-same-origin` unless the security contract changes.
- HTML thread anchors use optional `Thread.element_anchor` with zero numeric anchors. Require `elementAnchor` only for HTML files, cap `outerHtml` server-side, include it in stdout/transcripts, and report load-time detachment through SSE-only `POST /api/anchors/resolve` rather than `POST /api/source`.
- Live website review treats a sole HTTP/S argument as a fixed-upstream session. Bind API and proxy loopback listeners together before readiness; no-port sessions use two OS-assigned listeners, while explicit `--port N` binds the proxy exactly at `N + 1` and fails if either address collides. Report `mode: "live"`, `upstreamUrl`, and `proxyUrl`, but keep all Discuss mutations on `payload.endpoints`. Proxy HTML only: strip frame blockers/CSP meta, inject the early service-worker guard and cross-origin inspector, cap buffering, and keep non-HTML bytes streaming unchanged. Live anchors require route context, and detachment updates are route-scoped. Keep the live sandbox separate from the static contract and do not grant top navigation.
- The header version badge is fed by `GET /api/version` in `src/server/pages.rs`, which wraps the never-failing `update::version_status()` in `spawn_blocking` and caches only successful release lookups for the process lifetime; keep the endpoint browser-facing (not in `session_endpoints`) and keep the frontend copy-only — it must not trigger a server-side self-update.
