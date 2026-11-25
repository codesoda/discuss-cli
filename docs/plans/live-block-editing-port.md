# Bring back the Edit button: port live block editing onto main

**Status:** Plan for review, 2026-10-21.
**Repo:** discuss-cli (`codesoda/discuss-cli`). Work happens in a new worktree off `main`, not in the main checkout.
**What this is:** The edit mode you had in July lives on a stalled branch. This plan ports it onto today's main and gets it back into your installed `discuss`.

---

## What happened

- Edit mode was built in July on `feat/live-block-editing` ([PR 26](https://github.com/codesoda/discuss-cli/pull/26)). Click a block, edit its raw markdown in place, save splices it back into the file.
- The only installed build that ever had it was 0.5.0 (late July). Every build since came from main, which never got the branch.
- Main moved 81 commits since. The server file was split into a `src/server/` folder, a different `src/blocks.rs` landed for the agent blocks endpoint, and `discuss.html` grew from about 4,900 lines to 7,700 (PR review mode, file sidebar, prefs). The branch no longer merges: conflicts in the HTML, events, tests, changelog, a name clash on `blocks.rs`, and the old `server.rs` is gone.

## The fix, in one line

Don't rebase the old commits. Start a fresh branch off main and re-apply the feature piece by piece, using the old branch as the source of truth.

Why not rebase: five commits, each conflicting against a restructured server and a doubled HTML file. Resolving the same conflicts five times is slower and riskier than one clean port.

## What stays the same (decisions already made in July)

- Raw markdown textarea per block. No WYSIWYG.
- Server does the splice (`GET` / `PATCH /api/blocks/{idx}`), not the client.
- Edit is a real mode: toolbar toggle plus `e`. Review mode stays the default.
- Save on ⌘Enter or blur, Esc cancels. Disk write debounced about 500ms, atomic temp-file-and-rename.
- Stale `sourceVersion` → 409, keep the user's draft, show both.
- External change guard: if the file changed on disk, skip the write and show a banner.
- `--no-save` and stdin docs: edit in memory only, with a badge.
- Diff and PR files are read-only. Edit clicks fall through to commenting.

## What is different now (things the port has to account for)

1. **`src/blocks.rs` name clash.** Main's `blocks.rs` is the agent-facing block list (issue 39). The branch's 989-line block map + splice module comes in as `src/block_map.rs` instead.
2. **Server is split into modules.** The two handlers and the save-behind logic go in a new `src/server/block_edit.rs`. The per-file "last written content" state goes into `app_state.rs`. Routes register in `mod.rs`.
3. **Anchors are now stamped by the server** (issue 43, `render.rs`). The old block map reverse-engineered the browser's walk. Now `render.rs` is the single authority, so the map should be checked against it, and may get simpler.
4. **New event kinds exist** (PR imported, summary, publish). `source.save_failed` slots in alongside them.
5. **HTML has PR review mode and a file sidebar.** Edit mode must be disabled on PR diff files and must not fight the viewed-files sidebar. The branch's 12 HTML hunks get re-applied by hand against the current file.
6. **Prefs live on the server now** (PR 60). Small win: remember whether edit mode was on across launches. Optional, see questions.

## Phases

Each phase lands green before the next starts.

### Phase 0: set up

- New worktree off latest main: `discuss-cli-worktrees/feat-live-block-editing-v2`, branch `feat/live-block-editing-v2`.
- Run `branch-start`: file an issue "Bring back live block editing" linking PR 26.
- Verify: `cargo test` green on a clean main before touching anything.

### Phase 1: block map + splice (backend, no UI)

- Copy the branch's `src/blocks.rs` in as `src/block_map.rs`. Same comrak version on both sides (0.52.0), so it should compile with little change.
- Check the map against `render.rs`'s server-stamped anchors on a kitchen-sink doc. Fix any drift.
- Keep its 32 unit tests.
- Verify: `cargo test`, plus the identity-splice check (splice a block back unchanged → file is byte-identical).

### Phase 2: block endpoints + save-behind (backend)

- `src/server/block_edit.rs`: `GET /api/blocks/{idx}` and `PATCH /api/blocks/{idx}`, both taking `fileId`.
- PATCH: check `sourceVersion` → splice → re-render → re-anchor threads → bump version → broadcast `source.updated` → schedule debounced atomic disk write.
- External change guard and `source.save_failed` event.
- Verify: integration tests ported from the branch's `tests/server.rs` (happy path, stale 409, block split shifts anchors, block delete orphans its thread, out-of-range idx, uneditable diff file, no-save memory only). Plus a curl run against a live server.

### Phase 3: edit mode UI (frontend)

- Toolbar Edit toggle plus `e` shortcut.
- Click a block in edit mode → textarea with the block's markdown, matching the block's font, size, and padding.
- ⌘Enter or blur saves, Esc cancels.
- Comment gestures off while editing. Selection popup suppressed.
- `source.updated` handler keeps an open editor's draft, or shows the conflict.
- Draft stash survives re-renders and sidebar file switches.
- Edit disabled on PR diff files, with the click falling through to comment.
- "Edits not saved to file" badge for stdin / `--no-save`.
- Verify: manual script. Edit a paragraph, refresh, it stuck. Two tabs, edit in one, other updates live. Comment on a block, edit it, comment survives. Push `POST /api/source` via curl mid-edit, draft survives with a notice. Edit the file in another app, then edit in discuss, see the banner not a clobber.

### Phase 4: ship

- `CHANGELOG.md` entry. `skills/discuss/SKILL.md`: agents must expect `sourceVersion` to move from user edits.
- Build release binary in the worktree. Back up `~/.discuss/bin/discuss` to `discuss-0.11.1.bak2`, copy the new one in.
- Re-point `~/.claude/skills/discuss` at the new worktree's `skills/discuss` (it currently points at the `fix-idle-ping-page-open` worktree).
- Open a new PR to `codesoda/discuss-cli`. Close PR 26 with a note pointing at it.

## Size

About 2,800 lines on the old branch: 1,000 block map, 500 server, 370 HTML, 640 tests, rest docs. Most of it moves across with renames. The hand work is the HTML hunks and the server module split. Call it a half day, with Phase 3 the slow part.

## Questions

1. **Remember edit mode across launches** via the new server-side prefs? Small add. Default: yes.
2. **Branch name and PR.** Fresh `feat/live-block-editing-v2` plus new PR, closing 26. Or force-push over the old branch so PR 26 stays the PR. Default: fresh branch, cleaner history.
3. Anything you remember hating about the July version that should change while it's open?
