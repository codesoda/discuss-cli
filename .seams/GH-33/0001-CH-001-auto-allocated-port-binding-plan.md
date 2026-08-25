---
artifact: seam_plan
issue: GH-33
seam_id: "0001"
title: Sessions bind an auto-allocated port unless one is explicitly configured
tier: 3
checklist_ids:
  - CH-001
seam_pr_shape: stacked_pr
depends_on: []
expected_files:
  - src/config.rs
  - src/lib.rs
  - src/server/mod.rs
  - tests/cli.rs
  - tests/server.rs
  - AGENTS.md
  - README.md
  - llms.txt
  - CHANGELOG.md
risk_flags:
  - user-visible-default-behavior-change
  - config-validation-contract-change
  - public-lib-api-removal-default-port
  - startup-bind-path
  - exit-code-contract
requires_human_approval: true
seam_kind: implementation
created_by: seam_planner
created_at: "2026-08-24"
commit_slots: 3
branch: feat/gh33-ch-001-auto-allocated-port-binding
pr_strategy: stacked_prs
merge_strategy: 'Stack on the GH-33 base PR (branch feat/gh33); draft PR marked "benchmark re-run", issue #33 not closed'
plan_doc: docs/plans/2026-08-25-concurrent-sessions-auto-allocate-ports.md
description: First outcome of GH-33. Makes an OS-allocated loopback port the default so two review sessions can run concurrently, while keeping explicitly configured ports exact and fail-fast, and rejecting an explicit port 0 at the env and TOML layers. Ships with the AGENTS.md/README/llms.txt invariants and the CHANGELOG entry that describe the changed behavior, per master-plan Decision 13.
summary: No-flag sessions bind 127.0.0.1:0 and report a checked local_addr(); explicit ports still bind exactly, and explicit 0 becomes a config error.
key_points:
  - Deletes DEFAULT_PORT and binds 127.0.0.1:0 when config.port is None
  - Removes the local_addr().unwrap_or(addr) fallback per Decision 2, mapping failure to ServerBindError
  - 'Three commit slots: config zero-rejection, auto-allocation, then docs and CHANGELOG'
---

# Seam Plan: 0001 - Auto-allocated port binding

Front matter above is the skim index. Sections below are the reviewable body.

## Human Summary

A review session started without a port flag now takes a free port the operating
system picks, so two reviews can run at once without colliding. A port you ask for
explicitly still binds exactly and still fails fast when it is already taken. Asking
for port `0` becomes a configuration error instead of a silent request for a random
port.

Why now: this is the first outcome of GH-33, and every later one — the endpoint map,
the new startup lines, the skill rewrite — assumes the port is no longer fixed.

Observable change: a bare `discuss review.md` no longer lands on port 7777. Anyone
who relied on that address passes `--port 7777` to restore it. The URL printed at
startup and reported in the `session.started` event is the real bound port.

Tier 3 detail. The risky change is the default bind address: with no explicit port,
sessions move from a fixed loopback port 7777 to a request for any free loopback
port, with the actual port read back from the listener. The single Proposed Code
Changes entry carrying it is the `run_review_session` startup function, at its
`config.port.unwrap_or(DEFAULT_PORT)` line. Blast radius: the default startup URL of
every no-flag session, plus rejection of two previously-accepted configuration
values, `DISCUSS_PORT=0` and a config-file `port = 0`. No stored data, schema, auth,
or route path changes. Out of scope: the `session.started` endpoint map, the startup
wording change, multi-listener startup, and the bundled skill.
## Discovery Summary

- The fixed default has one definition and one use: `src/lib.rs:48` and `:211`.
  `grep -rn DEFAULT_PORT`, excluding `.seams/` and `docs/plans/`, returns nothing else
  — no test, no doc. Deleting it breaks no in-repo caller.
