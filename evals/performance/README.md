# Version 1 — current review baseline

This suite measures the **real rendered review page** and its SSE updates with a deterministic scripted agent (no model). `fixtures/script.json` fixes the question, response, scenario sizes, warmup (2) and measured trials (20). Small/large documents and seeded user threads exercise different DOM/state sizes. Keep this version's fixtures, options and protocol unchanged across before/after runs; create a new version for changed workloads.

## Run

Requires Node 20+, Chromium for Playwright and a locally built discuss executable. From this directory:

```sh
npm ci
DISCUSS_BIN=/absolute/path/to/discuss PERF_PROFILE=my-workstation npm run baseline
node compare.mjs results/before.json results/after.json
npm test
DISCUSS_BIN=/absolute/path/to/discuss npm run delayed
```

Alternatively set `PLAYWRIGHT_MODULE=/tmp/discuss-browser-tools/node_modules/playwright/index.mjs` to use an existing installation. `DISCUSS_BIN` must be the **actual binary under test**; the suite never substitutes a mock. Output defaults to `results/baseline-<timestamp>.json` (ignored by Git); set `PERF_OUTPUT` to override. Keep per-machine raw artifacts for audit; do not commit them by default. `PERF_PROFILE` is an operator-supplied machine label. Compare rejects mismatched fixture hashes, scenarios, options, sample sizes, CPU/OS/Node/browser/profile. It reports absolute and percentage arithmetic-median changes (percentage is `n/a` when the before median is zero), not cross-host speed claims. The saved `p50` is the nearest-rank 50th-percentile sample, whereas `median` averages the middle two samples for even counts; comparisons use `median`, so the two may differ.

The child uses `--no-open --no-save`, with an isolated temporary HOME/XDG/cwd, ephemeral loopback port, and is terminated after each scenario. The browser blocks non-loopback requests (including update checks and CDN assets) rather than timing external network; some remote syntax highlighting will therefore not load. Browser context/child are cleaned up on failure. Requests on loopback are permitted. For reproducibility use the same binary build mode and browser installation on both sides; metadata includes binary SHA-256, git HEAD, dirty-tree flag and dirty diff/status hash, fixture hashes, OS/CPU, Node/browser version, flags, counts, and profile. A dirty checkout is *reported*, not described as a clean release. The working tree is not modified outside this directory.

Each trial submits a visible thread follow-up in the browser; after the matching stdout `reply.added` is received the scripted agent POSTs the fixed response to `/api/threads/{id}/takes`. The page is already hydrated with its EventSource **open** before trials. A MutationObserver observes the actual inserted take and `requestAnimationFrame` supplies the next frame opportunity. `submitToStdoutMs` uses the harness monotonic `hrtime` before the Playwright click and at stdout receipt: **approximate cross-boundary latency**, including automation transport. `postToInsertionMs` and `postToFrameMs` use only browser `performance.now`, starting just before browser fetch POST. `roundTripMs` uses only harness `hrtime` from before click to observer/frame result receipt (includes Playwright transport). No clocks are subtracted across processes. The first two trials are discarded. Raw milliseconds, nearest-rank p50/p95 and arithmetic median are saved per metric; there are **no performance pass/fail thresholds**. An optional thinking-status API/indicator is not in the core workload so older binaries without that endpoint remain comparable; it is not measured or claimed here.

## Captured local baseline

Release binary SHA-256 `edc7d79fcd585ec25a860468f81211e5b7c05a8f667f430366ed6020091cd9ba`, HEAD `9b55150534edb5bf619331f97886cd38bd858003` **dirty**; local Ryzen 5 7640HS, Linux x64, Chromium 159.0.8154.6, Node v24.20.0. Each scenario: 2 warmup + 20 measured trials. Medians (ms):

| Scenario | Submit → stdout | POST → DOM | POST → frame | Round trip |
| --- | ---: | ---: | ---: | ---: |
| small | 33.789 | 12.900 | 27.850 | 62.097 |
| large | 61.305 | 13.500 | 19.100 | 97.226 |

These are one machine-specific observation, **not** a threshold or clean-tree claim. Raw samples and p50/p95: `results/baseline-1795644217091.json` on the originating machine (ignored by Git).

## Version 3 — deterministic delayed responses (revised overhead methodology)

