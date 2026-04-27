# Changelog

All notable changes to `discuss` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

## [0.3.0] - 2026-04-27

### Added
- Read markdown from stdin: `discuss -` reads stdin explicitly, and bare `discuss` with a piped (non-TTY) stdin auto-detects and reads stdin too. Bare `discuss` in an interactive terminal still prints help (on stderr) and exits with code 2 — preserving the contract from clap's previous `arg_required_else_help`. In stdin mode the `session.started` event reports `source_file: "<stdin>"` and history archives fall back to `.../unnamed/<timestamp>.json`. Lets agents pipe generated markdown (e.g. a summary of `git diff --cached`) straight into a review without writing a temp file. `/discuss` skill updated with stdin Monitor examples.
- `Cargo.toml` declares `rust-version = "1.88"` so the codebase fails with an actionable MSRV error on older toolchains.

### Changed
- `Cargo.toml` upgraded from `edition = "2021"` to `edition = "2024"`. `cargo fix --edition` applied; `unsafe { env::set_var(...) }` blocks added in test-only env helpers (with SAFETY comments referencing the existing `env_mutex()` serialization), and one `if let` chain in `src/launch.rs` switched to `let_chains` syntax. No public-API or runtime-behavior changes.
- Renamed `src/state/mod.rs` to `src/state.rs` so `state` follows the sibling-file module convention used elsewhere in the crate. No code changes; module path is unchanged.

### Fixed
- `/discuss` skill used a `Bash run_in_background` + "call Monitor on the task ID" pattern that does not match Monitor's actual API (Monitor runs its own command; it does not accept a task ID). Claude Code CLI improvised around the mismatch, but Claude Code App did not — events never streamed and the session appeared to hang after the browser launched. Step 1 now launches `discuss` via `Monitor(command, persistent: true)` directly; Step 4 stops via `TaskStop(task_id)`. `TaskStop` added to `allowed-tools`.

### Known limitations
- On Windows running under MSYS2 / mintty / Git Bash, `std::io::IsTerminal::is_terminal()` returns `false` at an interactive prompt (those shells use a named-pipe pseudo-tty rather than the conhost console). Bare `discuss` will fall into the stdin auto-detect arm and block on `read_to_string` instead of printing help. Workaround: use `discuss -` (explicit stdin), `discuss file.md` (file path), or `discuss --help` on those terminals. Tracked in [#5](https://github.com/codesoda/discuss-cli/issues/5); POSIX terminals (Linux, macOS, Windows conhost) work correctly.

## [0.2.0] - 2026-04-24

### Added
- `/discuss` skill at `skills/discuss/SKILL.md` for Claude Code, Codex, and other agents honoring `~/.agents/skills/`. Launches `discuss <file>`, streams stdout events via Monitor, and posts "takes" in response to user-opened threads.
- `install.sh` symlinks `skills/discuss/` into `~/.claude/skills/`, `~/.codex/skills/`, and `~/.agents/skills/` when run from a clone.
- Skill self-bootstraps the binary on first use: detects missing `discuss`, prompts the user, runs `curl | sh` the installer, and falls back to `~/.discuss/bin/discuss` if PATH is stale.
- Skill is also installable via `npx skills add codesoda/discuss-cli` (vercel-labs/skills), with the binary bootstrapping lazily on first invocation.
- `README.md` with install paths, agent-first quick start, and API reference.

### Changed
- **Breaking for stdout consumers:** `take.added`, `draft.updated`, and `draft.cleared` events no longer emit to stdout. These kinds remain on the SSE channel for the browser UI. `EventKind::ALL` shrinks from 11 to 8 variants.
- Repository metadata points at `codesoda/discuss-cli` (was `chrisraethke/discuss-cli`).
- `CLAUDE.md` consolidated to a single-line `@AGENTS.md` include; Rust Patterns content moved into `AGENTS.md` so Claude Code and Codex read the same source of truth.

### Removed
- `tasks/prd-discuss-cli.md` (gitignored; the PRD is no longer tracked).

## [0.1.0] - 2026-04-23

### Added
- Canonical first-release smoke test: push the `v0.1.0` tag to trigger `.github/workflows/release.yml`, publish `discuss-v0.1.0-aarch64-apple-darwin.tar.gz`, and attach `checksums-sha256.txt`.