- `src/server/mod.rs:83-89` places `TcpListener::bind` at `:83-85`, the
  `local_addr().unwrap_or(addr)` fallback at `:86`, `on_ready` at `:87`, and
  `spawn_idle_timer` at `:89`. An early `?` at `:86` therefore returns before every
  side effect and drops the listener (known unknown 3). Line 86 is also the file's
  only `unwrap_or`, which criterion a10 relies on.
- `src/server/mod.rs` has no `#[cfg(test)] mod tests`; the file ends at `:287` with
  `bind_error`. c2 adds the first one.
- Surprise: the serve-time error mapping at `:104` builds `ServerBindError { addr,
  source }` from the *requested* address, which renders as `127.0.0.1:0` under
  auto-allocation. This seam owns it. `ensure_loopback` (`:264-276`) inspects only the
  IP, so `127.0.0.1:0` passes unchanged.
- Zero-port gap per layer: `src/cli.rs:29` has `.range(1..)`, but `Config::port`
  (`src/config.rs:18`) and `ConfigLayer::port` (`:136`) are plain `Option<u16>`, both
  `from_toml_str` entry points (`:28-30`, `:146-148`) are bare `toml::from_str`, and
  `parse_env_var` (`:220-232`) is a bare `value.parse()`. Both TOML entry points need
  the guard, not just the private layer.
- No existing test parses a TOML document that omits `port`:
  `deserializes_partial_toml_using_config_defaults` supplies `port = 9999` (`:321`),
  and `resolve_ignores_missing_config_files` points at `missing-user.toml` /
  `missing-project.toml` (`:367-368`) and parses no TOML at all. The
  `deserialize_with` missing-field path is therefore uncovered today.
- TOML spans survive a custom deserializer (known unknown 2):
  `toml_edit-0.22.27/src/de/table.rs:162-178` attaches the value's span to any error
  whose span is `None`, including `serde::de::Error::custom` from a `deserialize_with`.
  `rejects_unknown_fields_as_config_parse_errors` (`src/config.rs:337`) already proves
  the span reaches `config_parse_error` as `line == 3, col > 0` (`:355-356`).
- Test harness: `tests/cli.rs` spawns the real binary with piped stdio, reads the first
  pipe line through `read_first_line` (`:305-319`) under `recv_timeout`, and uses
  `wait_with_timeout` (`:368-389`) for exit assertions. No sleep-based readiness exists
  in it, so a deterministic concurrency test fits the existing shape (known unknown 4).
- No `discuss.config.toml` exists at the repository root, so `cargo test` picks up no
  project config layer. New tests must still set `current_dir` and
  `env_remove("DISCUSS_PORT")` so a developer's environment cannot leak a port in.
- `CHANGELOG.md:7` is an empty `## [Unreleased]` section, Keep-a-Changelog format.
## Seam Boundary

- **Approval outcome now**: no-flag sessions bind an OS-allocated loopback port and
  report the checked bound address; explicitly configured ports still bind exactly or
  fail fast; explicit `0` is rejected at the env and TOML layers; and every in-repo
  sentence asserting the old fixed-port contract is corrected. Lands as the three
  commits in Planned Commits.
- **Checklist coverage**: CH-001, fully.
- **Explicit non-scope**: the `session.started` endpoint map and `apiBaseUrl` (CH-002);
  the `review UI/API:` stderr wording and the literal `listening on …` token at
  `llms.txt:25`, `AGENTS.md:73`, `skills/discuss/SKILL.md:186` (CH-003); the injectable
  multi-listener startup boundary (CH-007); the skill rewrite and repository guard test
  (CH-004). `session.started` keeps its current key set; the only change is that `url`
  carries the actually-bound port.
- **Why this is one seam**: under Decision 13 the behavior change and the sentences
  describing it are one reviewable outcome — `AGENTS.md:72` currently forbids exactly
  what c2 introduces, so c2 without c3 leaves the repository self-contradictory. c1
  must precede c2, because once `None` means auto an accepted explicit `0` would
  silently mean auto too. Ordered steps toward one outcome, so: commit slots.
