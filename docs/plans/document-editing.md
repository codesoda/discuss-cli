# Document editing in discuss — design exploration

**Status:** On hold as of 2026-06-18. Explored via a `/discuss` review of the original plan; paused before any implementation.
**Repo / target:** discuss-cli (this repo) — the tool behind `/discuss`.
**How to read this:** Two competing approaches, the decisions we locked in, the questions still open, and the architecture notes worth keeping for whoever picks this up.

---

## TL;DR

Goal: give discuss a writing experience, not just a read-and-comment one — so you can edit the document with the AI sidebar still beside you.

Two ways to get there:

- **Approach A — build an editor into discuss.** Inline WYSIWYG: click a block in the rendered doc, edit in place, convert back to markdown on save.
- **Approach B — keep your existing editor.** You write in Obsidian / VS Code / iA, and discuss becomes a comments companion that watches the file and floats beside it.

The key realisation from the review: **Approach B deletes the riskiest part of Approach A** (building an editor + the HTML→markdown round-trip). Both approaches share a **file-watching + re-anchoring core**. We parked it before choosing A vs B.

---

## Approach A — inline WYSIWYG inside discuss

Click a block, edit in place, see formatting live, save converts the block back to markdown.

- **Edit / Review mode toggle.** Needed because every click and drag in the doc today already means "start a comment." Writing needs its own gesture space.
- **The central risk: the HTML→markdown round-trip.** discuss exists to produce clean markdown an agent can read. Naive HTML→markdown is lossy (reflows `*`/`_`, list markers, tables). If every save rewrites the whole file, the doc degrades each edit.
- **The fix: only re-serialise the block you edited.** Turn on Comrak source positions so each rendered block knows its exact line span in the source. Edit one paragraph → convert just that paragraph → splice it into its line range → every untouched block stays byte-for-byte identical.
- **Structured blocks fall back to raw markdown.** Code fences, tables, mermaid are miserable to WYSIWYG-edit and worst to round-trip. Those get a small raw-markdown box; prose gets true in-place editing.

### Phasing sketch (Approach A)

- **Phase 0 — De-risk the round-trip (spike, no UI).** Confirm Comrak `render.sourcepos` works under `default-features = false`. Prototype render→edit→convert→splice→re-render. → Verify: a real doc survives an in-block edit with **zero diff outside the edited block**.
- **Phase 1 — Mutable source + document API (backend).** Make `markdown_source` mutable; add `PATCH /api/document`, debounced disk write, SSE + stdout `document.changed`. → Verify: curl edits a block, file updates, second tab sees it.
- **Phase 2 — Inline edit UI + toggle (frontend).** Editable prose blocks, raw-markdown fallback for structured blocks. → Verify: edit a paragraph, it persists, refresh shows it.
- **Phase 3 — Re-anchor comments across edits.** Recompute block indices, snippet-match threads, orphans detach with an "anchor lost" state. → Verify: in-block edit keeps the comment; deleting a commented block detaches gracefully.
- **Phase 4 — Structural edits + formatting toolbar (stretch).**
- **Phase 5 — Agent-proposed edits.** Propose/accept layer (see decisions below).

---

## Approach B — companion to your existing editor

You edit in whatever markdown app you already love; discuss watches the file and shows the rendered doc + AI comments in a floating native window beside it.

- **Biggest win: no editor to build, no HTML→markdown round-trip.** Approach A's #1 risk evaporates. Users get a far better editor than discuss would ever ship. discuss focuses on the one thing it's uniquely good at — AI comments anchored to the doc.
- **Editing and the file are the same thing.** The editor owns the file; no write-back endpoint, no atomic-write concerns on discuss's side.

### Costs, honestly

- **Needs file-watching** (external edit → re-render → re-anchor comments by snippet). But Approach A needed that same re-anchoring anyway, so it's shared cost — and it's exactly the **disco Phase 2** groundwork.
- **The floating native window is a new delivery surface.** discuss is a browser app today; browsers can't easily do always-on-top docked panels. Implies either a thin native wrapper (Tauri / Swift) hosting the existing web UI, or just parking a normal browser window beside the editor yourself.
- **Editing and commenting split across two apps.** You edit in Obsidian but still comment on discuss's rendered copy — you can't select-to-comment inside Obsidian. Two best-in-class surfaces, but a context switch.

