# Live block editing — edit the doc without leaving discuss

**Status:** Plan for review, 2026-07-07.
**Branch:** `feat/live-block-editing` (worktree off `origin/main` @ `c7d3997`).
**Supersedes:** the A-vs-B fork in [document-editing.md](document-editing.md) — Keith picked in-app editing (Approach A), block level.

---

## TL;DR

Click a block, edit its **raw markdown** in place, and the save flows through the live-update machinery upstream landed this morning. No WYSIWYG in v1, so the HTML→markdown round-trip — the riskiest part of the June plan — is gone entirely. The server splices the edited block into the source by line range, re-anchors comments mechanically, broadcasts the existing `source.updated` event, and writes the file to disk atomically.

Roughly half of the June plan shipped upstream today without us. This plan is the other half.

---

## What changed since June

Upstream (`eda6f59`, Chris, 2026-07-07, closes #3) landed **live source updates with an agent-pushed re-anchor protocol**:

- `markdown_source` is now mutable shared state — `Arc<RwLock<Arc<str>>>` ([server.rs:94](../../src/server.rs))
- `POST /api/source` replaces the whole document, with strict re-anchor coverage: every active thread must be re-anchored or explicitly orphaned, else 409 and nothing changes
- `sourceVersion` counter in state; `POST /api/threads` rejects stale versions with `409 stale_source_version`
- `source.updated` broadcasts over SSE + stdout with `{markdown, renderedHtml, threadAnchors, orphanedThreadIds, sourceVersion}`
- The browser handler ([discuss.html:1645](../../discuss.html)) swaps `#doc-content` in place, re-runs anchor indexing / Prism / mermaid / minimap, and preserves every conversation; orphaned threads get a badge
- `Thread` gained an `orphaned` flag; `line_range` already exists on threads

That's June's Phase 1 (mutable source) and most of Phase 3 (re-anchoring + orphan handling) done. June's Q4 (deleted-block comments) is answered: they orphan with a badge.

**What doesn't exist yet** — the actual feature:

1. No block → source-line mapping (`render.rs` doesn't enable comrak sourcepos)
2. No edit UI — every click still means "comment"
3. No block-level write path — `POST /api/source` makes the *caller* do all the re-anchor math; fine for agents, wrong for a click-to-edit UI
4. No disk persistence — `POST /api/source` swaps memory only (the agent already owns the file in that flow; a user edit born in discuss has nowhere to live)

## What disco taught us

Not much code, two good ideas. The disco repo has **zero implemented** editing/watching/re-anchoring — its docs explicitly defer "block-level collaborative editing" to a future phase. But its plans consistently converge on: **stable block identity + last-write-wins at block granularity, no CRDT**. That's the model here too. (Disco also anchors by char offsets; discuss anchors by block index, which is friendlier — in-block edits don't move anchors at all.)

---

## The design

### One editor for every block: raw markdown

In edit mode, clicking a block swaps it for a textarea holding that block's **markdown source**. Commit re-renders it. Prose, code fences, tables, mermaid — same treatment.

Why this beats WYSIWYG for v1:

- **Kills the round-trip risk.** June's central worry was HTML→markdown degrading the file on every save. Editing the source directly means the only bytes that change are the ones you typed.
- **June already conceded structured blocks need raw-markdown boxes.** v1 just makes that the universal case.
- **The audience writes markdown all day.** discuss docs are written *for agents, by people who write markdown*. A `**bold**` in a textarea is not a hardship.
- **WYSIWYG stays open as a later layer** on the same plumbing (see Later, below).

### Server does the splice, not the client

New endpoint pair:

- `GET /api/blocks/{idx}` → `{markdown, sourceVersion}` — fetched when an editor opens, so it's always current
- `PATCH /api/blocks/{idx}` with `{markdown, sourceVersion}` — the write path

On PATCH the server: checks `sourceVersion` (stale → 409, nothing changes) → splices the new markdown into the block's source line range → re-renders → **re-anchors mechanically** → bumps version → broadcasts the existing `source.updated` → schedules the disk write.

Mechanical re-anchoring is simple because anchors are block indices:

- Edit stays one block → no anchor moves anywhere
- Edit renders as N blocks (typed a new paragraph) → downstream anchors shift by N−1
- Edit renders as zero blocks (deleted everything) → threads on it orphan via the existing machinery, downstream shift by −1

The alternative — client composes the full document and calls `POST /api/source` — would push all that anchor math into 4,000 lines of vanilla JS. The server owns the source; it should own the splice.

### Edit mode is a real mode

Today click-block and select-text both mean "comment" — editing needs its own gesture space (June decision, still right). A toolbar toggle plus `e` switches modes; review mode stays the default so `/discuss` behaves exactly as before until you opt in.

In edit mode: blocks show a hover affordance, click opens the textarea, **⌘Enter or blur commits, Esc cancels**. (⌘Enter matches the comment-composer convention from `feat/cmd-enter-to-send`.) Comment gestures are suspended while in edit mode.

**v1 requirement, not polish** (from review, u-1 — "we shouldn't break the nice formatting"): the textarea inherits the block's typography — font, size, line-height, padding — so editing a paragraph feels like editing it in place with the syntax visible, not dropping into a monospace box. The doc around the edited block stays fully rendered.

### Live means live — and safe

- Commit on blur/⌘Enter; **disk write debounced ~500ms** behind the in-memory swap (June's locked decision)
- **Atomic writes**: temp file + rename, so a crash can never truncate the doc
- **Conflict safety comes free** from `sourceVersion`: if an agent pushes `POST /api/source` while your editor is open, your PATCH 409s. The client keeps your draft, re-fetches the block, and shows both — never silently drops your text, never silently overwrites the agent's.
- **External-change guard**: before writing the file, compare it against the last content we read/wrote. If someone edited it in another app meanwhile, skip the write and show a banner instead of clobbering. (Real file-watching is future work.)
- **No file to write to?** Stdin docs (`source_path` is `None`) and `--no-save` runs edit in memory only, with a visible "edits not saved to file" badge. This answers June's Q5: `--no-save` means *touch nothing on disk*, editing included.

---

## Phases

Each phase lands green before the next starts; every phase has its own verify step.

### Phase 0 — block map + splice spike (no UI) — ✅ done

Built as [blocks.rs](../../src/blocks.rs). One better than planned: sourcepos lives on every comrak AST node regardless of render options, so the map is built from `parse_document` internally and the **served HTML stays byte-identical** — no render change at all.

The spike surfaced what the mapping really is: the browser anchors *outermost commentable elements* (`COMMENTABLE_SELECTOR`), not top-level blocks — each `li` is its own anchor, tables and `h6` get none, footnote definitions anchor at the end in reference order (comrak conveniently reorders the AST the same way), and the front-matter `<details>` owns anchor 1. Raw HTML blocks are the one unpredictable case: shapes we can prove (known template classes, single commentable elements, comments) map normally; anything else marks the map unreliable from that point via `unsafe_from`, and splices there are refused.

- Frontmatter offset handled (sourcepos is body-relative; map converts to file coordinates)
- Trailing blank lines comrak absorbs into a block (last list item, indented code) are trimmed from ranges so an editor never shows them
- `splice()` replaces an inclusive line range byte-exactly, with insertion and deletion semantics

**Verified:** 32 unit tests including identity-splice byte-equality over a kitchen-sink fixture, plus a one-off empirical parity check — the map reproduced the exact anchor indices (74–78) a real browser assigned during this plan's own review session.

### Phase 1 — block endpoints (server)

`GET /api/blocks/{idx}` and `PATCH /api/blocks/{idx}` as designed above, reusing the `source.updated` broadcast and orphan machinery. Memory only — no disk writes yet.

**Verify:** integration tests in `tests/server.rs` — happy path (PATCH → SSE + stdout event + state + re-rendered root page), stale-version 409, block-split shifts downstream anchors, block-delete orphans its thread, out-of-range idx rejected. Plus a curl demo against a live server.

### Phase 2 — edit mode UI (frontend)

Toggle + `e` shortcut, textarea-in-block editor, ⌘Enter/blur/Esc, conflict handling with draft preservation, comment gestures suspended in edit mode. Also: the `source.updated` handler currently drops in-flight comment selections — it must **preserve an open editor's draft**, re-locating the block or surfacing the conflict.

**Verify:** manual script — edit a paragraph, refresh, it persisted in state; two tabs open, edit in one, other updates live; comment on a block, edit that block, comment survives; agent pushes `POST /api/source` mid-edit via curl, draft survives with conflict notice.

### Phase 3 — disk persistence

Debounced atomic write to `source_path` after each applied edit; external-change guard; stdin/`--no-save` memory-only badge.

**Verify:** tests for write-through, debounce coalescing, guard-triggered skip, stdin no-write; manual — edit, `cat` the file, see the change; edit the file externally then edit in discuss, see the banner not a clobber.

### Ship

Update `skills/discuss/SKILL.md` (agents must expect `sourceVersion` to move from user edits) and `CHANGELOG.md`. Then one feature PR upstream (per review, u-5), with commits sequenced by phase so it reviews cleanly.

---

## Later (explicitly not v1)

- **WYSIWYG prose editing** — same plumbing, gated on its own HTML→markdown spike (June Phase 0)
- **Structural ops** — add block between blocks, delete-block button, drag to reorder
- **Agent propose/accept** — agent suggests an edit, you approve (June's locked "user first, agent later")
- **File watching** — external edits re-render live (Approach B's core, and disco's someday-phase-2)

---

## Decisions from review (2026-07-07, via /discuss)

1. **Raw-markdown block editor for v1.** Keith's concern: "we shouldn't break the nice formatting." Answer: raw markdown is what *protects* the file's formatting; the edit-time cost is addressed by making the typography-matched textarea a **v1 requirement** (see Edit mode above). WYSIWYG stays a later layer.
2. **Server-side splice** via `PATCH /api/blocks/{idx}` — resolved in session ("make best decision" → recommendation adopted).
3. **`--no-save` / stdin → memory-only editing with a badge** — delegated ("no idea"), recommendation adopted. Save-as can come later if anyone hits it.
4. **External-change guard: skip + banner** — resolved in session ("Guard it is.").
5. **One feature PR** with everything ("put it all together"), commits sequenced by phase (spike → endpoints → UI → persistence) so it reviews cleanly. No split PRs, no upstream issue first.
