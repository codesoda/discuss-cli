@AGENTS.md

## Rust Patterns

- Shared fallible APIs should use `discuss::DiscussError` and `discuss::Result`, re-exported from `src/lib.rs`.
- Keep clap argument definitions in `src/cli.rs`; `src/main.rs` should stay thin and map app errors through `discuss::exit_code_for_error`.
- Config defaults and TOML parsing live in `src/config.rs`; use `Config::from_toml_str` when parsing file contents so errors preserve the config path and line/column location.
- Use `Config::resolve(ConfigOverrides)` for full layered config resolution. Internally, file/env layers are partial so omitted keys do not reset values from lower-priority layers.
- Tracing initialization lives in `src/logging.rs`; `run` resolves `Config` and calls `init_tracing`, which must write only to the rolling log file because stdout is reserved for JSON events.
- Markdown rendering lives in `src/render.rs` as pure `render(&str) -> String`; configure Comrak there, and keep the dependency on `default-features = false` unless a future story explicitly needs CLI/syntax-highlighting features.
- Bundled page-shell rendering lives in `src/template.rs`; call `render_page(rendered_markdown, initial_state_json)` after markdown rendering to preserve `discuss.html` while injecting `#doc-content` and seeding `window.__DISCUSS_INITIAL_STATE__`.
- Bundled browser assets live in `assets/` and are exposed through `src/assets.rs`; `render_page` inlines the Mermaid shim, while `assets::mermaid_js()` provides the minified asset for later static routes.
- Static browser asset routes live in `src/server.rs`; keep them exact-path, serve from `src/assets.rs`, and include `Cache-Control: public, max-age=86400`.
- State protocol types live in `src/state/types.rs`; keep serde field names camelCase, serialize `ThreadId` transparently as a string, and encode new-thread draft anchor ranges as `"start-end"` JSON object keys.
- Process-local review state lives in `src/state/store.rs`; use `State::new_shared()` for `Arc<RwLock<State>>`, mutate through typed accessors, and call `snapshot()` for the active browser/API state while soft-deleted threads stay preserved internally.
- Stdout JSON events live in `src/events.rs`; route all machine-readable stdout writes through `EventEmitter` and keep human-readable output on stderr or tracing.
- Browser SSE broadcasts live in `src/sse.rs`; use `EventBus` as an `Arc`-friendly Tokio broadcast wrapper alongside `SharedState`, and keep `BroadcastEvent` distinct from stdout `Event`.
- Axum server bootstrap lives in `src/server.rs`; use `AppState::for_process()` for production shared state and `serve(addr, app_state, shutdown)` for the 127.0.0.1-only graceful server wrapper.
- Attach the already-read markdown source to server state with `AppState::with_markdown_source(...)`; `GET /` renders that source and seeds the page from `State::snapshot()` on each request.
- Browser state API routes should serialize `State::snapshot()` directly so `GET /api/state` and initial page hydration share the same JSON shape.
- `discuss.html` hydrates state seed-first from `window.__DISCUSS_INITIAL_STATE__`, falling back to `GET /api/state`; keep `normalizeState` adapting server `StateSnapshot` fields into the legacy `userThreads`/`followups` renderer shape until the REST/SSE frontend migration is complete.
- Browser SSE streaming lives in `src/server.rs` at `GET /api/events`; subscribe to `AppState.bus`, emit `BroadcastEvent.kind` as the SSE event name with the JSON payload as `data`, and break the stream when `AppState::subscribe_shutdown()` fires.
- HTTP mutation handlers live in `src/server.rs`; on a successful state write they should publish a `BroadcastEvent` and emit the matching stdout `Event`, with tests injecting `EventEmitter::boxed(...)` through `AppState::new` to capture stdout.
- New-thread draft mutation routes use `/api/drafts/new-thread`; payloads include `scope: "newThread"` plus `anchorStart`/`anchorEnd`, whitespace-only POST delegates to clear, and idempotent clears still emit `draft.cleared`.
- Follow-up draft mutation routes use `/api/drafts/followup`; validate `threadId` against active `State::get_threads()` before upsert/clear, payloads include `scope: "followup"` plus `threadId`, and idempotent clears still emit `draft.cleared`.
- Axum dynamic routes in `src/server.rs` must use 0.8 `{id}` syntax (for example `/api/threads/{id}/replies`), not legacy `:id`.
- Child thread mutation handlers should validate against `State::get_threads()` before mutating; unknown or soft-deleted thread IDs return structured 404 JSON.
- Resolution mutation event payloads must include `threadId`; `thread.resolved` also nests the stored `resolution` object so clients can update state without rehydrating.
- Thread deletion is a soft delete for `kind = "user"` threads only; `kind = "prepopulated"` returns structured 403 code `prepopulated_thread`, and `thread.deleted` event payloads use `{ "threadId": ... }`.
- Root `install.sh` is currently a source-checkout installer only: it requires `Cargo.toml` next to the script, runs `RUSTFLAGS="-D warnings" cargo build --release`, installs to `~/.discuss/bin/discuss`, symlinks `~/.local/bin/discuss`, and verifies the linked binary with `--version`.