- **No CH-007 groundwork.** The `local_addr_or_bind_error` helper is an error-mapping
  extraction, not Decision 10's orchestration boundary: it takes no event sink, no
  `Write` sink, and no `BrowserLauncher`, and production keeps a single listener.
  CH-007 still owns collaborator injection and the all-or-nothing multi-listener proof.
## Tier Rationale and Gates

- **Tier evidence**: Tier 3 per **Approval Tiers** "behavior changes" — the default
  bind address of every no-flag session changes, and two configuration values that
  are accepted today start exiting 2. Matches the scope's `provisional_tier: 3`.
- **Gate impact**: reviewer approval plus a `/discuss` human plan approval before
  implementation, then report-reviewer approval plus a `/discuss` human approval
  before the final commit and any publication. Up to two plan-review rounds.
- **Risk alignment**: the risk matrix carries one user-visible break and one contract
  tightening, both from the same outcome and both answered by the same CHANGELOG
  entry. No unrelated risk domain is bundled, so the tier rests on one concern.
## Design Decisions

**1. Delete `pub const DEFAULT_PORT` outright (known unknown 1).**
Options: delete; keep it unapplied as documentation; keep it `#[deprecated]`. Chosen:
delete. Nothing in-repo references it beyond its single use site, so nothing breaks.
Public-API consequence: `discuss::DEFAULT_PORT` leaves the library surface. Acceptable
— the crate is `0.7.0` (`Cargo.toml:3`), `0.x` semver permits it, `discuss` ships as a
binary, and CHANGELOG `### Removed` announces it. Keeping the constant unapplied is
precisely the creep-back risk Decision 4 names: a ready-made value for a future
`unwrap_or`.

**2. Reject explicit zero via a serde `deserialize_with` for TOML plus a wrapper for
env, sharing one message (known unknown 2).**
Options: (a) post-parse validation inside `from_toml_str`; (b) `deserialize_with` on
the `port` fields. Chosen: (b). Option (a) discards the span and reports
`line 0, column 0` for a config *file*, contradicting the `AGENTS.md` Rust Patterns
rule that file errors preserve path and line/column. Option (b) keeps the span for
free via the `toml_edit` attachment cited in Discovery, which `config_parse_error`
(`src/config.rs:235-247`) already converts. The deserializer goes on **both**
`Config::port` and `ConfigLayer::port`, so the public and layered TOML paths agree.
`DISCUSS_PORT` never passes through serde, so `parse_env_port` re-checks after
`parse_env_var`. Both paths emit `PORT_ZERO_MESSAGE`, giving tests one substring to
assert. `deserialize_with` makes a field required unless a default is in scope, so the
field-level `#[serde(default)]` in Proposed Code Changes is load-bearing, not
decoration — and no test in the repository currently parses a TOML document that omits
`port`, so the Test Plan adds that coverage rather than assuming it.

**3. Extract the `local_addr()` error mapping into a private, directly testable helper
(known unknown 3).**
Options: (a) inline `map_err(…)?` at `src/server/mod.rs:86`; (b) inline plus a
structural grep as the only evidence; (c) a private helper taking the `local_addr()`
`Result`, unit-tested for both arms. Chosen: (c). Option (b) was the first draft and is
insufficient: absence of two exact strings does not prove an `io::Error` becomes
`DiscussError::ServerBindError`, and rustfmt could split the old expression across
lines and evade the pattern. The helper is the smallest surface that makes the mapping
executable, because a real `local_addr()` failure cannot be induced on a live listener.
It is **not** Decision 10's orchestration boundary and adds no collaborator injection:
`src/server/mod.rs:83-89` shows the `?` sits structurally before `on_ready` (`:87`) and
`spawn_idle_timer` (`:89`), and `listener` is a local dropped on that return, so the
failure path already emits no `session.started`, writes no stderr line, opens no
browser, and releases the socket. Splitting allocation from serving remains CH-007's
restructuring and stays out of scope.

