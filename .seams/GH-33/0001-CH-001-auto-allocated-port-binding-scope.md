---
artifact: seam_scope
issue: GH-33
seam_id: "0001"
checklist_ids: [CH-001]
accomplishes: "A session with no port flag starts on a free port, so two reviews can run at once, while an explicitly chosen port still binds exactly or fails fast"
boundary: "Port resolution and binding only: config-layer rejection of explicit port 0, removal of the applied 7777 default, checked local_addr() reporting in serve_with_ready, plus the AGENTS.md/README/llms.txt port statements and the CHANGELOG entry for this behavior break. Single-listener path only."
non_scope:
  - "multi-listener / all-or-nothing startup boundary (CH-007)"
  - "typed session.started payload, apiBaseUrl/endpoints/proxyUrl (CH-002)"
  - "stderr wording change to `review UI/API: <url>` (CH-003)"
  - "skills/discuss/** rewrite and the repo guard test (CH-004)"
  - "issue #32 website proxy, proxy routing, any user-facing second listener"
  - "route paths, state protocol, SSE, history, verdict behavior"
provisional_tier: 3
risk_flags:
  - user-visible-default-behavior-change
  - config-validation-contract-change
  - public-lib-api-removal-default-port
  - startup-bind-path
  - exit-code-contract
expected_surface:
  files:
    - "src/lib.rs"
    - "src/server/mod.rs"
    - "src/config.rs"
    - "tests/cli.rs"
    - "AGENTS.md"
    - "README.md"
    - "llms.txt"
    - "CHANGELOG.md"
  subsystems: [cli, config, server, docs]
  languages: [rust, markdown]
known_unknowns:
  - "Whether `pub const DEFAULT_PORT` can be deleted outright or must be kept as a non-applied constant; it is exported from src/lib.rs and referenced by tests/docs (Decision 4 says delete it as an *applied* default)."
  - "Whether serde `deserialize_with` on ConfigLayer::port or a post-parse check preserves TOML span line/col for the zero rejection via config_parse_error."
  - "Whether serve_with_ready can report a checked local_addr() without restructuring bind/serve; CH-007 may later split allocation from serving."
  - "Concurrency test shape in tests/cli.rs: whether two spawned no-flag processes are stable under existing timeout helpers."
  - "llms.txt:25 mixes the port statement with the `listening on` wording owned by CH-003; overlap must be split, not duplicated."
task_kind_hint: localized_implementation
---

First seam of GH-33. Smallest coherent outcome that leaves the repo consistent: auto-allocation as the no-flag default, explicit ports still exact, explicit `0` rejected per layer, and every invariant/doc sentence that currently asserts the opposite updated in the same seam (Decision 13). `AGENTS.md:72` today forbids free-port fallback, so shipping the behavior without it would leave the repo self-contradictory.

Two plan decisions bind the planner: Decision 2 (no seam may preserve `src/server/mod.rs:86`'s `listener.local_addr().unwrap_or(addr)`; a `local_addr()` failure is a hard startup error mapped through `ServerBindError`, listeners dropped before side effects) and Decision 3 (`DISCUSS_PORT=0` and TOML `port = 0` rejected in `src/config.rs` with `DiscussError::ConfigParseError`, exit 2; `--port 0` keeps clap's `range(1..)`).

Expected commit sequence — three lines, so the planner should declare `commit_slots`:

1. Reject explicit port `0` at the env and TOML layers in `src/config.rs`, with per-layer unit tests asserting `ConfigParseError` and exit 2.
2. Bind `127.0.0.1:0` when `config.port` is `None`, stop applying `DEFAULT_PORT`, and report the address from a checked `local_addr()` in `serve_with_ready`; process-level tests for two concurrent no-flag sessions on distinct nonzero ports and for explicit-port collision → `PortInUse`/exit 3.
3. Update `AGENTS.md:72`, `README.md:66/74/156`, `llms.txt:38/56/72` (plus the `llms.txt:25` port statement) and add the `CHANGELOG.md` entry covering the default-port break and the zero-port rejection, with `--port 7777` named as the exact-restore path.

Acceptance evidence is the plan's CH-001 acceptance plus the Definition of Done commands: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo build --all-targets`, `cargo test`. Planner owns exact rows, files per slot, and verification commands, and may disagree with the count.
