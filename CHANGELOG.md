# Changelog

All notable changes to `discuss` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added
- New-thread draft keys now include `fileId`. Previously a draft on file `a.md` line 5 collided with a draft on `b.md` line 5; the key encoding became `<fileId>|<start>-<end>` (e.g. `f-2|3-5`) and the upsert/clear request DTOs and `draft.updated`/`draft.cleared` SSE payloads carry `fileId`. Legacy single-file archives that key on `<start>-<end>` deserialize into the default file id (`f-1`) for compatibility.
- Done transcript / history archive groups threads by file. The Done payload now carries a top-level `files: Vec<FileMeta>` array (id / path / kind for every file in the session) and threads sort by `(file_index, anchor_start, anchor_end)` instead of a flat anchor sort, so multi-file sessions and `discuss diff` archives lay out as one block per file. Single-file sessions stay byte-for-byte compatible (the `files` key is omitted when no source is provided).
- v2 diff renderer lazy-highlights hunks via IntersectionObserver: each `pre.v2-diff-hunk` only triggers `Prism.highlightAllUnder` when it scrolls within 200 px of the viewport, with a `data-prism-highlighted` flag preventing double-runs. Keeps first-paint snappy on multi-file diffs that would otherwise fan out the autoloader across N grammars.
- `discuss diff` enforces a 5 MB hard cap on `git diff` output and exits with a clear `DiffError` before opening the browser, suggesting how to narrow the range. Prevents accidental review of huge diffs that would freeze the renderer. The cap is overridable via `--max-diff-bytes <N>` on the subcommand, `max_diff_bytes = N` in `discuss.config.toml`, or the `DISCUSS_MAX_DIFF_BYTES` env var; pass `0` to disable the cap entirely. CLI flag wins over config / env wins over the default.
- `discuss diff` subcommand reviews a unified `git diff` directly. Default scope is the staged diff (`git diff --cached`); pass `--unstaged` for the working tree, or trailing args (e.g. `HEAD~3..HEAD`, `main...feature`) to forward arbitrary ranges to `git diff`. Each changed file lands as a `FileKind::Diff` entry in `Source.files`, and the v2 `<DiffFile>` renderer parses each unified-diff payload into hunks, emitting one `language-diff-<lang>` Prism block per hunk so additions/deletions get language-aware highlighting on top of the diff gutter. Each hunk gets its own `data-anchor-idx` for hunk-granular threading. `session.started` payload gains `mode: "diff"` and `git_args`; an empty diff exits with `DiffError("no changes to review")` before opening the browser.
- Multi-file CLI: `discuss a.md b.md c.md` opens a single review session over multiple files. Ids `f-1` … `f-N` are assigned in argument order; extension-based kind inference promotes `.diff` / `.patch` to `FileKind::Diff`. Duplicate paths and a repeated stdin `-` fail loudly with a structured `DuplicateInputPath` error and exit code 2. `session.started` carries `files_count`, and history archives land under `multi-N-files/` when `files.len() > 1`.
- `?ui=v2` Preact + htm rewrite of the browser shell, opt-in alongside the existing v1 UI. The v2 page loads vendored `preact.umd.js`, `preact-hooks.umd.js`, and `htm.umd.js` from `/assets/*` with the same 24-hour cache headers as the existing mermaid asset, hydrates state from `window.__DISCUSS_INITIAL_STATE__` first and `GET /api/state` as a fallback, and dispatches all SSE event kinds (`thread.created`, `thread.deleted`, `thread.resolved`, `thread.unresolved`, `reply.added`, `take.added`, `draft.updated`, `draft.cleared`) through state setters with optimistic mutations and rollback on REST failure. Brings parity with v1: thread create / reply / resolve / unresolve / delete, debounced draft auto-save (500 ms) for new-thread and follow-up drafts via `/api/drafts/*`, Prism syntax highlighting with diff-highlight, mermaid lazy-load, light/dark/system theme toggle persisted to `localStorage`, and a Done flow that POSTs `/api/done`, tears down heartbeat + SSE, locks the UI, and shows a "review complete" banner.
- File tree UI in v2 for multi-file sessions: a left rail listing each file with kind tag and open / total thread badges (memoized over threads + resolutions). Includes a case-insensitive path filter and an "only with open threads" checkbox; clicking a row sets the active file and smooth-scrolls the content pane to its `<section data-section-file-id>`. Hidden via `data-single-file="true"` when only one file is loaded so single-markdown UX is unchanged.
- Files-aware schema: `Source { files: Vec<File> }` replaces the implicit single-markdown source on `AppState`, `Thread.file_id` defaults to the only file's id in single-file mode and is required in multi-file mode (404 on unknown ids), and `BroadcastEvent` payloads (`thread.created`, `thread.deleted`, `thread.resolved`, `thread.unresolved`, `reply.added`, `take.added`) carry `fileId` so v2 clients can route updates to the right file. `StateSnapshot.files: Vec<FileMeta>` lets the browser learn each file's id / path / kind from `/api/state`.

