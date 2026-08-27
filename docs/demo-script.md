# Discuss CLI — Demo Video Script

This script defines the new demo recording for `docs/demo.gif` and `docs/demo.mp4`. The current `docs/demo.gif` is stale. The new recording shows every major feature at version 0.9.0. It has a tight main cut of about 80 seconds for the README GIF. Optional extended scenes make a longer full video. Follow the scenes in order. Read the captions aloud only in your head — they render as on-screen text.

---

## Recording setup

### Approach

Use a scripted, repeatable pipeline. Do not record by hand.

| Part | Tool | Notes |
|---|---|---|
| Terminal scenes | VHS (charmbracelet) `.tape` file | Deterministic typed commands |
| Browser scenes | Playwright with `recordVideo` | Inject a synthetic cursor. Drive all clicks by script |
| Server | `discuss --port 5757 --no-open --no-save <fixtures>` | Stable URL, no browser auto-launch, no history writes |
| Join + encode | ffmpeg | Concat clips, output H.264 MP4 at ~30 fps |
| GIF | gifski | fps 12, width 960, quality ~80 |

Every retake is one command: `make demo`.

**Future versions:** consider [Remotion](https://remotion.dev) for a better design aesthetic. Remotion composes the Playwright clips as React components. It gives coded captions, transitions, and rendered terminal scenes. It replaces VHS and the ffmpeg concat step. Keep it in a `demo/` folder with its own `package.json`. Check the Remotion company license first. Decision (2026-08-27): v1 uses the pipeline above; Remotion is deferred.

### Window and viewport

- Browser viewport: 1280×800, `deviceScaleFactor: 2`.
- Terminal: 120×30 cells, dark theme, 18 pt monospace font.
- Theme: start in light mode. One scene flips to dark mode.

### Port

- Use `--port 5757` for the main session.
- Use `--port 5758` if a second concurrent session is on screen.

### Fixture files to prepare

| Fixture | Path | Contents |
|---|---|---|
| Plan doc | `docs/demo-fixtures/plan.md` | YAML frontmatter, two H2 sections, a `rust` fence, a `mermaid` flowchart, a table |
| Second doc | `docs/demo-fixtures/notes.md` | Short markdown, two paragraphs |
| Third doc | `docs/demo-fixtures/todo.md` | Short markdown list |
| Image mockup | `docs/demo-fixtures/mockup.png` | App UI mockup, at least 1200 px wide |
| Prototype | `examples/prototype.html` | Already in the repo |
| Git repo | `docs/demo-fixtures/demo-repo/` | Init a repo. Make two commits. Copy `plan.md` into the repo root. Edit `src/lib.rs`. Run `git add -A` so changes are staged |
| Verdict spec | shell variable | `'approve:Approve:positive|revise:Revise:neutral!|decline:Decline:negative!'` |

Prepare an agent session (Claude Code or pi) with the `/discuss` skill installed. Pre-write the agent prompt so typing is short.

### Post-processing commands

```sh
# 1. Concatenate scene clips.
ffmpeg -f concat -safe 0 -i scenes.txt -c copy raw.mp4

# 2. Encode the MP4 deliverable.
ffmpeg -i raw.mp4 -c:v libx264 -pix_fmt yuv420p -r 30 -crf 22 docs/demo.mp4

# 3. Extract frames for the GIF (main cut only).
ffmpeg -i main-cut.mp4 -vf fps=12,scale=960:-1 frames/frame%04d.png

# 4. Build the GIF.
gifski --fps 12 --width 960 --quality 80 -o docs/demo.gif frames/frame*.png
```

Keep `docs/demo.gif` at or under 3 MB and 30–90 seconds. Link `docs/demo.mp4` in the README as the full video.

---

## Scenes

Legend: **GIF** = in the README GIF main cut. **EXT** = full video only.

### Main cut (~80 s)

| # | Cut | Time | Screen | Commands / actions | Caption |
|---|---|---|---|---|---|
| 1 | GIF | 8 s | Terminal (agent session) | Type: `discuss ./plan.md with me`. Agent runs `discuss docs/demo-fixtures/plan.md`. The `session.started` JSON line prints. | "Ask your agent to discuss a doc." |
| 2 | GIF | 10 s | Browser | Click a paragraph in `plan.md`. Type: `This claim needs a source.` Click **Comment**. Wait 2 s. An agent take appears in the thread, styled as a take. | "Click a paragraph. Drop a comment. The agent replies in the margin." |
| 3 | GIF | 8 s | Terminal, then browser | Run: `discuss docs/demo-fixtures/plan.md docs/demo-fixtures/notes.md docs/demo-fixtures/todo.md`. Browser shows the file sidebar. Click `notes.md`, comment on a paragraph. The sidebar badge count changes. Click back to `plan.md`. | "Review many files in one session. Badges track open threads." |
| 4 | GIF | 10 s | Terminal, then browser | In `demo-repo/`, run: `discuss diff`. Browser shows highlighted hunks. Select three `+` lines in a `diff-rust` hunk. Type: `Handle the None case here.` Click **Comment**. A gutter bar marks the lines. | "Run `discuss diff`. Comment on exact lines of the staged diff." |
| 5 | GIF | 7 s | Browser | Run: `discuss docs/demo-fixtures/mockup.png`. Click the header area of the mockup. Pin 1 appears. Type: `Logo is too small.` Click **Comment**. | "Review images. Pins anchor threads to a spot." |
| 6 | GIF | 9 s | Browser | Run: `discuss examples/prototype.html`. Inspect mode is on. Hover a button — an outline follows the cursor. Click the button. Type: `Make this the primary action.` Click **Comment**. A numbered marker appears in the frame. | "Review HTML prototypes. Click an element to anchor a thread." |
| 7 | GIF | 8 s | Terminal, then browser | Agent rewrites `plan.md` and POSTs three `kind:"agent"` threads. Open the browser. Threads `a-1`, `a-2`, `a-3` are already on the doc with pending-take styling. Click `a-1`. | "The agent pre-annotates its own edits. Open the doc to a guided review." |
| 8 | GIF | 8 s | Browser | Agent POSTs new markdown to `/api/source`. The document swaps in place. Existing threads stay attached to their content. One thread shows the dashed "orphaned" badge. | "The doc updates live. Threads survive the rewrite." |
| 9 | GIF | 8 s | Browser | Session started with `--verdict-options 'approve:Approve:positive\|revise:Revise:neutral!\|decline:Decline:negative!'`. Click **Finish review**. The verdict modal opens. Click **Decline**. Validation asks for feedback. Type: `Ship after the risks section is fixed.` Submit. | "End with a verdict. Feedback is required when you decline." |
| 10 | GIF | 6 s | Terminal | The `session.done` JSON line prints. It contains the `verdict` object and the full transcript. The agent summarizes the decision. | "The agent gets the transcript and the verdict. Same terminal session." |

Main cut total: ~82 s. Trim scene tails to hit 80 s or less.

### Extended scenes (full video only)

| # | Cut | Time | Screen | Commands / actions | Caption |
|---|---|---|---|---|---|
| 11 | EXT | 15 s | Browser | In `plan.md`: expand the **Front Matter** block and comment on a field. Show the `rust` fence highlighted. Show the mermaid flowchart as SVG. Comment on a table cell. Collapse an H2 section — thread markers stack on the summary. Hover the left edge — the heading minimap expands. Flip the theme toggle to dark mode. | "Frontmatter, code, mermaid, tables, minimap, dark mode. All threadable." |
| 12 | EXT | 8 s | Terminal | Run: `cat notes.md \| discuss -`. Browser opens with `source_file: "<stdin>"`. Then run bare `discuss` interactively — it prints help and exits 2. | "Pipe markdown from anywhere. No file needed." |
| 13 | EXT | 8 s | Terminal, then browser | Run: `discuss plan.md diff HEAD~1..HEAD` inside `demo-repo/`. The range needs the two fixture commits. The sidebar shows the plan doc and the changed files with `+N −M` stats. | "Mix docs and diffs in one session." |
| 14 | EXT | 8 s | Browser | In the multi-file session: click **›** three times. The view steps through threads across files. Open the "X of Y resolved" popover. Click an entry — the view jumps to that thread. Resolve it. | "Walk every thread with ‹ and ›. Jump from the summary popover." |
| 15 | EXT | 7 s | Browser + terminal | Reload the image session — pins persist. Show one `thread.created` NDJSON line with `imageAnchor` in the terminal. | "Every event is one JSON line on stdout. Agents read it directly." |
| 16 | EXT | 7 s | Terminal | Run: `discuss update --check`. Output shows "up to date". Show the "Discuss v0.9.0" badge in the browser header. | "One binary. No cloud. Update when you choose." |

Full video total: ~135 s.

---

## Shot checklist

Preparation:

- [ ] Build the fixtures in `docs/demo-fixtures/`.
- [ ] Init `demo-repo/`, make two commits, copy `plan.md` into the repo root, edit `src/lib.rs`, run `git add -A`.
- [ ] Install the `/discuss` skill in the agent session.
- [ ] Set the terminal to 120×30, dark theme, 18 pt font.
- [ ] Set the Playwright viewport to 1280×800, scale factor 2.
- [ ] Start each session with `--port 5757 --no-open --no-save`.

Recording:

- [ ] Scene 1: agent launch, `session.started` line visible.
- [ ] Scene 2: paragraph thread, agent take appears.
- [ ] Scene 3: three files, sidebar badge changes.
- [ ] Scene 4: `discuss diff`, line-anchored comment, gutter bar.
- [ ] Scene 5: image pin 1 dropped and commented.
- [ ] Scene 6: Inspect mode hover, element thread, in-frame marker.
- [ ] Scene 7: `a-1`..`a-3` pre-annotations visible on load.
- [ ] Scene 8: live source swap, one orphaned badge visible.
- [ ] Scene 9: verdict modal, Decline, required feedback error, submit.
- [ ] Scene 10: `session.done` with `verdict` object in the terminal.
- [ ] Scenes 11–16: record for the full video only.

Post-processing:

- [ ] Concat clips with ffmpeg.
- [ ] Encode `docs/demo.mp4` (H.264, 30 fps).
- [ ] Build `docs/demo.gif` with gifski from scenes 1–10 only.
- [ ] Check the GIF is ≤ 3 MB and ≤ 90 s.
- [ ] Play both files. Confirm every caption is readable at 960 px width.
