# Discuss CLI — Demo Recording Script

This is the deterministic source for `docs/demo.mp4` and `docs/demo.gif`. The recording is generated from the real bundled `discuss demo` process; screenshots, mockups, or hand-edited substitutes are not acceptable.

## Reproduce the recording

Prerequisites: Rust, Node/npm, Python 3, Chrome or Chromium, [VHS](https://github.com/charmbracelet/vhs), ffmpeg, and gifski.

```sh
make -f demo/Makefile demo
```

The pipeline performs these steps:

1. The Makefile builds `target/release/discuss`, runs `npm ci`, and installs Playwright's reproducible Chromium/ffmpeg tools. The recorder also honors `CHROME_BIN` and uses an installed macOS Google Chrome when available.
2. `demo/record.mjs` starts a fresh real `discuss demo` process for each browser scene and records it with Playwright at 1280×800.
3. `demo/scene01.tape` and `demo/scene10.tape` record the terminal bookends with VHS.
4. `demo/captions.mjs` renders caption overlays.
5. `demo/stitch.sh` normalizes and joins the clips, writes H.264 `docs/demo.mp4`, and derives `docs/demo.gif` with gifski.
6. `demo/stitch.sh` fails if the GIF exceeds the 3.5 MiB embedded-asset budget mirrored by `src/server/demo.rs` tests.

`discuss demo` itself does **not** need Node, Chrome, VHS, ffmpeg, gifski, GitHub authentication, a model, an app runtime, or a public network connection. Those tools are only used to regenerate the checked-in recording.

## Deterministic scenes

| # | Screen | Scripted action | Acceptance evidence |
|---|---|---|---|
| 1 | Terminal | Run the actual release binary with `discuss demo`; parse its real `session.started` event. | One launch reports Feature tour, Example PR, and Local app loopback URLs. |
| 2 | Feature tour | Open `plan.md`, then open a seeded Demo-agent thread. | Existing six-file generic tour and canned agent remain functional. |
| 3 | Example PR | Open the synthetic PR, show the expanded nested file tree, select `src/payments/retry.ts`, and open imported thread `gh-review-thread-900003`. | Real PR overview/file/thread contracts are visible, with synthetic identities clearly labelled. |
| 4 | Example PR | Open `docs/operations/retry-runbook.md`, select **Viewed**, and let the UI advance. | Eye indicator and next-unviewed progression run through `/api/pr/files/{id}/viewed`. |
| 5 | Example PR | Open **Finish review**, include the local Demo-agent reply, and edit the deterministic summary. | Action, summary, destination, include/exclude, and text controls are editable. |
| 6 | Example PR | Preview the saved draft, show **Exact GitHub-flavored Markdown**, then press **OK — Simulate locally**. | The exact GFM view is recorded; the success banner states that nothing was sent to GitHub. The real publisher is not reachable from demo execution mode. |
| 7 | Local app | Open the bundled Ledgerly app in the live-review iframe. | Production proxy injection is used; root-relative CSS/JS and `/api/dashboard` render with no separate process. |
| 8 | Local app | Turn Inspect off, navigate to Payments with `pushState`, then exercise browser history for `popstate`. | Discuss shows `/payments` and `/` as the current live route while the iframe stays on its distinct proxy origin. |
| 9 | Local app | Return to `/payments`, enable Inspect, click the rollout card, save an element comment, and wait for the Demo-agent take. | Route-scoped element anchor, in-frame marker, thread, and canned response are all visible. |
| 10 | Terminal | Read the real `session.done` captured from scene 6. | Terminal states no `gh`, LLM, GitHub publication, network dependency, or history write occurred. |

## Recording checks

Before committing regenerated assets:

```sh
ffprobe -v error -show_entries format=duration:stream=codec_name,width,height \
  -of default=noprint_wrappers=1 docs/demo.mp4
ffprobe -v error -show_entries format=duration:stream=codec_name,width,height \
  -of default=noprint_wrappers=1 docs/demo.gif
wc -c docs/demo.gif docs/demo.mp4
cargo test embedded_demo_binaries_fit_the_asset_size_budget
```

Play both files from beginning to end. Confirm captions are readable at the README's 960 px GIF width and that scenes 3–6 and 7–9 visibly demonstrate the PR and local-app workflows rather than describing them in prose.
