# Changelog

All notable changes to `discuss` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **Finish-review verdict options** — `--verdict-options <SPEC>` and `--verdict-prompt <TEXT>` let sessions collect an overall review decision at Done time. The DSL is `id[:label][:style][!]` separated by shell-quoted `|` (for example `approved:Approve|declined:Decline:negative!`), with 2+ unique ids/labels, `positive` / `neutral` / `negative` cosmetic styles, and `!` marking feedback-required choices; invalid specs fail fast at startup with exit 2. The browser button is now `#finish-review` (renamed from `#copy-all`): without verdict options behavior is unchanged, while configured sessions open a modal with prompt, always-visible feedback, direct buttons for up to 3 options, or a dropdown plus submit for 4+ options. `/api/state` exposes `verdictConfig`; `/api/done` requires a verdict body only when configured and rejects missing bodies with 400 `bad_request` or unknown options / missing required feedback with 400 `validation_error`, where every rejected verdict is a total no-op (no `session.done`, no history write, no shutdown). Accepted `session.done` payloads and archives gain optional `verdict { optionId, label, feedback?, decidedAt }`.

## [0.5.0] - 2026-07-07

### Added

- **Multi-file review sessions** — `discuss a.md b.md c.md` opens one session over several files. A left sidebar (hidden for single files) lists each file with a kind tag and an open-thread badge; clicking swaps the pre-rendered document in place using the same re-anchor machinery as live source updates. The schema is files-aware: `Source { files }` with per-file ids (`f-1`, `f-2`, … in CLI order), `Thread.file_id` (legacy payloads default to `f-1`), file-scoped new-thread draft keys (`<fileId>|<start>-<end>`, legacy `<start>-<end>` still deserializes), and a `files` array in `/api/state` and the transcript. `fileId` is required on `POST /api/threads`, drafts, and `POST /api/source` when several files are loaded (400 `missing_file_id` / 404 `unknown_file` otherwise); `/api/source` coverage validation is scoped to the updated file and `source.updated` carries `fileId`. Duplicate CLI paths fail loudly with exit 2; `-` (stdin) may appear once anywhere in the list; `.diff`/`.patch` files load as diff-kind sections. Done transcripts group threads by file in CLI order and multi-file history archives land under `multi-<N>-files/`. Closes multi-file portion of [#7](https://github.com/codesoda/discuss-cli/issues/7)'s plan.
- **`discuss diff` — first-class git diff review** — reviews a unified git diff directly, no markdown-wrapper prompt needed. Default is the staged diff (`git diff --cached`); `--unstaged` reviews the working tree; trailing args (`HEAD~3..HEAD`, `main...feature`, paths) are forwarded to `git diff`. Combines with the inline file list: `discuss plan.md notes.md diff HEAD~1..HEAD` reviews the documents and the diff in one session. Each changed file becomes its own sidebar entry rendered through the normal markdown pipeline as a heading (path, `+N −M`, new/deleted/rename/binary notes) plus one fenced `diff-<lang>` block per hunk — Prism's diff-highlight plus the autoloaded grammar give language-aware highlighting, and line-anchored threads land directly on diff lines. `session.started` gains `mode` (`markdown` / `diff` / `mixed`), `files_count`, and `git_args`. Diff output is capped at 5 MB with `DiffError` guidance; override via `--max-diff-bytes <N>` (CLI) > `DISCUSS_MAX_DIFF_BYTES` (env) > `max_diff_bytes` (config); `0` disables. Empty diffs exit with `diff error: no changes to review`. Closes [#7](https://github.com/codesoda/discuss-cli/issues/7).

- Live source updates via `POST /api/source` — an agent that regenerated the markdown mid-session pushes the entire new source plus a re-anchor decision for every active thread (new `anchorStart`/`anchorEnd`/`snippet`/`lineRange`, or `orphaned: true`) in one atomic call. Coverage is strict in both directions: a request missing an active thread, or naming an unknown/deleted one, is rejected with 400 and nothing changes. On success the server swaps the source under the state lock, bumps a `sourceVersion` counter (exposed in `/api/state` and the initial page state), and broadcasts `source.updated { markdown, renderedHtml, threadAnchors, orphanedThreadIds, sourceVersion }` over SSE and stdout. The browser swaps `#doc-content` in place, re-runs anchor indexing / Prism / mermaid / heading minimap, updates thread anchors, and drops any in-flight new-thread selection — conversations survive the edit. Orphaned threads keep their conversation, render with a dashed marker-less "orphaned" badge, and stack below anchored threads. `POST /api/threads` accepts an optional `sourceVersion` for optimistic concurrency: a stale version returns `409 stale_source_version` instead of anchoring a comment onto content that changed underneath it (the browser sends it automatically). Closes [#3](https://github.com/codesoda/discuss-cli/issues/3).

- YAML frontmatter (the leading `---` ... `---` block at the top of a markdown file) renders as a `<details class="front-matter">` containing a `<pre><code class="language-yaml">` of the raw fields, collapsed by default, with a "Front Matter" summary. Body markdown renders unchanged below. The block is intentionally threadable — the existing pre-wrap pass in `discuss.html` treats it like any other code block, so reviewers can comment on a wrong title or missing tag; collapsing hides inner markers via normal `<details>` semantics, re-expanding restores them. Detection and HTML emission live in `src/render.rs` (six new unit tests); CSS in `discuss.html` reuses the existing `--line`, `--bg`, `--muted` custom properties so light and dark themes follow automatically. Closes [#14](https://github.com/codesoda/discuss-cli/issues/14).
- H1 and H2 heading sections are collapsible — each heading wraps its content in a `<details open>` element with nested H2-inside-H1 structure. An inline chevron toggle appears on hover, shifting the heading text right; clicking the heading itself still opens the thread editor. When a section is collapsed, thread markers from hidden anchors aggregate onto the summary as a muted dashed-outline marker stack (capped at 5 visible, `+N` overflow badge). Clicking an aggregated marker expands the section and opens the thread. The aggregation pattern generalizes to any `<details>` wrapper including the frontmatter block. Closes [#13](https://github.com/codesoda/discuss-cli/issues/13).
- Polling fallback for agent integrations without a Monitor-type tool: `skills/discuss/poller.sh` blocks on `/api/state`, emits newline-delimited JSON events (`thread.created`, `thread.updated`, `session.done`) plus a `snapshot` baseline line so no concurrent events are dropped between invocations. `skills/discuss/SKILL.md` documents the fallback as Option B, gated on Monitor being unavailable.

### Changed
- The **Finish review** button replaces "Done — send to chat" (completed state shows "Finished ✓"); the transcript hand-off behavior is unchanged.
- Bundled `assets/mermaid.min.js` upgraded from a v8-era build to mermaid v11.14.0 (UMD/iife) so modern flowchart syntax (`subgraph X["label"]`, cylinder shapes, `<br/>` in node labels, unicode arrows) renders correctly. The hydration shim was rewritten to use the v10/v11 promise-based `mermaid.render(id, source)` API with `securityLevel: 'loose'`, surfaces parse failures inline as a `.mermaid-error` note instead of failing silently, and now marks `<pre>` blocks with `mermaid-block`/`no-line-numbers` *before* Prism runs. `highlightCodeBlocks()` skips those blocks (no `line-numbers` class, no `Prism.highlightElement`) so mermaid sources are no longer tokenized by Prism before the SVG render lands. Asset size budget bumped from 700 KB to 4 MB to fit v11.

### Fixed
- Bare `discuss` no longer hangs at an interactive prompt in MSYS2 / mintty / Git Bash on Windows. Stdin terminal detection now uses the [`is-terminal`](https://crates.io/crates/is-terminal) crate, which recognizes MSYS pseudo-tty named pipes (`\\msys-*-pty*`) as terminals, instead of `std::io::IsTerminal` which reports them as pipes and sent bare `discuss` into the stdin auto-detect arm where it blocked waiting for EOF. POSIX terminals (Linux, macOS, Windows conhost) are unchanged. The same detection now also gates the `discuss update` interactive-confirm path. Closes [#5](https://github.com/codesoda/discuss-cli/issues/5).

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