**4. The concurrency test synchronises on the startup event, never on time
(known unknown 4).**
Options: sleep-then-probe; poll `/api/state` until it answers; block on each child's
first stdout NDJSON line. Chosen: the third. `session.started` is emitted from the
ready callback strictly after a successful bind (`src/lib.rs:229`), making that line a
happens-after-bind signal with no timing assumption. Both children are spawned before
either is read, so scheduler order is irrelevant. Ports are asserted only for
nonzero-ness and distinctness, never by value. `current_dir` is a temp directory and
`DISCUSS_PORT` is removed, so no ambient config layer applies.

**5. `llms.txt:25` splits by sentence, not by line ownership (known unknown 5).**
This seam edits the lead-in at `llms.txt:24` and leaves the literal
`  listening on http://127.0.0.1:<port>` at `:25` byte-identical. CH-003 owns that
token; claiming it here would assert an outcome this seam does not ship.

**6. The env-layer error keeps `line: 0, col: 0`.**
Accepted per Decision 3 rather than fixed. `ConfigParseError` (`src/error.rs:27-35`)
renders "at line 0, column 0" for `DISCUSS_PORT`, which is awkward but is already the
convention for every env-var error (`src/config.rs:220-232`) and for unreadable config
files (`:207`). Making the position optional would change the variant's shape and
every consumer — out of scope.
## Proposed Code Changes

All entries advance CH-001.

### `src/config.rs` — per-layer zero rejection (c1)

```rust
const PORT_ZERO_MESSAGE: &str =
    "port 0 is not a valid bind port; omit `port` to let the operating system choose a free port";

fn deserialize_port<'de, D>(deserializer: D) -> std::result::Result<Option<u16>, D::Error>
where
    D: serde::Deserializer<'de>;

fn parse_env_port(name: &str, value: &str) -> Result<u16>;
```

- `deserialize_port` deserializes `Option<u16>` and, on `Some(0)`, returns
  `serde::de::Error::custom(PORT_ZERO_MESSAGE)`. All other values pass through.
- Annotate both port fields `#[serde(default, deserialize_with = "deserialize_port")]`:
  `Config::port` (`:18`) and `ConfigLayer::port` (`:136`). The field-level `default` is
  required on `ConfigLayer::port`, whose container has no `default` attribute and whose
  implicit `Option` default `deserialize_with` suppresses; it is stated on
  `Config::port` for symmetry. Neither `deny_unknown_fields` changes.
- `parse_env_port` calls `parse_env_var::<u16>(name, value)?` and, on `0`, returns
  `DiscussError::ConfigParseError { path: PathBuf::from(name), line: 0, col: 0,
  message: format!("invalid value {value:?}: {PORT_ZERO_MESSAGE}") }` — the same shape
  `parse_env_var` already produces (Design Decision 6).
- `:158` becomes `"DISCUSS_PORT" => layer.port = Some(parse_env_port(&name, &value)?),`.
  No other `from_env` arm changes, and neither `ConfigOverrides` nor `apply_to`
  changes: clap's `.range(1..)` (`src/cli.rs:29`) already rejects `--port 0`.

### `src/server/mod.rs` — checked `local_addr()` (c2)

Add a private helper beside the existing `bind_error` (`:278-287`) and call it at
line 86:

```rust
fn local_addr_or_bind_error(
    requested: SocketAddr,
    local_addr: io::Result<SocketAddr>,
) -> Result<SocketAddr>;

// at :86, replacing `listener.local_addr().unwrap_or(addr)`
let listening_addr = local_addr_or_bind_error(addr, listener.local_addr())?;
```

The helper maps `Err(source)` to `DiscussError::ServerBindError { addr: requested,
source }` and returns `Ok(bound)` unchanged. `std::io` is already imported at `:17`, so
no new dependency is introduced. The `unwrap_or(addr)` fallback is removed and must not
reappear (Decision 2). The signature of `serve_with_ready`, the `R: FnOnce(SocketAddr)`
bound, `ensure_loopback`, and `bind_error` are unchanged. The `axum::serve` error
mapping at `:104` changes `addr` to `listening_addr`, so a serve failure never reports
port 0.

