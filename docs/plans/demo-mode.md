# Demo mode + collapsible file sidebar — implementation plan

Branch: `feat/demo-mode` (from `main` @ `60907e8`). This document is the living plan;
update it whenever a decision below changes during implementation.

## 1. Goal

Two user-facing features, shipped together:

1. **`discuss demo`** — a real CLI subcommand that opens a self-contained,
   mixed multi-file feature gallery (GIF, revised Markdown with pre-seeded agent
   takes, diff, image, self-contained HTML prototype) plus a deterministic
   backend-only "Demo agent" responder so a first-time user can experience the
   whole review loop without an agent session. Every *file* is embedded in the
   binary; the *page* is a normal discuss page and still fetches Prism and the
   version check (scoped in D21 — the demo is agent-free, not network-free).
2. **Collapsible file sidebar** — the multi-file sidebar collapses from 236px to a
   ~50px icon rail with per-kind icons, preserved open-thread badges, accessible
   toggle, and a persisted UI preference (localStorage, UI-pref-only).

## 2. Current-state evidence (verified against the repo)

- **CLI**: `src/cli.rs` — clap derive `Args { port, no_open, no_save, history_dir,
  verdict_options, verdict_prompt, files: Vec<PathBuf>, command: Option<Commands> }`;
  `Commands = Update(UpdateArgs) | Diff(DiffArgs)`. `src/main.rs` is thin; routing
  lives in `src/lib.rs::run_with_shutdown`, which matches `command` and sends
  `Diff`/`None` to `run_review_session`. **Top-level flags are not clap
  `global` args**, so they must precede a subcommand:
  `discuss --port N --no-open demo` parses, `discuss demo --port N` does not —
  same constraint already documented for `--verdict-options` before `diff`.
  `subcommand_precedence_over_arg = true` means a file literally named `demo`
  must be reviewed as `./demo` (existing precedent with `update`/`diff`).
- **Session construction**: `src/lib.rs::run_review_session` builds
  `Source { files }` from `FileInput`s in input order, assigning ids `f-1..f-N`,
  collects image bytes into a `HashMap<FileId, (Vec<u8>, &'static str)>` fed to
  `AppState::with_file_bytes` (sha256 version per file), then calls
  `server::serve_with_ready` with a readiness callback that emits `session.started`
  (with `apiBaseUrl`, `endpoints`, `agentInstructions`, `mode`, `source_file`,
  `files_count`) and announces `review UI/API: <url>` on stderr.
- **First-file selection**: `src/server/pages.rs::render_root_page` renders
  `rendered_files[0]` into `#doc-content`; `discuss.html` sets
  `activeFileId = String(discussFiles[0].id)` after hydration (~line 5790).
  Whatever file is `Source.files[0]` is selected on load — no frontend change
  needed to make the GIF initial.
- **File kinds**: `src/state/types.rs::FileKind` = `Markdown | Diff | Image | Html`,
  serialized lowercase. `src/lib.rs::file_kind_for_path` maps `.gif` →
  `FileKind::Image`; `image_mime_for_path` → `image/gif`. Images are served only
  via `GET /api/files/{id}/raw` (`src/server/files.rs`) with CSP sandbox + cache
  headers; `File.content` stays empty for images.
- **HTML files**: `src/server/files.rs::get_html_file` serves `File.content` from
  memory (works for embedded files); `get_html_asset` canonicalizes the *disk*
  parent of `file.path` **relative to the process CWD**. This is a hazard, not a
  guaranteed 404: a virtual path like `demo/prototype.html` has parent `demo`,
  and if the user's CWD contains a real `demo/` directory (this repo does — the
  GIF-recording tooling), `GET /files/f-6/assets/<name>` would serve real disk
  files from it. Mitigated by D15: demo virtual paths are bare filenames, whose
  `Path::parent()` is `Some("")`, and `"".canonicalize()` fails on every
  platform, so the asset route 404s deterministically regardless of CWD. Demo
  HTML is additionally fully self-contained (no relative asset refs).
- **ID allocation**: `src/server/app_state.rs` — `next_user_thread_id` (`u-N`),
  `next_agent_thread_id` (`a-N`), `next_reply_id` (`r-N`), `next_take_id` (`t-N`)
  are `pub(super)` atomics starting at 1. Seeding must go through these
  allocators (from inside the `server` module) or later runtime allocation would
  collide with hardcoded ids.
- **Agent threads**: `src/server/threads.rs::post_api_thread` with
  `kind: "agent"` creates a `ThreadKind::Agent` thread with empty `text` and its
  opening prose as the first `Take`, both under one write lock; the take is
  broadcast as SSE-only `take.added` (no stdout — `EventKind` in `src/events.rs`
  has 9 variants and no `TakeAdded`; a covering test pins `EventKind::ALL`).
  `reply.added` and `thread.created` are both SSE + stdout. Agent threads are
  soft-deletable; `Prepopulated` threads 403 on delete (agent kind is the right
  choice for demo seeds). SSE payloads serialize **camelCase**: the responder's
  filters must read `Thread.kind` (lowercase `"user"`/`"agent"`) and
  `Reply.threadId` — not the snake_case-adjacent stdout transcript naming.
- **Event bus**: `AppState::for_process()` uses `EventBus::new(1024)`
  (`src/server/app_state.rs:72`). Tokio broadcast `RecvError::Lagged` **drops**
  events (it never replays them), so the lag failure mode is a *missed* canned
  response, not a duplicate one — effectively unreachable at demo event rates.
- **Suppression seams**: `State::get_threads()` excludes soft-deleted threads;
  `State::resolution_for_thread(&ThreadId)` is `pub(crate)`
  (`src/state/store.rs:85`); `AppState::subscribe_shutdown()` is the watch-channel
  cancellation seam; `spawn_idle_timer` (`src/server/mod.rs:150`) is the exact
  pattern for a shutdown-aware background task (biased `select!` on
  `shutdown.changed()`).
- **Assets precedent**: `src/assets.rs` embeds mermaid via `include_str!` with a
  <4 MB size-budget test. `docs/demo.gif` is 2,966,143 bytes — in budget for
  `include_bytes!` (adds ~3 MB per release artifact; accepted, see Decisions).
- **Sidebar**: `discuss.html` — `body.multi-file { --files-w: 236px; }` (line 534);
  the workspace-grid gradient (517–522), `#inspect-banner`
  (`left: calc(var(--files-w)+14px)`, line 279), and `.heading-minimap`
  (`left: calc(var(--files-w)+2px)`, line 913) all derive from `--files-w`, so
  collapse is a single CSS-variable change. Marker/minimap/image-pin positions
  are pixel-measured and no `resize` event fires on a class toggle, so the
  toggle handler must call `scheduleReposition()` (as `switchToFile` does).
  `initFileSidebar()` (line 3087) rebuilds `nav.innerHTML` per hydration, sets
  `item.title = file.path` (hover text already exists), early-returns for <2
  files; `updateFileSidebar()` (line 3137) maintains active highlight +
  `.file-count` open-thread badges (counting user *and* prepopulated/agent
  threads).
