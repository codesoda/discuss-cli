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
- State protocol types live in `src/state/types.rs`; keep serde field names camelCase, serialize `ThreadId` transparently as a string, and encode new-thread draft anchor ranges as `"start-end"` JSON object keys.
- Process-local review state lives in `src/state/store.rs`; use `State::new_shared()` for `Arc<RwLock<State>>`, mutate through typed accessors, and call `snapshot()` for the active browser/API state while soft-deleted threads stay preserved internally.
- Stdout JSON events live in `src/events.rs`; route all machine-readable stdout writes through `EventEmitter` and keep human-readable output on stderr or tracing.
- Browser SSE broadcasts live in `src/sse.rs`; use `EventBus` as an `Arc`-friendly Tokio broadcast wrapper alongside `SharedState`, and keep `BroadcastEvent` distinct from stdout `Event`.
- Axum server bootstrap lives in `src/server.rs`; use `AppState::for_process()` for production shared state and `serve(addr, app_state, shutdown)` for the 127.0.0.1-only graceful server wrapper.