`src/server/mod.rs` has no `#[cfg(test)] mod tests` today, so c2 adds one at the end of
the file holding the two helper tests named in the Test Plan.

### `src/lib.rs` — auto-allocation (c2)

Delete `pub const DEFAULT_PORT: u16 = 7777;` (`:48`), and replace `:211-212` with one
statement binding `SocketAddr::from((Ipv4Addr::LOCALHOST, config.port.unwrap_or(0)))`.
`None` now means "OS chooses"; `Some(n)` is guaranteed nonzero by c1 plus clap. The
`serve_with_ready` call site and the ready-callback body are otherwise untouched —
`url` still comes from `launch::loopback_url(listening_addr)`, now carrying the real
port.

### Documentation and changelog (c3)

| Location | Change |
| --- | --- |
| `AGENTS.md:72` | Rewrite the runtime-launch invariant: no explicit port binds `127.0.0.1:0` and reports the OS-assigned port from a checked `local_addr()`; an explicit port binds exactly and fails fast with no fallback; explicit `0` from `DISCUSS_PORT` or TOML is a `ConfigParseError`. Delete the "do not add a free-port fallback" clause. |
| `README.md:66`, `:74` | Replace `http://127.0.0.1:7777` with the automatically chosen free loopback port printed at startup. |
| `README.md:156` | `--port <N>` Default cell becomes the OS-assigned free port; description states exact binding, fail-fast on collision, and rejection of `0`. |
| `llms.txt:24` | State the port is OS-assigned unless `--port` is given. `:25` stays byte-identical (Design Decision 5). |
| `llms.txt:38` | "Effective default: 7777" becomes "omit to let the OS assign a free port; `0` is rejected". |
| `llms.txt:56`, `:72` | Replace `port = 7777` with a non-7777 example plus an "optional; omit for an OS-assigned port" note. |
| `CHANGELOG.md:7` | Under the empty `## [Unreleased]`, add `### Changed` and `### Removed`. Criterion a8 pins the required literals: the section must contain `--port 7777` as the exact-restore path, `port = 0` for the TOML rejection, `DISCUSS_PORT=0` for the env rejection, `exit code 2`, and `discuss::DEFAULT_PORT` under a `### Removed` heading. |

After c3 the literal `7777` must not appear in `AGENTS.md`, `README.md`, or `llms.txt`
(criterion a7); the restore path lives in `CHANGELOG.md` only, per the master plan's
Rollback section. `AGENTS.md:73`, `AGENTS.md:75`, and `skills/discuss/**` are untouched
(CH-003, CH-002, CH-004).
## Planned Commits

Each row's command compiles and executes everything that row introduces: the targets
holding its new tests, plus the shell checks that are the only evidence for its
non-Rust changes. No command contains a literal `|`, so every cell parses as four
columns and runs verbatim.

| Slot | Scope | Files | Verification |
| --- | --- | --- | --- |
| c1 | Reject explicit port `0` at the env and TOML layers, and cover the omitted-`port` path | `src/config.rs` (`PORT_ZERO_MESSAGE`, `deserialize_port`, `parse_env_port`, `Config::port`, `ConfigLayer::port`, `from_env`) and its `mod tests`; `tests/cli.rs` (`cli_zero_env_port_exits_two`) | `cargo test --lib config::tests && cargo test --test cli cli_zero_env_port_exits_two` |
| c2 | Bind `127.0.0.1:0` when no port is configured, delete `DEFAULT_PORT`, and map a checked `local_addr()` through a tested helper | `src/lib.rs` (`DEFAULT_PORT`, `run_review_session`), `src/server/mod.rs` (`serve_with_ready`, new `local_addr_or_bind_error`, new `mod tests`), `tests/cli.rs`, `tests/server.rs` | `cargo test --lib --test cli --test server && sh -c 'if grep -rn DEFAULT_PORT src/; then exit 1; fi; if grep -n unwrap_or src/server/mod.rs; then exit 1; fi'` |
| c3 | Correct every invariant, doc, and changelog sentence describing the old fixed-port contract | `AGENTS.md`, `README.md`, `llms.txt`, `CHANGELOG.md` | `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo build --all-targets && cargo test && sh -c 'if grep -n 7777 AGENTS.md README.md llms.txt; then exit 1; fi' && sh -c 'sed -n "/^## \[Unreleased\]/,/^## \[0\.7\.0\]/p" CHANGELOG.md > /tmp/gh33-unreleased.txt; for s in "--port 7777" "port = 0" "DISCUSS_PORT=0" "exit code 2" "### Removed" "discuss::DEFAULT_PORT"; do if ! grep -qF -- "$s" /tmp/gh33-unreleased.txt; then echo "missing: $s"; exit 1; fi; done'` |

