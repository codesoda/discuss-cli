@AGENTS.md

## Rust Patterns

- Shared fallible APIs should use `discuss::DiscussError` and `discuss::Result`, re-exported from `src/lib.rs`.
- Keep clap argument definitions in `src/cli.rs`; `src/main.rs` should stay thin and map app errors through `discuss::exit_code_for_error`.
- Config defaults and TOML parsing live in `src/config.rs`; use `Config::from_toml_str` when parsing file contents so errors preserve the config path and line/column location.
- Use `Config::resolve(ConfigOverrides)` for full layered config resolution. Internally, file/env layers are partial so omitted keys do not reset values from lower-priority layers.
- Tracing initialization lives in `src/logging.rs`; `run` resolves `Config` and calls `init_tracing`, which must write only to the rolling log file because stdout is reserved for JSON events.
- Markdown rendering lives in `src/render.rs` as pure `render(&str) -> String`; configure Comrak there, and keep the dependency on `default-features = false` unless a future story explicitly needs CLI/syntax-highlighting features.