- **Theme allowlist coupling**: `tests/theme.rs` `ALLOWED` pins
  `".file-item .file-count | color:#fff"` and
  `".file-sidebar .file-sidebar-title | color:var(--muted, #888)"`, and
  `allowlist_has_no_stale_entries` fails if those selector/declaration pairs
  disappear — the new header row must keep the `.file-sidebar-title` element
  (or update `ALLOWED` in lockstep, in the same change set).
- **localStorage policy**: AGENTS.md *literally* says `discuss.html` "must not
  use `localStorage` or `STORAGE_KEY`", but the enforced invariant
  (`src/template.rs::bundled_template_hydrates_state_from_seed_or_api`,
  lines ~336–350, plus the shipped `discuss-theme` / `CMD_ENTER_KEY` prefs) is
  "UI prefs only, never review state". This is a formal plan/AGENTS.md conflict:
  resolved by updating the AGENTS.md sentence to codify the UI-pref exception in
  the same change set (see D10). The new sidebar key must be added to the
  template-test allowlist. `tests/theme.rs` forbids new hardcoded colors — new
  sidebar CSS must use `var(--token)` only.
- **History**: `src/history.rs` — Done writes
  `<history_dir>/multi-N-files/<ts>.json` unless `with_no_save(true)`.
- **Raw demo material**: `docs/demo-fixtures/{plan.md,notes.md,mockup.png,
  mockup-src.html}` exist and are on-theme (Payments Retry Plan / Brownout Notes /
  Ledgerly dashboard). `mockup-src.html` is already fully self-contained
  (inline `<style>`, no external refs).

## 3. Product behavior

### `discuss demo`

- `discuss demo` starts a normal review server on a loopback port (respecting the
  global `--port` / `--no-open` flags) with six embedded files, GIF first and
  therefore selected on load.
- Two Markdown files are *revised* documents seeded with four agent-authored
  threads/takes anchored to the changed passages; their open-thread badges light
  up in the sidebar, making the revised files discoverable while the GIF stays
  the initially selected file.
- A backend-only **Demo agent** responds after a short human-feeling delay
  (~1.5 s) to (a) new user-created threads and (b) user replies on any thread,
  including the pre-seeded agent threads. Responses are normal `Take` records
  published via the existing SSE `take.added` event — the browser needs zero
  changes to display them (toast + thread refresh already exist).
- Responses are deterministic, polished, context-sensitive canned English,
  prefixed so they are clearly identifiable as the Demo agent. Exactly one
  response per triggering event; none after a thread is resolved, deleted,
  or the session is shutting down. Responses are processed sequentially by a
  single task, so a burst of user actions gets responses ~delay-apart, one
  after another — deterministic and expected, not a bug.
- Demo sessions never write history archives (forced `no_save`).
- The idle timer runs as usual: an idle demo session emits stdout-only
  `prompt.suggest_done` events. This is truthful to the protocol and is left
  unchanged (no browser impact; stdout is typically unwatched in demo use).
- `session.started` / stderr announcement / Done / exit-code semantics are
  identical to a normal session, so the demo also demonstrates the agent-facing
  stdout protocol truthfully (the responder adds no stdout events, matching real
  take semantics).

### Collapsible sidebar (all multi-file sessions, not demo-specific)

- A toggle collapses the sidebar to a ~50px icon rail: per-kind icons
  (Markdown / diff / image / HTML), compact open-thread badges on the icon
  corner, native `title` tooltips + a per-item `aria-label` carrying the file
  path, its kind, and its open-thread count (D22).
- Expanded is the first-run default; the preference persists in localStorage
  under a single UI-pref key. Nothing else is persisted. localStorage is
  origin-scoped and the default port is OS-assigned, so the pref survives across
  *sessions* only with a pinned `--port` — user-facing copy says so (D22).

## 4. Non-goals

- No live file watching, editing, or `/api/source` updates in demo mode; the demo
  does not fake any server capability beyond the responder's canned takes.
- No LLM, randomness, network calls, or lorem ipsum in responder text.
- No new `Take` schema fields (no author field) — the "Demo agent" identity lives
  in the take text; transcripts/stdout schemas are untouched.
- No responder or demo seeding in normal sessions, ever (isolation is structural:
  only the demo code path spawns/seeds).
- No sidebar redesign beyond collapse/icons/toggle; no persistence of review
  state, active file, or thread state in localStorage.
- No re-encoding pipeline for the GIF; embed `docs/demo.gif` as-is.
- No unrelated refactors; no changes to `Prepopulated` thread semantics.

## 5. File/symbol changes

### New: `assets/demo/` (embedded fixture sources)

Naming note: the repo now has three demo-named directories with distinct roles —
`demo/` (GIF recording tooling, untouched), `docs/demo-fixtures/` (raw source
material, untouched), and the new `assets/demo/` (embedded fixtures). No
technical conflict; listed here to avoid confusion.

Author five files under `assets/demo/` (content detailed in §8); the GIF is
**not** copied:

- GIF: embedded directly via `include_bytes!("../../docs/demo.gif")` — no
  `assets/demo/` copy. Copying would permanently add a second 2.9 MB blob to git
  history and create a drift hazard between the two files (see D11).
- `assets/demo/plan.md` — revised Payments Retry Plan.
- `assets/demo/notes.md` — revised Provider Brownout Notes.
- `assets/demo/retry.diff` — hand-authored unified diff matching the plan's domain.
- `assets/demo/mockup.png` — copy of `docs/demo-fixtures/mockup.png` (74 KB).
- `assets/demo/prototype.html` — self-contained dashboard prototype (derived from
  `docs/demo-fixtures/mockup-src.html`, adjusted for iframe review; zero relative
  asset references).

### `src/cli.rs`

- Add `Commands::Demo` unit-ish variant:
  ```rust
  #[command(
      about = "Open a self-contained demo review session with bundled example files.",
      long_about = "Open a self-contained demo review session with bundled example files.\n\n\
  ... no agent session, no LLM, no history archive ... still loads Prism syntax\n\
  highlighting from a CDN and checks for a newer release (D21) ...\n\n\
  Top-level flags must come first: `discuss --port 4000 --no-open demo`."
  )]
  Demo,
  ```
  `--port`/`--no-open` are **not** `global` clap args, so they must precede the
  subcommand; the help text documents this. Making them `global = true` is
  rejected — it would change parse behavior for `diff`/`update` too (scope
  creep, see D12). Extend the inline `#[cfg(test)]` parse tests (see §9).