`cli_zero_env_port_exits_two` stays in c1 rather than moving to c2, because it is the
process-level half of c1's own outcome: after c1 alone, `DISCUSS_PORT=0` already
produces `ConfigParseError` and exits 2 through `exit_code_for_error`, without ever
reaching a bind. c1's command therefore gains `cargo test --test cli
cli_zero_env_port_exits_two`, which compiles and runs the integration target it adds.
c2's `--lib` covers the new `src/server/mod.rs` test module, `--test cli` and
`--test server` cover both integration targets it touches, and the appended shell
check is the evidence for the deletions c2 performs (criterion a10). c3 adds no Rust
tests, so the shell checks appended to its command (criteria a7 and a8) are the only
evidence for the documentation and changelog edits it introduces.
## Production Deploy Safety

No migration, schema, or persisted-state change; the history archive format
(`src/history.rs`) is untouched. Rollback is reverting the seam's commits. Deploy is
release-note framing only: the c3 CHANGELOG entry announces the default-port break
and the explicit-zero rejection, with `--port 7777` as the exact-restore path. Per
**Branching Strategy** this seam stacks on the GH-33 base PR, because CH-007, CH-002,
CH-003, and CH-004 each depend on it; the seam PR is a draft marked "benchmark
re-run" and issue #33 is not closed.
## Test Plan

### Tests to add

`src/config.rs` `mod tests` (c1):

- `rejects_zero_port_from_toml` — `Config::from_toml_str("port = 0\n", "/tmp/discuss.config.toml")`
  returns `ConfigParseError` with that `path`, `line == 1`, `col > 0`, and a message
  containing `PORT_ZERO_MESSAGE`.
- `rejects_zero_port_from_config_file_layer` — `resolve_with_sources` over a temp
  project TOML containing `port = 0` returns `ConfigParseError` whose `path` is that
  temp file and whose `line`/`col` point at the value.
- `rejects_zero_port_from_env` — `resolve_with_sources` with `DISCUSS_PORT=0` returns
  `ConfigParseError` with `path == "DISCUSS_PORT"` and a message containing
  `PORT_ZERO_MESSAGE`.
- `config_layer_toml_without_port_preserves_lower_layer_port` — `resolve_with_sources`
  with a user TOML setting `port = 1111` and a project TOML setting only
  `no_save = true` resolves to `port == Some(1111)` and `no_save == true`. This is the
  regression for the `deserialize_with` missing-field hazard: it is the only test that
  parses a `ConfigLayer` document with no `port` key.
- `config_toml_without_port_defaults_to_none` — `Config::from_toml_str` on a document
  containing only `auto_open = false` yields `port == None`, covering the same hazard
  on the public entry point.

`src/server/mod.rs` — new `#[cfg(test)] mod tests` (c2):

- `local_addr_failure_maps_to_server_bind_error` — `local_addr_or_bind_error` given a
  requested `127.0.0.1:0` and `Err(io::Error::new(io::ErrorKind::Other, …))` returns
  `DiscussError::ServerBindError` whose `addr` is the requested address and whose
  `source` kind is `Other`. This is the executed discharge of Decision 2's
  hard-startup-error requirement.
