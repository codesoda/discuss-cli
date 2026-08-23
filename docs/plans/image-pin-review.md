# Plan: Image review with pin anchors

Issue: https://github.com/codesoda/discuss-cli/issues/28

Support reviewing images (`discuss mockup.png`, alone or in multi-file sessions). The image
renders in the browser, the reviewer drops numbered pins at points on the image, and each pin
anchors a comment thread using the existing thread/reply/take model.

## Context

The codebase already has a multi-file session model (`File { id, path, kind, content }`,
`FileKind::{Markdown, Diff}`), a per-file render pipeline (`render_file_html` in
`src/server/pages.rs`), file-scoped threads (`Thread.file_id` + `anchor_start`/`anchor_end`
paragraph anchors), REST mutations, SSE, and stdout events. The `Diff` kind is a good precedent
for adding a new kind — the main new wrinkles are **binary content** and a **coordinate anchor**.

## Design decisions

1. **Anchor model**: add `image_anchor: Option<ImageAnchor { x_pct, y_pct }>` to `Thread`
   (serialized `imageAnchor: {xPct, yPct}`, skipped when `None`). Keep `anchor_start`/`anchor_end`
   present (set both to the pin number) so existing ordering/transcript/draft code keeps working
   without a parallel code path. Pin number = 1-based creation order per image file.
2. **Binary content**: `File.content` is a `String`. Don't base64 into it — keep `content` empty
   for images and serve bytes from disk via a new route `GET /api/files/{fileId}/raw` (read once
   at startup into an `AppState` bytes map so the session is stable even if the file changes on
   disk; matches how markdown is read once).
3. **SVG**: treat as image (render via `<img>`), not markdown.
4. **`POST /api/source` on an image file**: reject with `validation_error` (out of scope; live
   image updates can come later).
5. **Drafts for un-submitted pin comments**: keying the new-thread draft machinery by rounded
   coords is messy; v1 keeps the pin composer local-only (no server draft). Losing an unsent pin
   comment on reload is acceptable — flag in the PR.

## Open questions (settle before implementing)

- **Basis points vs f64** for pct storage. `Thread` derives `Eq`; f64 breaks that. Either impl
  `PartialEq` manually or store pct as `u16` basis points (e.g. `4200` = 42.00%). Lean basis
  points to keep `Eq` and avoid float-serde noise.
- Whether pin **drafts** must persist server-side in v1 (proposal: no).
- Whether **SVG** should ever render inline instead of via `<img>` (proposal: `<img>` only —
  safer, no script execution).

## Steps

### 1. Backend: file kind + loading — `src/state/types.rs`, `src/lib.rs`

- Add `FileKind::Image` (serializes `"image"`).
- `file_kind_for_path`: map `png|jpg|jpeg|gif|webp|svg` → `Image`.
- In session loading (`src/lib.rs` ~line 300–345): for image files use `fs::read` (bytes) not
  `read_to_string`; store empty `content` on the `File` and keep bytes in a
  `HashMap<FileId, (Vec<u8>, &'static str /* mime */)>` passed into `AppState`.
- Verify: unit tests for kind detection + loading a small PNG fixture.

### 2. Backend: serve bytes — `src/server/pages.rs`, `src/server/mod.rs`, `app_state.rs`

- `GET /api/files/{fileId}/raw` (axum 0.8 `{id}` syntax): look up bytes in `AppState`, correct
  `Content-Type` by extension, `Cache-Control: public, max-age=86400`; structured 404 for
  unknown/non-image ids.
- `render_file_html` for `FileKind::Image`: emit a small HTML shell, e.g.
  `<div class="image-review" data-file-id="f-2"><img src="/api/files/f-2/raw" alt="mockup.png"><div class="pin-layer"></div></div>`.
- Verify: integration test hits the route, checks status/headers/bytes.

### 3. Backend: anchor model — `src/state/types.rs`, `src/server/threads.rs`, `src/server/source.rs`

- Add `ImageAnchor` + optional field on `Thread` (see basis-points question above).
- `POST /api/threads`: accept optional `imageAnchor`; validate 0 ≤ pct ≤ 100 and that the target
  file is `kind: image` (and conversely, require `imageAnchor` for image files). Server assigns
  pin number (count of active threads on that file + 1) into `anchor_start`/`anchor_end`.
- `POST /api/source`: reject when `file_id` resolves to an image file.
- Verify: serde round-trip tests + handler validation tests, mirroring existing tests in
  `threads.rs`/`types.rs`.

### 4. Backend: events & transcript — `src/events.rs`, `src/transcript.rs`

- `thread.created` stdout/SSE payloads include `imageAnchor` and a context string like
  `"pin 3 at 42%,17%"` (set `Thread.breadcrumb` at creation so transcript/history get it free).
- Transcript: image threads sort by pin number (already works via `anchor_start`). Include
  `imageAnchor` in transcript thread entries.
- Verify: transcript test with one image thread.

### 5. Frontend — `discuss.html`

- **Render**: when the active file kind is `image`, skip the `data-anchor-idx` assignment pass for
  that container; wire the `.image-review` shell instead.
- **Pin drop**: click on the image → compute `xPct/yPct` from `event.offsetX / img.clientWidth`
  (percentage-based so zoom/resizes are safe) → show composer; submit via existing
  `apiJson('/api/threads', ...)` with `imageAnchor` + `fileId`, optimistic update + rollback per
  existing conventions.
- **Pin markers**: absolutely-positioned numbered badges in `.pin-layer` at
  `left: xPct%; top: yPct%`; re-render from `currentState` threads filtered by fileId; click pin →
  focus thread in the sidebar (reuse the existing thread-focus path used by `data-anchor-start`
  markers).
- **Thread list**: for image threads show label `📍 Pin 3` instead of `#start–end`; skip
  snippet/breadcrumb logic that assumes text anchors.
- **SSE**: `thread.created`/`deleted`/`resolved` handlers are already keyed by threadId — just
  make the pin layer re-render on state change.
- Verify: manual smoke `discuss mockup.png` and a mixed session `discuss plan.md mockup.png`
  (sidebar switching, pins survive reload via server state).

### 6. Skill/docs — `skills/discuss/SKILL.md`, `CHANGELOG.md`, `AGENTS.md`

- Document image sessions in the skill: the agent sees `imageAnchor` in events, can read the
  image path itself to view the pinned region, and replies via the same `/api/threads/{id}/takes`.
- Changelog entry under `## [Unreleased]`; add the new invariants (raw route, image anchor
  validation) to `AGENTS.md` Rust Patterns per repo convention.

### 7. Verification gate

`cargo fmt && cargo clippy --all-targets && cargo build --all-targets && cargo test` (CI order,
warnings denied), plus the manual browser smoke above.

**Rough size**: ~4 backend files meaningfully touched + `discuss.html`; the frontend pin layer is
the largest single chunk.
