---
artifact: master_plan
issue: GH-33
title: "Concurrent sessions: auto-allocate ports and emit an endpoint map"
created_by: master_planner
created_at: "2026-08-25"
---

# Concurrent sessions: auto-allocate ports and emit an endpoint map

- **Issue:** [#33](https://github.com/codesoda/discuss-cli/issues/33) — *Concurrent sessions: auto-allocate ports and emit an endpoint map*
- **Date:** 2026-08-25
- **Verified against:** `feat/gh33` @ `b074fde` (`chore: release v0.7.0`) — prefer symbol names over line numbers; line numbers cited here were re-read at `b074fde` and may drift.

## Benchmark Integrity Constraint (read first — binds every seam)

This session is a deliberate **benchmark re-run** of a closed issue. GH-33 was previously fixed on `main`. The worktree is rewound to `b074fde`, the commit immediately before that fix; **HEAD does not contain the fix**.

Every downstream role (seam scoper, planner, implementer, verifier, reviewer) inherits these rules:

- Treat the issue body as the authoritative spec. **Ignore the issue's CLOSED state and the existence of PR #35.**
- **Do not read the shipped implementation.** No `git show` / `git diff` / `git log -p` against `6bccbc4`, `b9ae74f`, `origin/main`, or any ref containing them. No `gh pr view/diff 35`. No reading files at those revisions. Those objects are reachable in this clone; treat them as off-limits. Consulting them invalidates the benchmark.
- `gh issue view 33` for the issue body is allowed. Do not act on its CLOSED state.
- **Handoff policy:** the PR is opened as a **draft** titled/marked "benchmark re-run", and issue #33 is **not** closed.
- Tooling is a closed world: `seams` (required) and `gh` (optional, `/opt/homebrew/bin/gh`). Do not use or probe other helpers.

## Overview / Problem

`discuss` binds one fixed default port. `src/lib.rs:run_review_session` computes `config.port.unwrap_or(DEFAULT_PORT)` (`DEFAULT_PORT = 7777`, `src/lib.rs:48`) and binds `127.0.0.1:<port>` through `server::serve_with_ready`. A second concurrent session collides and exits with `DiscussError::PortInUse` unless the caller hand-picks a port. Agents work around this by scanning `7777–7782` before launch (`skills/discuss/SKILL.md:200`), which is race-prone and encodes the port into agent behavior.

Issue #32 (live website proxying) will add a second listener, making manual collision management worse. Startup output is also not self-describing: `session.started` carries `url`, `mode`, `source_file`, `files_count`, `started_at`, plus `git_args` when diff mode received git arguments (`src/lib.rs:233-241`). None of those keys names an API endpoint, so an agent must construct every URL itself from a remembered port.

Affected: any user or agent running more than one review at a time, and every bundled skill/doc that repeats the launch contract.

## Goals

- No-flag sessions bind an OS-allocated loopback port; two concurrent sessions succeed with distinct URLs.
- Explicit port (`--port`, `DISCUSS_PORT`, config `port`) keeps exact binding and fail-fast collision behavior at every configuration layer.
- `session.started` becomes a self-describing machine contract: `url`, `apiBaseUrl`, optional `proxyUrl`, structured `endpoints`, short `agentInstructions`.
- Multi-listener startup is all-or-nothing: bind every listener before browser open or `session.started`; on partial failure close all and produce no startup side effect.
- The allocation + reporting contract is reusable by a second listener (issue #32) **without** implementing #32.
- Bundled skill, poller guidance, README, `AGENTS.md`, and `llms.txt` stop presenting `7777` as the unconditional default; a repo test guards against regression.

## Current State (evidence)

Port resolution — the zero-port gap:

- `src/cli.rs:Args::port` — `Option<u16>` with `value_parser = clap::value_parser!(u16).range(1..)` (`src/cli.rs:26-32`), so `--port 0` is rejected (`src/cli.rs:rejects_zero_port_override`).
- **No other layer rejects zero.** `src/config.rs:ConfigLayer` declares plain `port: Option<u16>` (`src/config.rs:135-136`) and `ConfigLayer::from_toml_str` is a bare `toml::from_str` (`src/config.rs:147`), so TOML `port = 0` deserializes cleanly. `DISCUSS_PORT` goes through `parse_env_var` (`src/config.rs:158`), whose body is `value.parse()` with no range check (`src/config.rs:220-233`). `ConfigLayer::apply_to` and `ConfigOverrides::apply_to` copy the value through unchecked (`src/config.rs:175-178`, `src/config.rs:103-105`).
- Consequence today: `DISCUSS_PORT=0` or TOML `port = 0` is an *explicit* setting that binds an OS-selected port. Under this issue's contract that silently converts an explicit request into auto-allocation, so `src/config.rs` is a code-change surface, not documentation.
- `src/config.rs:Config::port` is `Option<u16>`, default `None` (`src/config.rs:79`), layered file → env → CLI (`Config::resolve` / `resolve_with_sources`, `src/config.rs:32-60`). **The `Option` already distinguishes "explicitly configured" from "absent"** — no new explicitness flag is needed.
- `src/lib.rs:run_review_session` — `let port = config.port.unwrap_or(DEFAULT_PORT); let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));` (`src/lib.rs:211-212`). Sole application point of the fixed default.
- `src/config.rs:config_parse_error` (`src/config.rs:235-243`) builds `DiscussError::ConfigParseError { path, line, col, message }`; `parse_env_var` reuses that variant with `line: 0, col: 0` and the variable name as `path`. `src/exit.rs:exit_code_for_error` maps `ConfigParseError` to `EXIT_CONFIG_ERROR` (2) and `PortInUse` / `ServerBindError` to `EXIT_SERVER_ERROR` (3).

Binding — the `local_addr()` fallback is wrong for port 0:

- `src/server/mod.rs:serve_with_ready` calls `ensure_loopback(addr)?`, `TcpListener::bind(addr)`, then **`let listening_addr = listener.local_addr().unwrap_or(addr); on_ready(listening_addr);`** (`src/server/mod.rs:82-87`). With a requested `127.0.0.1:0`, that fallback yields **port 0** — the wrong answer precisely in the auto-allocation path. Existing code therefore does **not** guarantee a real bound address; the callback signature is reusable, the fallback is not.
- `src/server/mod.rs:bind_error` maps `ErrorKind::AddrInUse` to `DiscussError::PortInUse { port }`, else `ServerBindError`. `src/error.rs` renders "port N already in use … pass `--port <N>`".
- `src/server/mod.rs:ensure_loopback` rejects any non-`127.0.0.1` address.
- `serve_with_ready` owns bind, ready callback, idle timer, router, and `axum::serve` in one function; there is no separate allocation step and no way to hold a bound listener before serving.

Startup event and human output:

- `src/lib.rs:run_review_session` builds the payload inside the ready callback with `serde_json::json!` — `url` from `launch::loopback_url(listening_addr)`, plus `mode`, `source_file`, `files_count`, `started_at`, and conditionally `payload["git_args"]` (`src/lib.rs:229-241`) — then emits `Event { kind: EventKind::SessionStarted, .. }` via `app_state.emitter` (`src/events.rs:EventEmitter`; wire name at `src/events.rs:38`). **There is no typed startup-payload struct.**
- `src/launch.rs:loopback_url` — `format!("http://127.0.0.1:{}", addr.port())`.
- `src/launch.rs:announce_listening` — writes exactly `listening on {url}\n`, then calls `BrowserLauncher::open(url)` when `auto_open`, logging (not failing) browser errors. Called after emission (`src/lib.rs:259`).

Test surface:

- `tests/cli.rs` — process-level: `cli_emits_single_session_started_event_after_listening` asserts one stdout event with `payload.url == http://127.0.0.1:{port}`, `source_file`, `started_at`; a second test asserts the exact stderr line `listening on http://127.0.0.1:{port}\n`; a busy-port test covers `PortInUse`. Helper `free_port()` (`tests/cli.rs:299`) binds `127.0.0.1:0`.
- `tests/server.rs` — ~3.7k lines of route/SSE/state coverage; helper `free_loopback_addr()` (`tests/server.rs:3387`).
- `tests/theme.rs` — stylesheet guard test: **existing precedent for a repository-scanning guard**. `tests/logging.rs`; `tests/browser/html-prototype-smoke.mjs` is a manual `DISCUSS_URL`-driven CDP smoke.
- Unit tests: `src/launch.rs` tests hardcode `http://127.0.0.1:7777`; `src/cli.rs`, `src/config.rs`, `src/error.rs`, `src/exit.rs` cover port parsing and errors. `src/config.rs` has no zero-port test.
- CI (`.github/workflows/ci.yml:44-54`) runs `cargo fmt --check` → `cargo clippy --all-targets -- -D warnings` → `cargo build --all-targets` → `cargo test`, with workflow-level `RUSTFLAGS="-D warnings"`. No justfile/Makefile/CONTRIBUTING.

Docs and bundled integrations that encode `7777` or a remembered port:

- `skills/discuss/SKILL.md` — the only bundled skill (`skills/discuss/manifest.txt` lists `SKILL.md`, `poller.sh`; there is **no** `integrations/` directory). Line 186 documents the `listening on …` stderr line; lines 196-200 launch with `--port <port>` and say "pick a free port by checking which of 7777–7782 isn't already bound"; line 207 passes a base URL to `poller.sh`; line 242 shows a `session.started` sample with only `url`/`source_file`/`started_at`; line 245 says "don't hardcode `7777`" while the scan guidance contradicts it; Step 3 and the API table (lines 262-317) tell the agent to build `"$URL/api/threads/<thread-id>/takes"` by hand.
- `skills/discuss/poller.sh` — takes the base URL as `$1` and appends `/api/state`; no hardcoded port, but its contract is "base URL", not "endpoint map".
- `README.md:66`, `README.md:74` — "server launches on `http://127.0.0.1:7777`"; `README.md:156` — options table lists default `7777`, "No free-port fallback".
- `llms.txt:25` (`listening on http://127.0.0.1:<port>`), `llms.txt:38` ("Effective default: 7777"), `llms.txt:56`, `llms.txt:72` (config examples).
- `AGENTS.md:72` — invariant: "binds exactly `127.0.0.1:<port-or-7777>` via `serve_with_ready`; **do not add a free-port fallback because agent URLs must stay predictable**" — this issue reverses it. `AGENTS.md:73` pins the `listening on …` stderr line; `AGENTS.md:75` pins the `session.started` payload keys.
- Incidental `7777` that is *not* contract: `src/launch.rs`, `src/error.rs`, `src/exit.rs`, `src/config.rs` test fixtures; `Cargo.lock` checksum bytes; `assets/mermaid.min.js` minified bytes. Any scanner must exclude these.

## Desired End State

- No explicit port → API listener bound to `127.0.0.1:0`; every reported URL derives from a **checked** `TcpListener::local_addr()`.
- Explicit port from any layer → bind that exact port; `AddrInUse` fails with `PortInUse` and exit code 3. No fallback. An explicit zero is rejected as configuration error before any bind.
- A listener-allocation step accepts N specs (each explicit-or-auto), binds them all before any side effect, and on any failure — including a `local_addr()` failure — drops every already-bound listener and returns the mapped error, producing no `session.started`, no stderr endpoint line, and no browser open.
- A payload builder turns (primary bound addr, optional secondary bound addr, existing session fields) into the `session.started` payload:

```json
{"kind":"session.started","payload":{"url":"http://127.0.0.1:49152","apiBaseUrl":"http://127.0.0.1:49152","proxyUrl":"http://127.0.0.1:49153","endpoints":{"state":"…/api/state","events":"…/api/events","createThread":"…/api/threads","addTakeTemplate":"…/api/threads/{threadId}/takes","done":"…/api/done"},"agentInstructions":["…"]}}
```

  with `proxyUrl` omitted (key absent) when there is no secondary listener, and existing keys (`mode`, `source_file`, `files_count`, `started_at`, `git_args`) retained.
- stderr shows `review UI/API: <url>`, plus `website proxy: <url>` only when a secondary listener exists.
- The bundled skill, README, `llms.txt`, and `AGENTS.md` describe auto-allocation as the default, endpoint-map consumption as the agent contract, and explicit `--port` as the predictable-URL escape hatch.
- A repository test fails if bundled skill/integration files reconstruct endpoints from a hardcoded port.

## Locked Decisions

1. **Explicitness is modeled by the existing `Config::port: Option<u16>`; `None` means auto.** Every layer already collapses to this Option, so no new "explicit" flag is warranted.
2. **Auto = bind `127.0.0.1:0` and read the actual address from a checked `local_addr()`. Never scan-then-bind, and never fall back to the requested address.** The current `listener.local_addr().unwrap_or(addr)` (`src/server/mod.rs:86`) returns port 0 in exactly the auto path, so a `local_addr()` error must propagate as a hard startup error (mapped through `DiscussError::ServerBindError`), dropping acquired listeners before any side effect. No seam may preserve the `unwrap_or` fallback.
3. **Port `0` is rejected as an explicit setting at every layer, not just clap.** `--port 0` keeps clap's `range(1..)` rejection; `DISCUSS_PORT=0` and TOML `port = 0` are rejected in `src/config.rs` with `DiscussError::ConfigParseError` (env: variable name as `path`, `line: 0, col: 0`, per `parse_env_var`; TOML: the config file path, with span line/col when available per `config_parse_error`), exiting `EXIT_CONFIG_ERROR` (2). Rationale: "auto" must be expressed by *omitting* the port; accepting an explicit zero would silently turn a deterministic request into auto-allocation and break FR-2.
4. **Delete `DEFAULT_PORT` as an applied default.** Leaving a fixed default reachable lets the old behavior creep back; `7777` survives only as user-supplied input.
5. **Payload keys are exactly as specified in the issue** (`apiBaseUrl`, `proxyUrl`, `endpoints.{state,events,createThread,addTakeTemplate,done}`, `agentInstructions`), camelCase, alongside the existing snake_case keys. The mixed casing is deliberate: the issue JSON is the durable contract and existing keys must not break criterion 10 or `tests/cli.rs`.
6. **`url` is retained as an alias of `apiBaseUrl`.** Real consumers read it (`tests/cli.rs`, `skills/discuss/SKILL.md:245`); keeping it is additive and free.
7. **`proxyUrl` is omitted, not null, when absent** (issue: "optional and omitted").
8. **The startup payload gets a typed builder** (a dedicated function/struct, e.g. `src/endpoints.rs`), not inline `serde_json::json!`. This makes criterion 8 testable without #32: a unit test builds the payload with a synthetic second bound address and asserts `proxyUrl` presence and derivation; an ordinary-session test asserts absence.
9. **Multi-listener support ships as an internal, tested capability with N=1 in production.** No proxy feature, no user-facing second listener. Criterion 7 is discharged at the **orchestration boundary** (Decision 10), not by the allocation helper alone.
10. **Criterion 7 needs an orchestration-level observable.** A bind helper with no emitter, stderr writer, or browser launcher cannot prove side effects were suppressed. The seam must expose a startup boundary that owns allocation *and* the startup side effects, with injectable collaborators (reuse the `BrowserLauncher` trait and a `Write` sink as `announce_listening` already does, plus a capturable event sink). The test injects two specs, forces the second bind to fail, and asserts: no `session.started` emitted, no endpoint line written, no browser opened, and the first listener's address is re-bindable afterwards. The helper-level cleanup test remains as supporting evidence only.
11. **stderr wording becomes `review UI/API: <url>` / `website proxy: <url>`**, replacing `listening on <url>`, exactly as the issue specifies.
12. **All listeners remain loopback-only**; `ensure_loopback` applies to every spec. No security-boundary change here.
13. **Every behavior change ships with the invariants and docs that describe it.** `AGENTS.md`, `README.md`, `llms.txt`, and `CHANGELOG.md` lines describing a changed behavior are part of that behavior's outcome, never a follow-up. `AGENTS.md:72` actively forbids auto-allocation, so shipping it separately would leave the repo self-contradictory.
14. **Handoff is a draft PR marked "benchmark re-run"; issue #33 is not closed** (benchmark constraint above).

## Invariants & Constraints

- **Stdout is machine-only NDJSON; human text goes to stderr or tracing** (`AGENTS.md:76`; `src/events.rs:EventEmitter`; `src/logging.rs` logs to file only). Endpoint lines go to stderr.
- **Loopback only** — `src/server/mod.rs:ensure_loopback` must gate every listener; no `0.0.0.0` (`AGENTS.md` server bootstrap invariant).
- **Errors flow through `discuss::DiscussError` / `discuss::Result`, mapped by `exit_code_for_error`** (`AGENTS.md` Rust Patterns; `src/error.rs`; `src/exit.rs`). Bind failures keep `PortInUse` / `ServerBindError` (exit 3); configuration rejections keep `ConfigParseError` (exit 2).
- **clap definitions stay in `src/cli.rs`; config defaults, layering, and validation stay in `src/config.rs`; `src/main.rs` stays thin** (`AGENTS.md` Rust Patterns). Config file errors must preserve the config path and line/column (`AGENTS.md` Rust Patterns; `Config::from_toml_str`).
- **Browser-open failure warns via tracing and never fails the session** (`src/launch.rs:announce_listening`, `AGENTS.md:73`).
- **`AGENTS.md` invariants are part of the change surface**: `AGENTS.md:72` (`<port-or-7777>`, no free-port fallback), `AGENTS.md:73` (`listening on …` line), `AGENTS.md:75` (`session.started` payload keys). A stale invariant is a defect (Decision 13).
- **Surgical changes** (`AGENTS.md` §3): no refactor of adjacent server/state code.
- **Simplicity** (`AGENTS.md` §2): no proxy implementation, no configurable listener registry, no abstraction beyond the two-listener contract the issue names.
- **CI must stay green under `RUSTFLAGS="-D warnings"`** with the `fmt → clippy --all-targets → build --all-targets → test` order (`.github/workflows/ci.yml`, `AGENTS.md`).
- **Bundled skill files are shipped artifacts** (the installer links `skills/discuss/` into agent roots, `README.md:34`); `skills/discuss/manifest.txt` must list any new file.

## Definition of Done (applies to every seam)

- `cargo fmt --check` clean.
- `cargo clippy --all-targets -- -D warnings` clean.
- `cargo build --all-targets` clean.
- `cargo test` green (criterion 10's regression bar: markdown, diff, image, static HTML, shutdown, history suites in `tests/server.rs` and `tests/cli.rs`).
- New non-trivial branches covered by tests (auto vs explicit, explicit zero per layer, `local_addr()` failure mapping, proxy present vs absent, partial-bind failure).
- Invariants and docs describing a changed behavior updated in the same seam as that behavior (Decision 13) — never deferred.
- No hand-edited generated artifacts (`Cargo.lock` only via cargo; `assets/mermaid.min.js` untouched).
- Benchmark integrity constraint respected (no reading of the shipped fix).

## Patterns & Utilities to Reuse

- `src/server/mod.rs:serve_with_ready` ready-callback **shape** — the `FnOnce(SocketAddr)` handoff is the right seam for reporting the bound address; its `unwrap_or(addr)` fallback is not reusable (Decision 2).
- `src/launch.rs:loopback_url` for URL formatting; the `BrowserLauncher` trait plus the existing `FakeLauncher` test double and `Write` sink in `src/launch.rs` tests — the injection pattern Decision 10 builds on.
- `src/events.rs:EventEmitter` / `Event` / `EventKind::SessionStarted` for emission; no new event kind.
- `src/error.rs:DiscussError::{PortInUse, ServerBindError, ConfigParseError}` plus `src/config.rs:config_parse_error` and `parse_env_var` for the zero-port rejection contract; `src/exit.rs:exit_code_for_error` for exit codes.
- `tests/cli.rs:free_port` and `tests/server.rs:free_loopback_addr` for occupying/allocating ports in tests.
- `tests/theme.rs` — precedent for a repository-scanning guard test.
- `src/state/types.rs` camelCase serde conventions for the new payload struct.

## Functional Requirements

- **FR-1:** When no port is explicitly configured by CLI, env, or config, the API listener binds `127.0.0.1:0` and all reported addresses derive from a checked `local_addr()`; a `local_addr()` failure is a hard startup error, never a fallback to the requested address.
- **FR-2:** When a port is explicitly configured at any layer, the API listener binds exactly that port and fails with `PortInUse` (exit 3) on collision; it never falls back. An explicit port of `0` — via `--port`, `DISCUSS_PORT`, or TOML `port` — is rejected before any bind with a configuration error (exit 2) naming the offending source.
- **FR-3:** Listener allocation performs an actual bind per listener; no pre-bind free-port scan exists in production code.
- **FR-4:** All required listeners are bound before `session.started` is emitted, before any stderr endpoint line, and before the browser is opened.
- **FR-5:** If any required listener fails to bind or to report its address, every already-bound listener is closed, no `session.started` is emitted, no endpoint line is written, no browser opens, and the process exits with the mapped error code.
- **FR-6:** `session.started.payload` includes `url`, `apiBaseUrl`, and `endpoints` with `state`, `events`, `createThread`, `addTakeTemplate` (containing the literal `{threadId}`), and `done`, each absolute and derived from the bound API address.
- **FR-7:** `session.started.payload.proxyUrl` is omitted when no secondary listener exists and carries the secondary listener's bound URL when one does.
- **FR-8:** `session.started.payload.agentInstructions` is a short ordered list of sequencing guidance and never the sole source of endpoint information.
- **FR-9:** Existing `session.started` keys (`mode`, `source_file`, `files_count`, `started_at`, optional `git_args`) are preserved.
- **FR-10:** stderr prints `review UI/API: <url>`, and `website proxy: <url>` only when a secondary listener exists; stdout stays pure NDJSON.
- **FR-11:** All listeners bind loopback only.
- **FR-12:** `skills/discuss/SKILL.md` and every bundled integration instruct agents to wait for `session.started`, treat `payload.url`/`apiBaseUrl`/`endpoints` as authoritative, substitute `{threadId}` into `addTakeTemplate`, use the reported `state`/`events`/`done` endpoints for the whole session, retain the endpoint map as session state across monitor wake-ups and follow-up turns, and reach for explicit `--port` only when a predictable URL is required.
- **FR-13:** A repository test fails when bundled skill/integration files construct endpoints from a hardcoded port; documentation may mention `7777` only when explaining explicit-port compatibility.
- **FR-14:** `README.md`, `llms.txt`, `AGENTS.md`, and `CHANGELOG.md` describe auto-allocation as the no-flag default, the new startup reporting, and explicit `--port` as the deterministic option.

## Scope Areas (backlog — NOT seams)

Each area is an independently shippable outcome: it leaves the repository consistent, including the invariants and docs describing the behavior it changes (Decision 13). Dependency-ordered.

- [ ] **CH-001 — Sessions bind an auto-allocated port unless one is explicitly configured.** Acceptance: two concurrent no-flag sessions both start and serve `/api/state` on distinct ports; a no-flag session reports a nonzero bound port (no `unwrap_or(addr)` fallback survives; a `local_addr()` failure maps to a startup error); an explicit `--port`/`DISCUSS_PORT`/TOML port binds exactly and fails with `PortInUse`/exit 3 when occupied; explicit `0` from env and TOML is rejected with a configuration error and exit 2, covered by `src/config.rs` unit tests per layer; no scan-then-bind path exists in production code; `AGENTS.md:72`, `README.md:66/74/156`, `llms.txt:25/38/56/72` port statements, and a `CHANGELOG.md` entry for the default-port behavior change land with it. Likely touches: `src/lib.rs`, `src/server/mod.rs`, `src/config.rs`, `src/launch.rs`, `tests/cli.rs`, `AGENTS.md`, `README.md`, `llms.txt`, `CHANGELOG.md`. (Refs: FR-1, FR-2, FR-3, FR-11, FR-14)
- [ ] **CH-007 — Startup binds every required listener all-or-nothing before any side effect.** Acceptance: a startup boundary owning allocation *and* startup side effects accepts N listener specs with injectable event sink, stderr writer, and `BrowserLauncher`; with two specs where the second bind fails, a test asserts no `session.started`, no endpoint line, no browser open, and that the first listener's address is re-bindable afterwards; a helper-level test additionally shows every acquired listener is dropped; production still runs with N=1; nothing in the API surface implies a proxy feature exists. Likely touches: `src/lib.rs`, `src/server/mod.rs`, `src/launch.rs`, unit tests in those modules. (Refs: FR-3, FR-4, FR-5, FR-11)
- [ ] **CH-002 — `session.started` reports a self-describing endpoint map.** Acceptance: a typed builder replaces the inline `serde_json::json!` payload; the emitted payload matches the issue's shape with every endpoint derived from the bound address and `addTakeTemplate` containing the literal `{threadId}`; a take POSTed to the reported `addTakeTemplate` with `{threadId}` substituted appears in that session's `/api/state`; `proxyUrl` absent for ordinary sessions and present/derived when the builder is given a second bound address (unit test); `mode`, `source_file`, `files_count`, `started_at`, `git_args` preserved; `AGENTS.md:75` and the `skills/discuss/SKILL.md:242` sample event updated with it. Likely touches: new `src/endpoints.rs` (or equivalent), `src/lib.rs`, `tests/cli.rs`, `tests/server.rs`, `AGENTS.md`, `skills/discuss/SKILL.md`. (Refs: FR-6, FR-7, FR-8, FR-9)
- [ ] **CH-003 — Startup prints human-readable endpoint lines on stderr.** Acceptance: stderr emits `review UI/API: <url>`; `website proxy: <url>` appears only when a secondary listener exists; stdout remains parseable NDJSON with exactly one startup event; the browser is opened with the reported review URL and a browser failure still only warns; `AGENTS.md:73`, `llms.txt:25`, and `skills/discuss/SKILL.md:186` land with it. Likely touches: `src/launch.rs`, `src/lib.rs`, `tests/cli.rs`, `AGENTS.md`, `llms.txt`, `skills/discuss/SKILL.md`. (Refs: FR-4, FR-10)
- [ ] **CH-004 — Bundled agent integrations consume the endpoint map, guarded by a repo test.** Acceptance: `skills/discuss/SKILL.md` and `skills/discuss/poller.sh` guidance satisfy FR-12 with the `7777–7782` scan guidance and hand-built take URLs removed; a repository test scans bundled skill/integration files and fails on hardcoded endpoint construction while permitting explicit-port compatibility prose, and excludes `Cargo.lock`/`assets/`; the guard fails against the pre-change SKILL.md text and passes against the repository as it stands when the outcome lands; `skills/discuss/manifest.txt` lists any added file. Likely touches: `skills/discuss/**`, a new test in `tests/` modeled on `tests/theme.rs`, `README.md`. (Refs: FR-12, FR-13, FR-14)
- [x] **CH-005 — dropped.** Standalone "docs and project invariants" outcome removed: it deferred `AGENTS.md`/README/`llms.txt` updates away from the behavior they describe, contradicting Decision 13 and the Definition of Done. Its content is now acceptance in CH-001, CH-002, CH-003, and CH-004.
- [x] **CH-006 — dropped.** Standalone "acceptance-level regression coverage" outcome removed: it was a verification task duplicating CH-001/CH-002 acceptance and pre-carved seam grouping. Concurrency, collision, and end-to-end regression checks now sit in the runtime outcomes that own the behavior.

> Ordering is a dependency hint (allocation → all-or-nothing startup → payload → human output → integrations), not a seam sequence. Seam grouping is the scoper's call.

## Out of Scope (Non-Goals)

- Implementing issue #32's website proxy, proxy routing, or any user-facing second listener. Only the reusable contract ships.
- Non-loopback binding, TLS, auth, or any other security-boundary change.
- Session discovery/registry, "attach to an existing session", or a lock/PID file.
- Changing `/api/*` route paths, state protocol types, SSE semantics, history/transcript behavior, or verdict handling.
- Renaming or restructuring existing `session.started` keys.
- Adding a `--port 0` / `--port auto` alias.
- Browser UI changes in `discuss.html` or `assets/`.
- Closing issue #33 or opening a non-draft PR.

## Risks & Open Questions

- **Guard-test false positives.** A naive `7777` scanner trips on `Cargo.lock`, `assets/mermaid.min.js`, and legitimate explicit-port prose. The scanner must target bundled skill/integration files and hardcoded endpoint *construction*, not the digit string (CH-004 acceptance).
- **Criterion 8 testability.** `proxyUrl` cannot be produced end-to-end without #32. Mitigation is locked in Decisions 8/9: contract-level tests over the payload builder with a synthetic second bound address. If a seam finds those unconvincing, re-plan rather than stubbing a fake proxy listener into production.
- **Criterion 7 observability.** Suppression of startup side effects is only provable where those collaborators live (Decision 10). If the startup boundary proves hard to inject without restructuring `serve_with_ready`, flag it rather than downgrading to a helper-only test.
- **Behavior change is user-visible.** Anyone relying on `http://127.0.0.1:7777` after a bare `discuss` must now pass `--port 7777`. The `CHANGELOG.md` entry is CH-001 acceptance, not a nice-to-have.
- **Zero-port rejection is itself a behavior change** for anyone setting `DISCUSS_PORT=0` or TOML `port = 0` today. Judged correct under FR-2 and noted in the same CHANGELOG entry.
- **Stderr wording change breaks scripted greps** for `listening on`; CH-003 carries the doc updates.
- **Test flakiness from real binds.** Concurrency tests spawn real processes; follow existing `tests/cli.rs` timeout patterns and never assert specific port numbers.
- No open questions block planning.

## Verification Strategy & Success Metrics

Project commands (from `.github/workflows/ci.yml`; the Rust/bugatti commands in the shared Verification Policy belong to a different project and do not apply here):

- `cargo fmt --check`
- `cargo clippy --all-targets -- -D warnings`
- `cargo build --all-targets`
- `cargo test` (full suite; `cargo test --test cli`, `--test server` for targeted loops)

End-to-end bar, mapped to the issue's acceptance criteria:

1. Two concurrent no-flag sessions → both emit `session.started` with distinct, nonzero `apiBaseUrl` ports and independently reachable `/api/state` (CH-001).
2. Every reported endpoint string contains the actual bound port, taken from a checked `local_addr()` (CH-001, CH-002).
3. The browser is opened with the reported review URL, asserted through the injected `BrowserLauncher` double (CH-003).
4. A take POSTed to the reported `addTakeTemplate` (with `{threadId}` substituted) appears in that session's `/api/state` (CH-002).
5. Explicit `--port` on an occupied port exits with `PortInUse` and exit code 3; explicit `0` from env or TOML exits 2 before binding (CH-001).
6. Allocation performs real binds; no scan helper exists in production code (CH-001/CH-007, review plus test).
7. Partial bind failure at the **startup boundary** with two specs asserts no `session.started`, no stderr endpoint line, no browser open, and a re-bindable first address; the allocation-helper test supports this as cleanup evidence only (CH-007).
8. `proxyUrl` omitted for ordinary sessions, present and correctly derived when a second bound address is supplied to the builder (CH-002).
9. Bundled skill/integration files consume `payload.endpoints`, and the guard test blocks hardcoded endpoint construction (CH-004).
10. Full `cargo test` green (all outcomes).

`tests/browser/html-prototype-smoke.mjs` is manual and env-driven; it is not part of the automated bar.

## Rollback / Safety

No schema, migration, or persisted-state change. Rollback is reverting the seam commits; the only durable artifacts are history archives (`src/history.rs`), whose format is untouched. Deploy consideration is release-note framing only: the no-flag default-port change and the explicit-zero rejection are behavior breaks announced in `CHANGELOG.md`, with `--port 7777` documented as the exact-restore path.

## Progress Log

_Append-only as seams land._