- `local_addr_success_returns_bound_address` — the same helper given `Ok(127.0.0.1:N)`
  for a nonzero `N` returns that bound address, not the requested `:0`. This is the
  regression against reintroducing an `unwrap_or(addr)` fallback.

`tests/cli.rs` (`cli_zero_env_port_exits_two` in c1, the rest in c2):

- `cli_zero_env_port_exits_two` — spawning the binary with `DISCUSS_PORT=0` exits `2`,
  leaves stdout empty, and names `DISCUSS_PORT` on stderr.
- `cli_auto_allocates_distinct_ports_for_concurrent_sessions` — two children spawned
  with `--no-open`, no `--port`, `env_remove("DISCUSS_PORT")`, and separate temp
  `HOME`/`current_dir`; each first stdout line parses as `session.started`; both
  `payload.url` ports are nonzero and differ; `GET /api/state` on each returns
  `200 OK`. Uses `read_first_line` and `recv_timeout`, never a sleep.
- New helper `fn get_state(port: u16) -> String`, modelled on `post_done` (`:330`),
  issuing `GET /api/state` over a raw `TcpStream`.

`tests/server.rs` (c2):

- `serve_with_ready_reports_os_allocated_port_for_zero` — `serve_with_ready` on
  `127.0.0.1:0` invokes the ready callback with a loopback address whose port is
  nonzero, and that address accepts a connection.

### Existing coverage relied on, not modified

Claims here were checked against the test bodies at HEAD, not against test names.

- `cli_busy_port_exits_three_and_reports_port` (`tests/cli.rs:15`) passes
  `--port <busy>` and asserts exit `3` plus `port {busy_port}` on stderr, so it proves
  exact binding and the `PortInUse` mapping.
- `cli_emits_single_session_started_event_after_listening` (`:111`) asserts
  `payload.url == http://127.0.0.1:{port}` for an explicit `--port`;
  `cli_no_open_logs_listening_url_to_stderr` (`:54`) asserts the exact stderr line for
  the same. Both bind the port they were given verbatim.
- Every binary-spawning test in `tests/cli.rs` that reaches a bind passes `--port`
  (`:29`, `:64`, `:125`, `:182`, `:234`). The two that omit it exit before
  `run_review_session`: `:87` runs the `update` subcommand, and `:272` fails verdict
  parsing at `src/lib.rs:85`. Auto-allocation therefore disturbs none of them.
- `src/exit.rs:11-13` maps `ConfigParseError` to `EXIT_CONFIG_ERROR` (2), and
  `rejects_zero_port_override` (`src/cli.rs:360`) covers `--port 0`.

No existing test parses a TOML document that omits `port`, and none exercises a
`local_addr()` failure; both gaps are filled by the new tests above rather than
claimed as existing coverage.

### Acceptance criteria and their checks

| # | Criterion | Check |
| --- | --- | --- |
| a1 | Two concurrent no-flag sessions start on distinct nonzero ports and both serve `/api/state` | `cargo test --test cli cli_auto_allocates_distinct_ports_for_concurrent_sessions` |
| a2 | `serve_with_ready` reports a real nonzero port for a requested `:0` | `cargo test --test server serve_with_ready_reports_os_allocated_port_for_zero` |
| a3 | Explicit port binds exactly and collides with `PortInUse`/exit 3 | `cargo test --test cli cli_busy_port_exits_three_and_reports_port` |
| a4 | Explicit `0` is a config error per layer, and an omitted `port` still defaults cleanly | `cargo test --lib config::tests` |
| a5 | Explicit `0` from env exits 2 at process level | `cargo test --test cli cli_zero_env_port_exits_two` |
| a6 | A `local_addr()` failure maps to `ServerBindError` at the requested address, and success returns the bound address | `cargo test --lib server::tests::local_addr` |
| a7 | `AGENTS.md`, `README.md`, and `llms.txt` no longer mention `7777` | `sh -c 'if grep -n 7777 AGENTS.md README.md llms.txt; then exit 1; fi'` |
| a8 | The CHANGELOG `## [Unreleased]` section states the restore path, both zero rejections, the exit code, and the removed constant | `sh -c 'sed -n "/^## \[Unreleased\]/,/^## \[0\.7\.0\]/p" CHANGELOG.md > /tmp/gh33-unreleased.txt; for s in "--port 7777" "port = 0" "DISCUSS_PORT=0" "exit code 2" "### Removed" "discuss::DEFAULT_PORT"; do if ! grep -qF -- "$s" /tmp/gh33-unreleased.txt; then echo "missing: $s"; exit 1; fi; done'` |
| a9 | Full CI chain green | `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo build --all-targets && cargo test` |
| a10 | Supporting: no fixed default and no `unwrap_or` fallback survive in production code | `sh -c 'if grep -rn DEFAULT_PORT src/; then exit 1; fi; if grep -n unwrap_or src/server/mod.rs; then exit 1; fi'` |