### `src/lib.rs`

- Route `Some(cli::Commands::Demo)` in `run_with_shutdown` to a new
  `run_demo_session(verdict_config, &config, shutdown)`.
- Reject positional files combined with the subcommand: if
  `command == Some(Demo)` and `!files.is_empty()`, return the new
  `DiscussError::ConfigError { message }` variant ("`discuss demo` does not
  accept file arguments"), added to `src/error.rs` and mapped to exit code 2
  in `src/exit.rs` (no pre-existing variant had the right semantics).
- `run_demo_session`:
  1. `let (source, file_bytes) = server::demo::demo_source();`; use
     `DEMO_MODE` / `DEMO_SOURCE_LABEL` for the startup payload.
  2. Build `AppState::for_process()`
     `.with_source(source).with_file_bytes(file_bytes)`
     `.with_verdict_config(verdict_config).with_no_save(true)`
     `.with_idle_timeout_secs(config.idle_timeout_secs)` — **no** `source_path`,
     **no** `history_dir` effect (no_save forced regardless of config).
  3. `let seeded = server::demo::seed_demo_threads(&app_state);` (before serving,
     so the first `GET /` snapshot includes the seeds).
  4. `server::demo::spawn_demo_responder(app_state.clone(), seeded, Duration::from_millis(1500));`
     — the seeded id vector is threaded through rather than re-derived from
     literal `a-N` strings (D21).
  5. Reuse the existing `serve_with_ready` readiness behavior
     (`session.started` with `mode: "demo"`, `source_file: "demo"`,
     `files_count: 6`, same endpoint/instruction payload; stderr announce +
     browser open honoring `auto_open`). Implemented as the extracted
     `session_ready_callback(...)` helper used verbatim by both
     `run_review_session` and `run_demo_session`.

### New: `src/server/demo.rs` (declared `pub mod demo;` in `src/server/mod.rs`)

Lives in the server module so it can use `pub(super)` ID allocators and
`pub(crate)` store internals. Public surface is `pub` (not `pub(crate)` as
originally planned) because the integration tests in `tests/demo.rs` are an
external crate and must drive `demo_source` / `seed_demo_threads` /
`spawn_demo_responder` / `canned_response` directly (D18):

- `const` embedded content: `include_bytes!("../../docs/demo.gif")` (D11; size
  capped at record time by `demo/stitch.sh`, re-checked by `DEMO_GIF_MAX_BYTES`
  in the module's tests — D21),
  `include_bytes!("../../assets/demo/mockup.png")`, `include_str!` for the four
  text files.
- `fn demo_source() -> (Source, HashMap<FileId, (Vec<u8>, &'static str)>)`
  — builds the six `File`s in the exact §8 order with **bare-filename virtual
  paths** (`demo.gif`, `plan.md`, … — no directory component, see D15), kinds,
  empty `content` for images, and the image-bytes map with mimes `image/gif` /
  `image/png`. The mode/label live in `DEMO_MODE` / `DEMO_SOURCE_LABEL` consts
  (plus `DEMO_RESPONSE_DELAY` and `DEMO_AGENT_PREFIX`) instead of tuple slots
  — simpler call sites, same information (D18).
- `fn seed_demo_threads(app_state: &AppState) -> Vec<ThreadId>` — under one
  `state.write()` lock,
  for each of the four seeds: allocate `next_agent_thread_id()`, build a
  **fully populated** `Thread { kind: ThreadKind::Agent, text: String::new(),
  anchor_start/end, snippet, breadcrumb, image_anchor: None, line_range: None,
  element_anchor: None, orphaned: false, created_at: <now> }` whose anchors are
  computed against
  `crate::blocks::markdown_blocks(<embedded content>)` at seed-definition time
  (constants asserted by tests, §9), `state.add_thread(...)`, then
  `state.add_take(Take { id: next_take_id(), .. })` with the authored opening
  prose. No events are emitted (server not started yet; state arrives via the
  initial snapshot exactly like restored state). Returns the allocated ids in
  `DEMO_SEEDS` order, which is how the responder pairs a thread with its
  tailored follow-up (D21).
- `fn spawn_demo_responder(app_state: AppState, seeded: Vec<ThreadId>, delay: Duration)`
  — see §7.
- `fn seed_index(seeded: &[ThreadId], thread_id: &ThreadId) -> Option<usize>` and
  `fn canned_response(seed_index: Option<usize>, kind: FileKind, demo_take_count: usize)`
  — the copy selector takes the seed index, not the thread id (D21).
- Responder copy tables: `const` structured `&str` responses (see §8.3).

### `src/server/mod.rs`

- One line: `pub mod demo;` (plus route table untouched). No behavior change for
  normal sessions.

### `discuss.html` (sidebar)

CSS (all colors via existing `var(--token)`s):

- `body.multi-file.files-collapsed { --files-w: 50px; }` — grid gradient,
  inspect banner, and heading minimap offsets follow automatically.
- `.file-item .file-icon` — 18px inline SVG, `stroke: currentColor`, matching
  the header `theme-icon` sizing.
- Collapsed-state rules under `body.files-collapsed`: hide `.file-name`,
  `.file-kind`, and the `Files (N)` title text; center icons; give `.file-item`
  `position: relative` and absolutely position `.file-count` as a corner
  mini-badge (existing `.zero` hide logic carries over); keep focus outline
  visible within the 50px rail by reducing horizontal padding on **both**
  `.file-item` and the sidebar container (its current padding alone would leave
  ~16px for an 18px icon).
- `.file-sidebar-toggle` button styles (reuse `.toggle`-style tokens).

JS (`initFileSidebar` / new helpers):

- New guarded pref helpers using key `'discuss-files-collapsed'`
  (`FILES_COLLAPSED_KEY` const): try/catch around `localStorage.getItem/setItem`,
  copied from the theme pattern. Absent/anything-but-`'1'` ⇒ expanded
  (first-run default).
- `initFileSidebar` additions: read pref → toggle `document.body.classList`
  `files-collapsed` (applied before the <2-files early return is harmless — the
  CSS is scoped under `body.multi-file`); render a header row that **keeps the
  existing `.file-sidebar-title` element** (theme-allowlist coupling, §2) plus a
  `<button type="button" class="file-sidebar-toggle" aria-controls="file-sidebar"
  aria-expanded="true|false" aria-label="Collapse file list"/"Expand file list">`
  with a chevron SVG (`aria-hidden="true"`). Click handler flips the body class,
  updates `aria-expanded`/`aria-label`, persists the pref, and calls
  `scheduleReposition()` so pixel-measured markers/minimap/image pins re-anchor
  to the new layout (no resize event fires on a class toggle). State lives on
  `body`, so it survives `initFileSidebar` innerHTML rebuilds.
- Per item: prepend an `aria-hidden` per-kind SVG icon keyed on `file.kind`
  (`markdown` doc icon, `diff` ±icon, `image` picture icon, `html` code icon);
  compose the accessible name once into `item.dataset.a11yBase`
  (`"<path>, <kind>"` for non-markdown files, bare path otherwise) and set it as
  `aria-label`; `updateFileSidebar` appends the open-thread count to that base.
  aria-label replaces the button's content, the icon is `aria-hidden`, and
  `.file-kind` is `display:none` in the rail, so the kind must be in the label
  (D22). `title` hover text already set at line 3106 stays.
- No-op for single-file sessions (existing `<2 files` early return already
  covers it).

### `src/template.rs`

- Extend the localStorage allowlist in
  `bundled_template_hydrates_state_from_seed_or_api` to also accept
  `FILES_COLLAPSED_KEY` / `discuss-files-collapsed` contexts.
- New template tests for toggle/icons/rail (see §9).

### `AGENTS.md`

- Amend the sentence "`discuss.html` must not use `localStorage` or
  `STORAGE_KEY`" to codify the already-enforced invariant: localStorage is for
  UI preferences only (theme, Cmd+Enter hint, sidebar collapse), never review
  state, with the template-test allowlist as the gate (D10). One-sentence edit;
  no other AGENTS.md changes.

### `CHANGELOG.md`

Under `## [Unreleased]` add `### Added` entries:

- **Demo mode** — `discuss demo` opens a bundled six-file gallery
  (feature-tour GIF, two revised markdown docs pre-annotated with agent takes, a
  diff, an image, and an HTML prototype) with a deterministic Demo agent that
  replies to your comments in-process over the normal SSE take flow. No agent
  session, no LLM, no history writes; the page still loads Prism highlighting
  from a CDN and runs the usual release check (D21).
- **Collapsible file sidebar** — multi-file sessions can collapse the sidebar to
  a ~50px icon rail with per-kind file icons, open-thread badges, a labelled
  toggle (tooltip + `aria-expanded`), and per-file names split by channel: the
  `title` tooltip carries the path, the accessible name adds kind and
  open-thread count (D23); defaults to expanded and persists to `localStorage`
  under `discuss-files-collapsed` (origin-scoped, so cross-session persistence
  needs a pinned `--port` — D22).

### `docs/plans/demo-mode.md`

This file; keep updated as decisions change.

## 6. Data flow

### Bundled inputs

```text
assets/demo/* + docs/demo.gif --include_bytes!/include_str!--> src/server/demo.rs consts
  -> demo_source(): Source{files f-1..f-6 in fixed order} + image bytes map
  -> AppState::with_source / with_file_bytes (sha256 versions for raw URLs)
  -> GET /            : render_root_page renders f-1 (GIF) into #doc-content,
                        injects __DISCUSS_RENDERED_FILES__ + state seed
  -> GET /api/files/f-1/raw, /f-5/raw : embedded bytes, image/gif|png + CSP sandbox
  -> GET /files/f-6   : embedded HTML served from File.content; asset route
                        /files/f-6/assets/* 404s deterministically because the
                        bare virtual path's parent ("") never canonicalizes
                        (D15); content is self-contained so nothing requests it
```

### Pre-seeded takes

```text
seed_demo_threads(): one write lock ->
  4x [next_agent_thread_id() -> add_thread(kind=Agent, text="")
      next_take_id()         -> add_take(opening prose)]
-> State::snapshot_with_files() feeds both initial page hydration and
   /api/state -> sidebar badges count the open agent threads on f-2/f-3
-> normal lifecycle from there: reply/resolve/delete all work (Agent kind).
No stdout/SSE emission at seed time (server not yet started).
```

### Fake responder

```text
spawn_demo_responder(app_state, delay):
  bus.subscribe() -> filter BroadcastEvent.kind in {"thread.created","reply.added"}
    thread.created: payload.kind == "user" (ignore agent/self)  -> key T:<id>, thread <id>
    reply.added:                                                -> key R:<id>, thread <threadId>
  (payloads use serialized `id` plus camelCase `threadId` for replies — see §2)
  dedupe: HashSet<String> of handled keys (single task, no lock needed)
  sleep(delay) via select! against shutdown.changed()
  then under state.write():
    - thread must still be in get_threads() (soft-delete check)
    - resolution_for_thread(id).is_none()
    - Done not started (`done_started`) and shutdown not signaled — see §7
    -> add_take(Take{ id: next_take_id(), thread_id, text: canned(context) })
  app_state.record_mutation()
  bus.publish(BroadcastEvent{ kind: "take.added", payload }) — SSE only,
  no EventEmitter call (mirrors post_api_thread_takes exactly).
```

Copy selection (`canned(context)`) is a pure function of: file kind of the
thread's file, the thread's index in the seeded id vector returned by
`seed_demo_threads` (`Some(i)` ⇒ per-seed tailored follow-up, `None` ⇒ a thread
the reviewer opened), and the count of existing Demo-agent takes in that thread
(0 → opener, 1 → follow-up, ≥2 → short closer). Fully deterministic.

Trigger scope is **per-event, not per-human**: `reply.added` fires for any
`POST /api/threads/{id}/replies`, so a curl user in a demo session also gets a
canned response. The "exactly one response per triggering event" invariant is
defined against events, which is the intended behavior.

## 7. Concurrency & shutdown handling

- Responder is one `tokio::spawn`ed task modeled on `spawn_idle_timer`:
  `loop { select! { biased; shutdown.changed() => break, event = rx.recv() => ... } }`.
  Handle `RecvError::Lagged` by continuing (same policy as `get_api_events`);
  `RecvError::Closed` breaks. With the 1024-capacity bus (§2) and the inline
  post-trigger sleep, lag is effectively unreachable; while one response is
  pending, further triggers queue in the broadcast buffer and are answered
  sequentially (~delay apart). Note broadcast lag *drops* events — the residual
  risk is a missed response, never a duplicate; the dedupe `HashSet` is
  belt-and-braces on top of that.
- The post-delay sleep also races `shutdown.changed()` so a Done during the
  delay cancels the pending take (requirement: no take after session shutdown).
- `shutdown` alone is not a sufficient Done guard: `post_api_done` builds and
  emits the transcript *before* calling `shutdown.signal()`, so a take added in
  that window would reach the browser over SSE while being absent from the
  emitted transcript. `AppState::begin_done()` latches a `done_started` flag
  immediately before the transcript read lock, and the responder checks it under
  the state write lock — closing the window (D21).
- Re-validation happens *after* the delay *under the write lock*, so
  resolve/delete that raced the delay wins and suppresses the response.
- Because the responder never subscribes to `take.added` and only reacts to
  user-kind thread creations and replies, it cannot respond to itself or loop.
  The no-loop invariant for `reply.added` additionally depends on demo sessions
  having **no agent client** posting replies (true today: the responder itself
  posts only takes, never replies). Any future demo extension that adds
  agent-posted replies must revisit this filter.
- Seeding runs before `serve_with_ready`, so there is no race with HTTP handlers.
- Normal-mode isolation is structural: `run_review_session` never references
  `server::demo`; no `AppState` flag exists to accidentally flip.

## 8. Demo content

### 8.1 File order (exact)

| id  | path             | kind     | notes |
|-----|------------------|----------|-------|
| f-1 | `demo.gif`       | image    | feature tour GIF; selected on load |
| f-2 | `plan.md`        | markdown | revised; seeded threads a-1, a-2 |
| f-3 | `notes.md`       | markdown | revised; seeded threads a-3, a-4 |
| f-4 | `retry.diff`     | diff     | static hand-authored hunk matching plan.md |
| f-5 | `mockup.png`     | image    | Ledgerly dashboard screenshot |
| f-6 | `prototype.html` | html     | self-contained Ledgerly prototype |

Paths are deliberately bare filenames (no `demo/` prefix): D15 makes the HTML
asset route unreachable regardless of the user's CWD. Display cost is nil —
the sidebar and tooltips just show the short names.

### 8.2 Revised markdown + seeded takes (prose and takes authored together)

`plan.md` (based on `docs/demo-fixtures/plan.md`, with two deliberate revisions):

- Revision A: backoff attempts `3 → 5` with a new 30 s total ceiling in the
  `charge_with_retry` rust fence and surrounding prose.
  **a-1** anchored to that block: Demo agent explains the change ("raised the
  attempt cap from 3 to 5 and added a 30 s ceiling after replaying the March
  brownout") and asks the reviewer to confirm the ceiling fits the 50 rps
  provider budget.
- Revision B: rollout table gains a Stage 0 shadow-mode row and Stage 2 exit
  criteria tightened `0.5% → 0.3%`.
  **a-2** anchored to the table: explains both edits and asks whether 0.3% is
  achievable for the 5% cohort before widening rollout.

`notes.md` (based on `docs/demo-fixtures/notes.md`, expanded):

- Revision C: recovery estimate `84% → 79%` with a sentence explaining that
  429-rate-limited attempts were excluded from the replay.
  **a-3** anchored there: explains the exclusion and asks whether the business
  case still holds at 79%.
- Revision D: new paragraph reserving a 20% retry budget of the 50 rps limit.
  **a-4** anchored to the new paragraph: asks the reviewer to confirm the
  80/20 split with the merchant-traffic team.

Anchors (`anchor_start`/`anchor_end`, snippet, breadcrumb) are derived from
`markdown_blocks()` over the final authored content and pinned by tests so the
prose and callouts cannot drift apart.

### 8.3 Responder copy (deterministic canned English)

All strings begin with a `Demo agent — ` prefix. Tables (const, in
`src/server/demo.rs`):

- **Seeded-thread follow-ups**: one tailored reply per a-1..a-4 (e.g. for a-1:
  acknowledges the reviewer's answer, notes the ceiling keeps worst-case retry
  load under 10 rps, offers to update the plan wording), plus one generic closer
  for further replies ("Noted — I've logged that; resolve this thread when
  you're satisfied.").
- **User-thread openers by file kind**: markdown (comments on discussing a
  passage), diff (talks about the hunk/regression risk), image (references the
  pinned location), html (references the selected element). Each nudges the
  user toward the reply/resolve affordances being demonstrated.
- **User-thread follow-up + closer**: depth-indexed as in §6.

No timestamps, randomness, or interpolated user text beyond safe, fixed phrasing.

### 8.4 `retry.diff`

A small hand-authored unified diff (`diff --git a/src/charge.rs b/src/charge.rs`)
showing the 3→5/backoff-ceiling change from plan.md, so the diff pane content is
coherent with the seeded story. Parsed through the existing diff-file rendering
path as `FileKind::Diff` content (no git invocation). Because it is
hand-authored, nothing in the codebase validates the hunk header — the counts
were wrong on first authoring and were corrected to `@@ -10,7 +10,8 @@` in D22;
verify any future edit with `git apply --check`.

## 9. Test matrix

Unit — `src/cli.rs` tests:
- `demo` parses to `Commands::Demo`; `discuss --port 4000 --no-open demo`
  parses with both flags set (flags-before-subcommand ordering, §5).
- `discuss --verdict-options "Ship it,Hold" demo` parses with the verdict
  options populated and `Commands::Demo` selected (pins D9's pass-through).
- `discuss --help` lists `demo`; `discuss demo --help` documents the
  flag-ordering constraint.

Unit — `src/server/demo.rs` tests:
- `demo_source()` returns exactly 6 files in §8.1 order with expected ids,
  paths, kinds; images have empty `content` + bytes-map entries with correct
  mimes; gif is `files[0]`.
- Size budget: embedded gif+png total `< 4 * 1024 * 1024` (assets.rs precedent).
- Anchor lockstep: for each seed, `markdown_blocks(content)[anchor_start..]`
  snippet matches the seed's snippet/breadcrumb (prose↔take drift guard).
- `prototype.html` contains no `src=`/`href=` relative asset references
  (self-containment guard).
- Path safety (D15): every `demo_source()` path has no directory component
  (`FsPath::new(path).parent() == Some(Path::new(""))`), so `get_html_asset`'s
  parent-canonicalize step can never resolve against the user's CWD.
- Responder copy: every (kind × depth) key resolves; identical inputs give
  identical strings; every string starts with the Demo agent prefix; none are
  lorem ipsum/empty.

Integration — new `tests/demo.rs` file (`tests/server.rs`-style: in-process
`AppState` + router, `SharedWriter` stdout capture):
- Seeded snapshot: `GET /` + `/api/state` include a-1..a-4 with takes t-1..t-4;
  a user-created agent thread after seeding allocates `a-5` (no ID collision).
- Responder initial: create user thread → after injected short delay, one
  `take.added` SSE with Demo-agent text; **stdout has no take event**.
- Responder self/agent filter (primary no-loop guard, §6): POST an
  agent-kind thread (`kind: "agent"`) in a demo session → **no** canned
  response is ever produced for its `thread.created` event.
- Responder follow-up: reply on seeded `a-1` → tailored follow-up take; second
  reply → closer; exactly one take per reply (dedupe).
- Suppression: (a) resolve thread during delay → no take; (b) delete thread
  during delay → no take; (c) signal shutdown during delay → no take;
  (d) dedupe belt-and-braces: feeding the responder the same trigger key twice
  produces one take (broadcast lag *drops* events rather than replaying them,
  so this guards the responder's own bookkeeping, not a real bus behavior).
- Burst behavior (replaces an earlier "assert bus capacity" idea, which is not
  implementable — `EventBus` exposes no capacity accessor and `1024` is a
  literal in `AppState::for_process()`; a literal-equals-literal assertion
  would also prove nothing): fire several rapid user triggers with a short
  injected delay and assert exactly one response per trigger arrives, in
  order, with no drops — behaviorally pinning §7's headroom premise.
- Asset-route safety (D15): with a `demo/` directory present in the test CWD
  (this repo has one), `GET /files/f-6/assets/<existing-name>` → structured
  404, never file bytes.
- Normal-mode isolation: a standard `run_review_session`-shaped `AppState`
  (no `spawn_demo_responder`) produces zero takes for the same stimuli, and
  stdout event stream is unchanged.
- Demo history: Done on a demo-configured state writes no archive (no_save).

Template — `src/template.rs` tests:
- Sidebar toggle markup: `file-sidebar-toggle`, `aria-expanded`,
  `aria-controls="file-sidebar"`.
- Per-kind icon markers present (`file-icon` + kind-keyed SVG ids/classes).
- `files-collapsed` CSS rule sets `--files-w` in the 48–52px range (assert
  `--files-w: 50px`).
- localStorage allowlist now includes `discuss-files-collapsed` and still
  rejects any other key (existing loop keeps enforcing).
- Expanded-by-default: template JS treats missing pref as expanded.

Binary smoke — `tests/cli.rs`:
- Spawn `CARGO_BIN_EXE_discuss --no-open --port <free> demo` (flags first);
  read `session.started` (assert `mode == "demo"`, `files_count: 6`); `GET /`
  contains `id="file-sidebar"` seed data; `GET /api/files/f-1/raw` → 200
  `image/gif`; `GET /files/f-6` → 200 HTML; create a user thread via the API,
  wait past the responder delay, then assert stdout contains **no**
  `take.added`-shaped line (wire-level normal-semantics check); `POST /api/done`
  → exit 0. Assert stderr announce line present. (This is the runnable demo
  smoke path.)
- `discuss somefile.md demo` exits 2 with the config error message.

Theme guard — `tests/theme.rs` continues to pass (new CSS uses vars only).

Optional (time-permitting) — `tests/browser/` CDP script mirroring
`html-prototype-smoke.mjs` that toggles collapse, reloads, and asserts the
persisted rail state + icon visibility. Not a CI gate.

## 10. Verification commands

```sh
cargo fmt --check
cargo clippy --all-targets    # with RUSTFLAGS="-D warnings" to match CI
cargo build --all-targets
cargo test
cargo test --test cli demo    # focused smoke
cargo run -- demo --no-open   # manual: check session.started, open printed URL,
                              # verify GIF selected, badges on plan.md/notes.md,
                              # comment -> ~1.5s Demo agent take, resolve suppresses,
                              # sidebar collapse persists across reload, Done exits 0
awk '/^## \[Unreleased\]/,/^## \[/' CHANGELOG.md   # changelog smoke
```

CI order to respect: `fmt -> clippy --all-targets -> build --all-targets -> test`.

## 11. Decisions log

- **D1 — Embed the existing 2.9 MB GIF.** Binary grows ~+3 MB/target; no release
  size gate exists. Re-encoding pipeline rejected as scope creep. Revisit if a
  size gate is added.
- **D2 — Seeds are `ThreadKind::Agent`, not `Prepopulated`** so the user can
  exercise delete/resolve on them.
- **D3 — Responder gated structurally** (only `run_demo_session` spawns it), not
  by an `AppState` flag — smaller surface, impossible to enable accidentally.
- **D4 — `no_save` forced on** for demo sessions; demo reviews are throwaway.
- **D5 — Text/png fixtures under `assets/demo/`** consistent with the
  bundled-asset convention; `docs/demo-fixtures/` remains untouched raw
  material. The 74 KB `mockup.png` copy is acceptable; the GIF is not copied
  (D11).
- **D6 — Rail width 50px** (within the 48–52px requirement) via one CSS var.
- **D7 — Take identity via text prefix** ("Demo agent — "), no schema change.
- **D8 — `mode: "demo"` in `session.started`**; existing payload fields all
  preserved (`files_count: 6`, `source_file: "demo"`).
- **D9 — Verdict flags pass through to demo** unchanged (shared plumbing, zero
  extra code); files + `demo` is a config error.
- **D10 — AGENTS.md localStorage sentence updated in this change set** to codify
  the enforced "UI prefs only" invariant (matching the shipped
  `discuss-theme`/`CMD_ENTER_KEY` precedent and the template-test allowlist),
  resolving the literal-ban conflict flagged in review.
- **D11 — GIF embedded from `docs/demo.gif` directly** via
  `include_bytes!("../../docs/demo.gif")`; no `assets/demo/demo.gif` copy.
  Avoids a second permanent 2.9 MB blob in git history and a two-copy drift
  hazard. Accepted tradeoff: the README asset and the demo file are the same
  bytes, so re-recording the README GIF changes the demo too (desirable — the
  demo should show the current tour).
- **D12 — No `global = true` on `--port`/`--no-open`** (rejected): it would
  change parse behavior for `diff`/`update` too. Flags-before-subcommand is
  pinned in tests and documented in `demo`'s help text instead.
- **D13 — SKILL.md / llms.txt left unchanged; README mention added late**:
  `skills/discuss/SKILL.md` documents `mode` as markdown/diff/mixed, but demo
  mode is human-facing — agents never invoke `discuss demo`, so agent-facing
  docs are not extended in this change set. Verified safe: no test pins the
  SKILL.md `mode` enumeration, so emitting `mode: "demo"` breaks nothing.
  The README mention was originally deferred, then added in the final
  verification pass (see D20) at reviewer prompting; CHANGELOG still carries
  the user-facing announcement.
- **D14 — Review claim rejected**: a plan review asserted `.heading-minimap`
  does not reference `--files-w`. Verified false — `discuss.html:913` is
  `left: calc(var(--files-w) + 2px)` inside the `.heading-minimap` rule
  (block starts at line 910). §2 stands as written.
- **D15 — Bare-filename virtual paths** (was `demo/<name>`): the final
  architecture audit showed `get_html_asset` resolves `file.path`'s parent
  against the process CWD, so a `demo/` virtual parent would serve real disk
  files if the user's CWD contains a `demo/` directory (this repo does).
  Bare filenames make `parent()` the empty path, which never canonicalizes on
  any platform → the asset route 404s unconditionally. Pinned by a unit
  invariant (no directory components) and an integration 404 test (§9).
- **D16 — "Transposed line numbers" review claim rejected**: the final
  verification review asserted §2's citations for `--files-w: 236px` (534) and
  `#inspect-banner` (279) were swapped. Re-verified against `discuss.html`:
  line 534 is `body.multi-file { --files-w: 236px; }` and line 279 is the
  banner's `left: calc(var(--files-w) + 14px)` (rule opens at 275). §2 stands
  as written.
- **D17 — Bus-headroom test dropped, replaced by a burst test**: asserting the
  1024 capacity is not implementable (`EventBus` has no capacity accessor; the
  value is a literal in `AppState::for_process()`, src/server/app_state.rs:72)
  and adding an accessor just to compare two literals would be churn without
  signal. §9 instead pins the premise behaviorally: N rapid triggers → N
  in-order responses, no drops.
- **D18 — Demo module surface is `pub`, mode/label are consts** (implementation
  deviation from the original `pub(crate)` 4-tuple sketch): integration tests
  live in `tests/demo.rs`, an external test crate, so the driving functions
  must be `pub`; `demo_source` returns `(Source, bytes-map)` and the
  mode/label/delay/prefix are exported consts. Seed snippets/breadcrumbs are
  computed from `markdown_blocks` at seed time (never hardcoded), and unit
  tests pin each anchor index to a distinctive phrase from the revision it
  annotates.
- **D19 — Verification-review fixes** (from the verification ledger):
  (a) `respond_to_thread`'s shutdown check moved *under* the state write lock,
  matching what §6's diagram already specified — closes the small
  check-then-write window a racing Done could slip through;
  (b) the sidebar collapse toggle now sets a `title` tooltip synced with its
  `aria-label`, so sighted mouse users get the same affordance;
  (c) because `aria-label` overrides a button's content for assistive tech,
  `updateFileSidebar` composes the open-thread count into each file item's
  accessible name (`"plan.md, 2 open threads"`) whenever badges refresh — the
  count is no longer visual-only. Template tests pin (b) and (c).
  Also hardened two load-sensitive 2 s startup `recv_timeout`s in
  `tests/cli.rs` (history-dir test, demo smoke) to 10 s after a reviewer
  reproduced a flake under concurrent-build load; the pass path is
  unaffected. Explicitly rejected as no-ops: seeding-time panic/poison
  hardening (startup-only, test-pinned), README/SKILL mentions (D13 stood at
  the time; the README half was later revisited in D20), the optional CDP
  sidebar script (§9 non-gate), responder `handled`-set growth and sequential
  burst pacing (documented §3/§7 behavior).
- **D20 — Final verification pass**: added the minimal README coverage the
  final-quality reviewer suggested (one CLI-table row plus a two-sentence
  "first time here" pointer in Quick Start · Without an agent); no test pins
  README content. `llms.txt`/SKILL.md remain unchanged per D13's agent-facing
  rationale. Rejected as out of scope: hardening the pre-existing
  `free_port()` bind-release-rebind race in `tests/cli.rs`
  (`cli_no_open_logs_listening_url_to_stderr`) — the flake predates this
  branch, reproduced only under parallel-suite load, passed on reruns, and
  the suggested fix (rework tests to `--port 0` + parse the reported port)
  would refactor untouched tests, violating the surgical-changes rule.

- **D21 — PR-review pass fixes** (independent review of the complete branch):
  - **"No network" claim scoped, not engineered away.** `discuss demo` was
    advertised as "fully offline / No network" in `src/cli.rs`, `README.md` and
    `CHANGELOG.md`, but the served page loads Prism CSS+JS from unpkg
    (`discuss.html` head, pinned by `template.rs`) and `initVersionBadge()`
    always calls `/api/version` → GitHub. Vendoring Prism was **rejected**:
    AGENTS.md pins "Prism loaded from unpkg" as the design, and the autoloader
    resolves grammars on demand, so a genuine offline bundle means vendoring the
    grammar set — far outside this change set. Suppressing `/api/version` in
    demo mode was also rejected (a demo-only branch in a shared, browser-facing
    endpoint for a claim that would still be wrong about Prism). The docs now
    say what is actually true: *no agent session, no LLM, no history writes*,
    with the CDN/version-check fetches named explicitly and the graceful offline
    degradation (no highlighting, no per-line diff comments — the line-numbers
    plugin never emits `.line-numbers-rows`) called out. Pinned by the `cli.rs`
    help tests.
  - **Done race genuinely closed.** `respond_to_thread`'s comment claimed its
    under-the-write-lock `shutdown.is_signaled()` check closed the Done race, but
    `post_api_done` signals shutdown only *after* building and emitting the
    transcript. Added `AppState::begin_done()` / `done_started()` (a `SeqCst`
    latch set immediately before the transcript read lock) and check it in the
    responder. Non-demo behavior is unchanged: nothing else reads the flag.
    Pinned by `responder_suppresses_takes_once_done_has_started`.
  - **Seed pairing made explicit.** `canned_response` matched literal `"a-1"`..
    `"a-4"` while the seeder documented the ids as allocator-derived.
    `seed_demo_threads` now returns the allocated ids in `DEMO_SEEDS` order,
    `spawn_demo_responder` takes that vector, and `canned_response` takes an
    `Option<usize>` seed index resolved by `seed_index(...)`. No literal thread
    ids remain in the responder.
  - **GIF/binary coupling guarded where it is caused.** `demo/stitch.sh` now
    fails the re-record if `docs/demo.gif` exceeds a 3.5 MiB cap, with a message
    pointing at the gifski knobs; `DEMO_GIF_MAX_BYTES` re-checks the same cap in
    `src/server/demo.rs` tests. Previously the only guard was a 4 MiB unit-test
    budget with less headroom than the last re-record consumed (+1.9 MB), so the
    next routine `make -f demo/Makefile demo` would have failed an apparently
    unrelated test.
  - **Test timeout inconsistency resolved.** The widened startup wait in the
    unrelated `cli_history_dir_flag_overrides_config_history_dir_and_writes_archive`
    was not reverted but generalized: `tests/cli.rs` now has one
    `STARTUP_TIMEOUT` constant applied to every "wait for the spawned binary to
    announce itself" step, so the six previously-2 s sites cannot drift apart.
    No measured startup regression was found locally (`--version` returns in
    ~0 ms); the constant's doc says so rather than asserting an unverified cause.
    Process-exit `wait_with_timeout` waits are untouched.
  - **Comment corrected.** The sidebar-toggle comment claimed markers, minimap,
    and image pins needed explicit re-anchoring; `scheduleReposition()` only
    re-lays-out `.thread.open` cards. Verified that is sufficient: the grid is
    `var(--files-w) var(--left-w) var(--divider-w) 1fr` with `--left-w: 60%` of
    the container, so collapsing changes only pane-right's width; the minimap
    re-anchors from `calc(var(--files-w) + 2px)` and image pins are
    percentage-positioned. Comment (and the mirrored `template.rs` test comment)
    now say that.
  - **Fixture-location convention carved out** in `AGENTS.md` alongside the D10
    localStorage precedent: demo fixtures are session inputs, not browser
    assets, so they stay in `assets/demo/` + `src/server/demo.rs` rather than
    being re-exported through `src/assets.rs`.
  - **Untagged fences fixed**: the three ASCII flow diagrams in this file are now
    ```` ```text ```` per AGENTS.md's "tag every fence with a language".
  - Rejected: the review's suggestion that `docs/plans/` sidesteps a gitignore
    convention — `git ls-files docs/plans` already tracks three plan documents.

- **D22 — Second PR-review pass fixes** (independent re-review of the whole
  branch after D21):
  - **`assets/demo/retry.diff` hunk header corrected** `@@ -10,9 +10,10 @@` →
    `@@ -10,7 +10,8 @@`. The body is 6 context + 1 removal + 2 additions ⇒
    old 7 / new 8. Nothing validated it: `src/diff.rs` splits on the literal
    `@@` and emits the hunk verbatim, so the malformed header rendered straight
    into the demo's diff pane — the one file shown to the most diff-literate
    audience the project has. Confirmed by reproduction: `git apply --check`
    rejects the old header ("corrupt patch at line 15") and accepts the new one.
  - **CHANGELOG persistence claim corrected.** "persists per browser" was wrong
    in the default flow: `config.port.unwrap_or(0)` binds an OS-assigned port,
    so every session is a new origin and therefore a new localStorage
    partition. Now matches the 0.4.0 theme entry's phrasing — names the key and
    states the fixed-`--port` caveat. (The same caveat has always applied to
    `discuss-theme`; this change set is the one that made persistence a
    headline claim, so it is fixed here rather than treated as pre-existing.)
  - **Vacuous test assertion fixed.** `seed_anchors_match_the_revised_passages`
    passed `""` as the expected breadcrumb for both `notes.md` seeds, and
    `str::ends_with("")` is unconditionally true — half the seeds had no
    breadcrumb verification despite the test's stated purpose. Replaced with the
    real value, `"Provider Brownout Notes"` (the file's only heading), plus a
    guard asserting no expectation is empty so the trap cannot reappear.
  - **Accessible-name regression fixed.** `item.setAttribute('aria-label',
    file.path)` replaced the button's content-derived name, silently dropping
    the `.file-kind` tag text that assistive tech previously announced — worst
    in the new collapsed rail, where `.file-kind` is `display:none` and the icon
    is `aria-hidden`, leaving no kind signal at all. The name is now composed
    once into `item.dataset.a11yBase` (path + kind) and `updateFileSidebar`
    appends the open-thread count to that base. This is the same rule D19(c)
    applied to the count, now applied to the kind. Template test updated.
  - Accepted without change (verified, not findings): responder
    scheduling/dedupe/cancel-safety, `begin_done()` placement after verdict
    validation, camelCase event payload keys, seed anchors 8/12/2/4, the
    bare-filename asset-route 404, the `stitch.sh`/`DEMO_GIF_MAX_BYTES` pair,
    single-`initFileSidebar` ordering, and collapsed-rail CSS specificity.

- **D23 — Final CHANGELOG pass** (read the shipped behavior back into the
  Unreleased entries; no code changed):
  - **Hover vs. accessible name un-conflated.** Both the entry and §5 above read
    "hover and screen-reader file names (path, kind, and open-thread count)",
    but the two channels differ: `item.title` is set once to `file.path`
    (`discuss.html`, `initFileSidebar`) and never updated, while kind and count
    live only in the `aria-label` composed from `dataset.a11yBase` (D19c, D22).
    The kind is included exactly when the visual `.file-kind` tag is rendered
    (`file.kind !== 'markdown'`), so the entry says "the kind tag when one is
    shown" rather than implying every row announces a kind.
    The entry now states each channel separately — the promise D22 actually
    shipped is that the *accessible* name survives the collapsed rail, not that
    the tooltip grew.
  - **Demo responder described as it behaves, not as a black box.** Added the
    ~1.5 s delay (`DEMO_RESPONSE_DELAY`), the seeded-thread-vs-user-thread split
    with kind-aware openers (`canned_response`), and the suppression contract
    (resolved, soft-deleted, or `done_started()` ⇒ no take) — all already pinned
    by `tests/demo.rs`, previously invisible to a reader of the changelog.
  - **Binary-size effect stated.** `include_bytes!("../../docs/demo.gif")` adds
    ~3 MB to every released artifact (D1/D11, capped at 3.5 MiB by
    `demo/stitch.sh` + `DEMO_GIF_MAX_BYTES`). Users download that; a changelog
    that omits it hides the one cost of the feature.
  - **`discuss demo` takes no file arguments** — a `ConfigError` in `run`, worth
    one clause since the CLI otherwise accepts files alongside a subcommand
    (`discuss plan.md diff HEAD~1..HEAD`).
  - **Offline degradation carried over from the README wording** (no
    highlighting, no per-line diff comments) so the scoped D21 claim reads the
    same in both places.
  - Verified accurate, left as written: six-file order with the GIF first
    (`demo_source` unit test), "dashboard screenshot" for `mockup.png` (§8.1),
    "self-contained HTML prototype", the CDN/version-check scoping (D21), the
    flags-before-subcommand example, the ~50px rail (`--files-w: 50px`), the
    `discuss-files-collapsed` key, and the origin-scoping caveat (D22).

## 12. Iteration policy

- Work in small verified steps, in this order: (1) this plan; (2) CLI + demo
  source/seeding + tests; (3) responder + tests; (4) sidebar collapse + template
  tests; (5) binary smoke + CHANGELOG; run `cargo fmt && cargo clippy
  --all-targets && cargo test` after each step and fix regressions before moving on.
- If implementation contradicts this plan (e.g. an anchor computation or
  allowlist detail), update this file in the same change set.
- Keep every changed line traceable to §5; no adjacent cleanups.
- Do not commit, push, or open a PR in this workflow; a separate final review
  workflow handles that.

## 13. Blocked-stop conditions

Stop and surface the blocker (do not improvise) if any of these occur:

- `include_bytes!` of the GIF pushes a build/CI limit or the size-budget test
  cannot be satisfied without re-encoding (needs a product decision on D1).
- Seeding via `next_agent_thread_id` proves impossible without widening
  `pub(super)` visibility beyond the `server` module in a way that violates the
  architecture rules.
- The localStorage allowlist test cannot accommodate the sidebar key without
  weakening the state-in-localStorage ban.
- The responder cannot guarantee no-response-after-resolve/delete/shutdown
  without changing public handler semantics or adding stdout events.
- `tests/theme.rs` or the CI `-D warnings` gate requires touching unrelated
  code to pass.
- Any requirement conflict between this plan and AGENTS.md discovered mid-flight.