## [0.4.0] - 2026-04-28

### Added
- Browser-side syntax highlighting for fenced code blocks via [Prism](https://prismjs.com/) loaded from unpkg, including language-aware diffs (e.g. ` ```diff-rust `, ` ```diff-typescript `). The autoloader fetches grammars on demand, so any Prism-supported language works; unknown tags fall back to plain `<pre><code>`. Tag every fence with a language — see `skills/discuss/SKILL.md` for the curated list and Prism's site for the full set.
- Light/dark/system theme toggle in the top bar (inline SVG icons; sun/moon/monitor). Persists to `localStorage` under `discuss-theme`. System mode subscribes to `prefers-color-scheme` and updates live. A pre-paint `<head>` bootstrap script applies the saved mode before first paint, preventing the flash of wrong theme on reload. Dark mode also recolors discuss's own UI via `[data-theme="dark"]` overrides on the existing CSS variables.
- Inline comments on code blocks via an optional `lineRange: { start, end }` field on threads. Selecting text inside a single `<pre>` and creating a thread anchors it to those lines; the gutter shows a thin colored bar (faded green when resolved) on the affected line numbers. Whole-block threads still work via the existing click-without-selection path. Schema added to `src/state/types.rs`, propagated through `POST /api/threads`, `/api/state`, `thread.created` events (stdout + SSE), and the Done/history transcript. Server validates `start >= 1` and `end >= start` — otherwise structured 400 `validation_error`.
- Heading minimap pinned to the left edge of the document — collapsed bars (h1–h4) by default, expand into a feathered translucent panel on hover with click-to-scroll and a tooltip per heading. Bar widths scale proportionally to heading text length so the longest heading anchors to the right edge of the panel. The first heading visible in the viewport (or the most recent one scrolled past) gets an accent-colored highlight, updated on scroll via `requestAnimationFrame`. Hovered bars grow vertically into the surrounding gap (negative margins keep flex layout stable, `border-radius: 999px` caps to a pill shape) without pushing siblings.
- GitHub link in the header bar — sits between the spacer and the "Show all" toggle, opens `https://github.com/codesoda/discuss-cli` in a new tab. Styled to match the existing theme-toggle icon button (32×32, muted color, accent-soft hover tint in both light and dark themes).

### Changed
- `Prism.manual = true` plus a post-hydration `Prism.highlightAllUnder('#doc-content')` call lets the page control highlighting timing rather than auto-running on `DOMContentLoaded`. Prism's `complete` hook re-runs `renderMarkers` so the line-range gutter bars settle once the autoloader finishes any deferred grammar load.
- Removed trailing blank space below short documents: `.workspace-grid` now uses `align-items: start` so panes hug their content instead of stretching to viewport, and a column gradient on the grid (card / divider / bg) preserves the per-column background colors when cells stop short. `reposition()` measures pane-right's vertical padding via `getComputedStyle` and matches `threadsHost.minHeight` to pane-left's content area so neither pane outgrows the other. Pane bottom padding tightened from 180px to 80px.

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
