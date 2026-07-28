# Changelog

All notable changes to `discuss` are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]

### Added

- **Concurrent sessions and self-describing startup** — no-override sessions now bind `127.0.0.1:0` directly and use the OS-assigned port, while explicit nonzero CLI, environment, and TOML ports bind exactly and fail on collision without fallback. Startup acquires all required loopback listeners before readiness and releases partial acquisitions on failure. `session.started` preserves its existing fields and adds `apiBaseUrl`, an exact endpoint map, and agent instructions; ordinary sessions omit the optional `proxyUrl`. Stderr now reports `review UI/API: <actual-url>`, and bundled integrations consume reported endpoints instead of scanning ports or assuming `7777`. Closes [#33](https://github.com/codesoda/discuss-cli/issues/33).
- **Table rows are commentable** — `<tr>` joins the commentable selector, so clicking a row (header rows included) anchors a thread to it, and drag-selecting several rows produces one range comment via the existing multi-element mechanism. Cells are deliberately not commentable: anchors bind to the outermost match, so rows and cells are mutually exclusive. Rows sit inside a horizontal scroll container, so marker stacks are hosted on the unclipped `.table-wrap` and offset to the row, tagged with `data-for-anchor` because one wrap holds a stack per commented row; a `ResizeObserver` re-offsets them when a resize rewraps a cell. Row snippets join cell text with `·` so the thread card and transcript stay readable. Row state highlights are left to the generic `[data-anchor-idx]` rules rather than restated, so they track the shared palette; only the outline is suppressed (an offset outline on a `<tr>` draws outside the table) and `thead th` takes `background-color: inherit` so its opaque header fill doesn't hide the row state.

### Changed

- **Tables render compact instead of falling back to browser defaults** — `discuss.html` previously had no table CSS at all. Tables now get `border-collapse`, 13px text, tight padding, hairline row separators, and a header band, all from existing theme variables. `max-width: 100%` lets auto layout shrink columns to the pane so a long prose column wraps rather than squeezing its neighbours; sideways scrolling only engages once even the minimum widths don't fit, inside a two-layer `.table-wrap` / `.table-scroll` wrapper that keeps gutter markers out of the overflow clip. A right-edge fade appears only while there is more table to scroll to.
- **Column headers are left-aligned** — `<th>` no longer inherits the browser's centred default. Scoped as `th:not([align])` so explicit markdown alignment (`|---:|`, which comrak emits as `align="right"`) is preserved.

### Fixed

- **Links are readable in dark mode** — `#doc-content` had no link styling at all, so links fell back to the UA blue (`#0000EE`, 2.3:1 on the dark page) and the UA visited purple, both effectively unreadable. Links now use a new `--link` token, with `:visited` matching so the second colour can't reintroduce the problem. `--link` is deliberately not `--accent`: on the dark page `--accent` measures 5.5:1, which passes AA but sits far below body text at 12.5:1, so links read as the dimmest thing on the page despite being the one thing you want to click. The dark value is lifted to `#7cb0ff` (6.9:1); the light value is `#0b5fff` (5.13:1), unchanged from what `--accent` already gave, so light mode looks the same.

## [0.7.0] - 2026-08-24

### Added

- **Thread summary navigation** — the resolved-thread count in the header now opens a popover listing every thread in document order with its status and a short preview. Selecting an entry switches files when needed and opens the matching thread; click-outside and Escape dismiss the popover.
- **Cross-platform releases and installers** — releases now publish native artifacts for Apple Silicon and Intel macOS, x86_64 Linux, and x86_64 Windows, with CI running on both Ubuntu and Windows. A checksum-verifying PowerShell installer installs `discuss.exe`, the self-updater supports Windows zip artifacts, and Windows-safe history filenames plus portable home-directory handling keep runtime behavior consistent across platforms.
- **HTML prototype review with element anchors** — `.html`/`.htm` inputs render in a sandboxed same-origin iframe with an Inspect toggle, hover outline, numbered markers, selector fallback/fuzzy reattachment, and the existing thread/reply/take lifecycle. Relative prototype assets are served through traversal-guarded per-file routes; served HTML receives a local `<base>` and inspector script while CSP meta tags are neutralized. HTML `thread.created` events and transcripts include `elementAnchor { selector, fallbacks, tag, textDigest?, outerHtml }`; `/api/anchors/resolve` persists detached status without emitting agent-facing stdout noise. Mixed-file switching keeps loaded prototype iframes alive. Root-absolute assets, closed shadow-root internals, and live file watching remain out of scope.
- **Image review with pin anchors** — PNG, JPEG, GIF, WebP, and SVG paths can be reviewed alone or alongside markdown/diff files. Images are read once as bytes at startup, rendered through an `<img>` backed by `GET /api/files/{fileId}/raw`, and annotated with numbered percentage-positioned pins. Image threads carry `imageAnchor: {xPct, yPct}` basis-point coordinates while retaining their pin number in `anchorStart`/`anchorEnd`; events and transcripts include the coordinates and a `pin N at X%,Y%` breadcrumb. The browser supports optimistic pin creation, reload-stable saved pins, file switching, focusing from the pin layer, and the existing reply/take/resolve/delete lifecycle. Live `/api/source` updates and server-persisted drafts for unsent pin comments remain out of scope.

### Changed

- HTML prototype reviews now start in Inspect mode, preserve an open comment editor while anchors synchronize, and retain the in-frame highlight for the currently open thread.
- The bundled `/discuss` skill now requires a monitor-type background tool when available, documents both Claude Code and pi launch/stop primitives, keeps launch commands approval-friendly by starting directly with `discuss`, and explicitly leaves browser auto-open enabled.

### Fixed

- Review-shell responses now prevent stale HTML prototype UI from being reused by browser caches.

### Removed

- Removed the redundant one-line `CLAUDE.md` include; agents read `AGENTS.md` directly as the single source of repository instructions.

## [0.6.1] - 2026-08-03

### Changed

- `src/server.rs` (2050 lines) split into a `src/server/` module tree — `mod.rs` (routing, `serve`/`serve_with_ready`, idle timer, shutdown middleware, `resolve_file_id`, bind helpers), `app_state.rs` (`AppState`, `ActivityTracker`, `ShutdownSignal`), `threads.rs`, `drafts.rs`, `source.rs`, `done.rs`, `pages.rs`, and `response.rs` (shared error/asset response helpers). Internal items moved to `pub(super)` visibility; the public surface (`AppState`, `serve`, `serve_with_ready`) and all HTTP behavior are unchanged.

### Fixed

- The mermaid hydration shim no longer assigns mermaid's rendered markup via `innerHTML`. `assets/mermaid-shim.js` now parses the SVG in an inert document with `DOMParser` (`image/svg+xml`, falling back to `text/html`), strips `<script>` elements and `on*` / `javascript:` attributes, imports only the resulting `<svg>` element, and inserts it with `replaceChildren`. Markup that fails to parse surfaces through the existing inline `.mermaid-error` note instead of being injected. Rendering behavior for valid diagrams is unchanged.

- **Text on accent-filled controls in dark mode** — `Save`, the primary buttons in both comment editors, `#finish-review`, `header .toggle.on` and `#selection-popup:hover` fill with `--accent` and set `color: white`. That is 5.1:1 in light mode, but the dark palette lightens `--accent` from `#0b5fff` to `#5a9bff` so it reads against a dark page, which drops white text on it to **2.8:1**. Text on an accent fill now uses a new `--accent-ink` token (`#ffffff` light, `#0a1a33` dark) and measures 6.3:1. `#finish-review.ok` restates `color: white` because its success state swaps to a fixed `#2e7d32` green that is dark in both themes (5.1:1). Light mode is unchanged. The `tests/theme.rs` allowlist rationale was corrected at the same time: "sits on a saturated fill" is not grounds for hardcoding white — only a fill that is itself identical in both palettes is.

- **Dark mode contrast** — the dark theme re-defined the `:root` palette under `html[data-theme="dark"]`, but roughly 25 rules in `discuss.html` bypassed the tokens with literal light-mode colours, so they kept rendering dark-on-dark once the theme flipped. Worst offenders were `#doc-content h3` / `h4` (`#333` / `#444`, measuring 1.4:1 and 1.8:1 against `--bg`) and `#doc-content blockquote` (`#333` at 1.3:1) — body text that was effectively invisible. Also fixed: the resolve/resolved greens (`#2e7d32` on `--decision`, 2.6:1), `.verdict-validation` red on `--card` (2.7:1), the open-thread anchor highlight (`rgba(255,236,139,0.55)` washed out to a pale olive that `--ink` could not sit on, 2.7:1), and a set of light islands in an otherwise dark UI — `.followup` and `.new-thread-editor` textareas and buttons (`background: white`), the `.new-thread-editor` panel (`#fff4f7`), `.mutation-error`, `#doc-content .mermaid-error`, and the `.done-banner`. The fix adds the missing semantic tokens to both palettes (`--ink-soft`, `--ink-softer`, `--field`, `--field-hover`, `--ok-ink`, `--ok-bg-hover`, `--danger`, `--danger-ink`, `--danger-bg`, `--danger-bg-hover`, `--danger-border`, `--teal`, `--grip`, `--highlight-active`, `--accent-dashed-strong`) and points those rules at them, rather than stacking per-rule dark overrides — so a rule added later inherits the right colour instead of reintroducing the bug. Every fixed surface now clears WCAG AA (4.5:1); measured in-page, the three from the original report went 1.4→10.5:1, 1.8→9.1:1, and 1.3→9.0:1. Light mode is unchanged: each new token's light value is the literal it replaced, verified by resolving the light palette and diffing against the previous stylesheet. The `.done-banner` keeps a scoped `html[data-theme="dark"]` override instead of four global tokens, since its greens are used by that one component.

## [0.6.0] - 2026-07-09

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