No criterion command contains a literal `|`, so every cell parses as three columns and
runs verbatim. a6 is the discharge of Decision 2's hard-startup-error requirement;
a10 is supporting evidence only. a10 greps for any `unwrap_or` in `src/server/mod.rs` rather than the
exact old expression, because line 86 is that file's only current `unwrap_or` and a
bare-token search cannot be evaded by reformatting. Both a8 and a10 were dry-run
against HEAD and fail there, confirming they discriminate rather than pass vacuously.
The side-effect-free failure path is argued structurally in Design Decision 3 from
`src/server/mod.rs:83-89`; it is not claimed as executed behavior, and proving it at
the orchestration level stays with CH-007.
## Identified Risks

| Scenario | Consequence if wrong | Decision level | Status |
|----------|----------------------|----------------|--------|
| A user or script depends on the bare-`discuss` URL being `http://127.0.0.1:7777` | Their workflow breaks with a connection refusal | Agent must confirm with user | Addressed — the Tier 3 human gate covers it; CHANGELOG names `--port 7777` as the exact restore (a8) |
| Someone sets `DISCUSS_PORT=0` or TOML `port = 0` today and gets auto-allocation | Their session now exits 2 instead of starting | Agent must confirm with user | Addressed — required by FR-2 and Decision 3; same CHANGELOG entry (a8) |
| `deserialize_with` on `Option<u16>` loses serde's implicit missing-field default | Every config file without a `port` key fails to parse, or silently resets a lower-priority port | Agent may decide | Addressed — field-level `#[serde(default)]` on both fields. No existing test parses a port-less TOML, so `config_layer_toml_without_port_preserves_lower_layer_port` and `config_toml_without_port_defaults_to_none` are added to cover it (a4) |
| A `local_addr()` failure silently degrades instead of failing startup | A session reports port 0 or a stale address, the exact defect Decision 2 forbids | Agent must check instructions | Addressed — `local_addr_or_bind_error` is unit-tested for both arms (a6), with a10 as structural backstop |
| The custom deserializer error carries no span, so a config-file zero reports line 0 | Breaks the `AGENTS.md` path-and-position rule for file errors | Agent must check instructions | Addressed — span attachment confirmed in `toml_edit-0.22.27/src/de/table.rs:162-178`; asserted by `rejects_zero_port_from_toml` (a4) |
| The concurrency test is flaky under a loaded CI runner | Intermittent red CI, eroded trust in the suite | Agent may decide | Addressed — Design Decision 4: event-driven readiness, no sleeps, no port-value assertions |
| A developer's ambient `DISCUSS_PORT` leaks into the new no-flag tests | Local-only failures that CI cannot reproduce | Agent may decide | Addressed — `env_remove("DISCUSS_PORT")` plus temp `current_dir` and `HOME` |
| Removing `discuss::DEFAULT_PORT` breaks an out-of-tree library consumer | Downstream compile error | Agent may decide | Addressed — Design Decision 1: `0.x` crate, binary distribution, announced under CHANGELOG `### Removed` (a8) |
## Dependencies

None beyond plan ordering. CH-001 is the first outcome of the master plan and
depends on no prior seam.