### The sharp version

The valuable, low-risk core is **file-watching + re-anchoring**. It works with *any* editor and needs zero native code — you just put discuss next to your editor. The floating native window is optional polish on top. That core might make the WYSIWYG editor unnecessary, or leave it as a "quick tweak without leaving the tab" nicety.

---

## What A and B share

- **Re-anchoring comments after the document changes.** Threads anchor to block index, so structural edits shift them; both approaches need snippet-matching with graceful orphan detach.
- **A mutable view of the document** (Approach A mutates source from the browser; Approach B reloads it from disk on change).
- **Direct overlap with disco's planned Phase 2** (file-watching + snippet re-anchoring). Whatever we build here is groundwork for the private disco fork, not throwaway.

---

## Decisions locked in during review

1. **Save timing → live.** Commit on **block-blur, debounced ~500ms** (not per-keystroke) — keeps the real-editor feel while sidestepping half-finished saves. Pair with **atomic writes** (temp file + rename) so a crash can't truncate the `.md`. (Approach A.)
2. **Agent editing → user first, agent later.** The user-editing loop must be solid before the agent touches the doc. Note: the agent can already hit any localhost endpoint, so the agent-edit phase isn't about the endpoint — it's a **propose/accept layer** (agent suggests an edit, you approve; never silent overwrite). Slotted as Phase 5.

---

## Open questions (mostly Approach A)

- **Q3 — HTML→markdown: client or server?** Turndown on the client (keeps it where the DOM is, but the first JS dep in a deliberately vanilla file) vs a Rust html2md crate (one source of truth, testable in Rust). Leaning client-side; not decided.
- **Q4 — Deleted-block comments.** Detach and keep the thread visible ("anchor lost"), or auto-resolve it?
- **Q5 — `--no-save` interplay.** That flag disables history archiving today. Should it also suppress disk writes from editing (edit-in-memory-only, hand the result back on Done)?
- **The big fork (A vs B).** Is Approach B a *replacement* for the WYSIWYG editor, or a *parallel track*? And for v1, is "file-watching + arrange your own windows" enough, or is the native floating window the actual point?

---

## Architecture notes worth keeping (from reading the code)

- `markdown_source` in `src/server.rs` is `Arc<str>`, read once at launch and never mutated — making it mutable shared state is the core backend change for Approach A.
- `GET /` renders the source via Comrak (`src/render.rs`) and injects it into `#doc-content` (`src/template.rs`). The frontend only ever sees rendered HTML, never the markdown source — which is why WYSIWYG needs an HTML→markdown step.
- Comrak `render.sourcepos` is the enabler for surgical per-block splicing (maps each block to its source line range).
- **Anchors are `data-anchor-idx`** — the 1-based position of a block element within `#doc-content` (`assignAnchorIndices()` in `discuss.html`), *not* character offsets. So in-block text edits are anchor-safe; only add/remove/reorder of blocks shifts downstream anchors. (Note: the older `disco-handoff.md` claims char-offset anchoring — that's outdated; the code uses block indices.)
- The current interaction model: **select text → popup → comment**, and **click a block → comment**. Both gestures already mean "comment," which is why Approach A needs an explicit edit mode.
- `discuss.html` is hand-written vanilla with zero JS dependencies — adding Turndown (Q3) would be the first.

---

## Pointers for picking this back up

- `src/server.rs` — `with_markdown_source`, `get_root`/`render_root_page`, the route table, SSE at `/api/events`.
- `src/state/types.rs` — `Thread { anchor_start, anchor_end, snippet, ... }` (block indices).
- `discuss.html` — `assignAnchorIndices()`, `selectionRange()`, the click-dispatch handler (interaction model).
- `/Users/keithlang/Documents/GitHub/disco/` — the private P2P fork; its planned Phase 2 (file-watching + snippet re-anchoring) is the shared groundwork.
- First decision on resume: **A vs B.** If B, the native floating window is the main new cost. If A, the Phase 0 round-trip spike gates everything.