`npm run delayed` runs `fixtures/delayed.json` against the same small document, with one warmup and 20 measured **batches per profile**. Two questions target distinct visible threads in separate browser tabs at fixed offsets 0 and 500 ms. Each scripted response sleeps 5,000 ms after its matching stdout receipt. Concurrent handling runs both timers simultaneously; intentionally serial handling queues the second *after* its question is received. The one-question five-second profile is a reference. Questions/responses are fixed, with a deterministic profile/batch/event suffix on each response for exact routing and duplicate detection. The script verifies the reply text in the correct thread in `/api/state` and the DOM, checks absence from other threads, and checks thinking → replying → idle indicator lifecycle when the activity endpoint exists. Unsupported activity on older binaries without an advertised activity endpoint is recorded, not silently treated as a pass; an advertised endpoint returning 404/405 or a structured missing-thread response fails the probe. Any correctness failure aborts the run. Non-loopback network is blocked as in version 1.

`DELAY_FIXTURE=/absolute/path/to/longer.json` selects a custom JSON workload with the same schema, permitting longer fixed waits; keep its file and hash identical for before/after. `DELAY_PROFILES=fiveSecond,overlapConcurrent,overlapSerial` selects profiles (default: all); `PERF_OUTPUT` and `PERF_PROFILE` also work. Comparison checks version, fixture hashes, full schedules including offsets, delays and serial/concurrent handling, profile selection, sample counts, environment and flags before reporting differences. A v1 zero-delay result is **not** compared to v3. Version-2 delayed artifacts are incompatible with v3: the overhead calculation changed and new queue/indicator metrics were added; `compare.mjs` rejects cross-version comparisons. Do not overwrite the version-1 script to add delays.

Per-question `submissionToVisibleMs` is harness monotonic time from just before browser follow-up submission to DOM-observer receipt; `observedWaitMs` is stdout receipt → response POST start (includes queued time, indicator requests and the deliberate timer); `actualTimerMs` is just the actual sleep interval. `queueWaitMs` measures stdout receipt → start of the response handler (serial scheduling); `indicatorMs` measures the thinking/replying activity POSTs and browser indicator waits before the take POST (idle occurs after visibility and is excluded). `overheadMs = submissionToVisibleMs - actualTimerMs - queueWaitMs` includes indicator cost and is an **approximate** browser/harness/transport remainder, not a pure Discuss CPU measurement. `batchMakespanMs` is batch schedule start → last visible response (excludes subsequent correctness queries); `scheduleDriftMs` records actual submit start minus intended offset, and may be negative. No browser clock is subtracted from a harness clock. Raw samples include nearest-rank p50/p95, arithmetic median and counts (for even sample counts, p50 and median may differ); comparisons use the median, without gating thresholds. Synthetic 5-second sleep is **not measured model speed**.

**Historical v2 baseline only — obsolete for v3 metrics.** No compatible v3 before/after baseline has been collected; do not use the numbers below as a v3 comparison or fabricate replacement results. Captured dirty-tree local release baseline: `results/delayed-1795647344776.json` (gitignored), SHA-256 `edc7d79fcd585ec25a860468f81211e5b7c05a8f667f430366ed6020091cd9ba`, HEAD `9b55150534edb5bf619331f97886cd38bd858003`, Ryzen 5 7640HS/Linux x64/Chromium 159.0.8154.6. Medians in milliseconds:

| Historical v2 profile | Submission → visible | Observed wait | Actual timer | Old v2 overhead (obsolete) | Batch makespan |
| --- | ---: | ---: | ---: | ---: | ---: |
| 5 s single | 5059.025 | 5026.725 | 5000.671 | 32.307 | 5061.053 |
| 5 s overlap/concurrent | 5060.098 | 5026.606 | 5000.258 | 32.643 | 5559.961 |
| 5 s overlap/serial | 7316.327 | 7282.888 | 5000.763 | 36.923 | 10075.717 |

Correctness: 0 missing, duplicate, misrouted or indicator-lifecycle failures; activity supported. Each overlap profile checked 42 responses including warmup (40 measured). See the raw file for all samples and p50/p95. These numbers demonstrate the configured wait/queue schedule, not a model/provider benchmark. An application-millisecond reduction is **not** an equivalent full-user speedup. A future real-model end-to-end evaluation would need controlled model/provider, prompt, reasoning and token budgets, cache policy, tool turns and network conditions.

## Future profiling

For CPU attribution, build a separate Cargo profiling binary with debuginfo and frame pointers, record the exact Cargo/RUSTFLAGS profile and tool invocation beside the run, then profile startup/seed, DOM rendering and thread mutation separately. Do not compare profiling-build timings directly to optimized release timings. No profiler is run by this suite.
