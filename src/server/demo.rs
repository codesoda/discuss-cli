//! Offline demo mode: bundled example files, pre-seeded agent takes, and a
//! deterministic canned responder.
//!
//! Everything here is demo-only. Normal review sessions never reference this
//! module: only `run_demo_session` in `src/lib.rs` calls `demo_source`,
//! `seed_demo_threads`, or `spawn_demo_responder`, so real sessions keep their
//! exact stdout/SSE semantics.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use axum::Router;
use axum::http::{StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::get;
use chrono::Utc;
use tokio::net::TcpListener;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use crate::blocks::markdown_blocks;
use crate::pr::{GithubPrUrl, PrImportBundle};
use crate::sse::BroadcastEvent;
use crate::state::{File, FileId, FileKind, Source, Take, Thread, ThreadId, ThreadKind};
use crate::{DiscussError, Result};

use super::app_state::AppState;

/// Every take the demo writes (seeded openers and canned responses) starts
/// with this prefix so it is clearly identifiable as the Demo agent. No
/// schema change: the identity lives in the text.
pub const DEMO_AGENT_PREFIX: &str = "Demo agent — ";

/// `session.started` mode / source_file label for demo sessions.
pub const DEMO_MODE: &str = "demo";
pub const DEMO_SOURCE_LABEL: &str = "demo";

/// Delay before a canned response lands, tuned to feel like a human replied.
pub const DEMO_RESPONSE_DELAY: Duration = Duration::from_millis(1500);

// The feature-tour GIF is embedded straight from the README asset so the two
// can never drift apart (decision D11 in docs/plans/demo-mode.md). That
// couples the binary to a re-recordable artifact, so `demo/stitch.sh` enforces
// DEMO_GIF_MAX_BYTES at record time and the test below re-checks it here.
const DEMO_GIF: &[u8] = include_bytes!("../../docs/demo.gif");

/// Ceiling for the embedded feature-tour GIF, mirrored by the `gifski` step in
/// `demo/stitch.sh`. Re-recording the demo must not silently add megabytes to
/// every released binary: if a re-record trips this, re-tune the `gifski`
/// quality/width in `demo/stitch.sh` rather than raising the cap.
#[cfg(test)]
const DEMO_GIF_MAX_BYTES: usize = 3 * 1024 * 1024 + 512 * 1024;
const DEMO_PLAN_MD: &str = include_str!("../../assets/demo/plan.md");
const DEMO_NOTES_MD: &str = include_str!("../../assets/demo/notes.md");
const DEMO_RETRY_DIFF: &str = include_str!("../../assets/demo/retry.diff");
const DEMO_MOCKUP_PNG: &[u8] = include_bytes!("../../assets/demo/mockup.png");
const DEMO_PROTOTYPE_HTML: &str = include_str!("../../assets/demo/prototype.html");
const DEMO_PR_IMPORT_JSON: &[u8] = include_bytes!("../../assets/demo/example-pr.json");
const DEMO_LOCAL_APP_HTML: &str = include_str!("../../assets/demo/local-app.html");
const DEMO_LOCAL_APP_CSS: &str = include_str!("../../assets/demo/local-app.css");
const DEMO_LOCAL_APP_JS: &str = include_str!("../../assets/demo/local-app.js");

pub const DEMO_PR_URL: &str = "https://github.com/demo-only/ledgerly/pull/56";
pub const DEMO_PR_IMPORTED_THREAD_ID: &str = "gh-review-thread-900003";
pub const DEMO_PR_LOCAL_TAKE: &str = "Demo agent — I tied this sleep to the shared 30-second deadline and added a test for the complete retry window. This local take is excluded from the simulated GitHub reply until you choose Include in Finish Review.";
pub(super) const DEMO_PR_SUMMARY: &str = "Demo review: the bounded retry window and coverage look ready for a staged rollout. Confirm the 20% provider-capacity reserve during Stage 2 monitoring.";

/// One pre-seeded agent thread anchored to a deliberately revised passage.
/// Anchors are 1-based block indices into `markdown_blocks(content)`; the
/// snippet/breadcrumb are pulled from the same walk at seed time so the prose
/// and the callout cannot drift apart. Tests pin each anchor to a distinctive
/// phrase from the revision it annotates.
struct DemoSeed {
    file_id: &'static str,
    content: &'static str,
    anchor: usize,
    /// The seeded opening take: explains what changed and asks the reviewer
    /// to confirm it.
    opening: &'static str,
    /// Tailored canned response to the reviewer's first reply on this thread.
    followup: &'static str,
}

const DEMO_SEEDS: [DemoSeed; 4] = [
    // Revision A (plan.md): retry attempts 3 -> 5 plus a 30 s total ceiling.
    DemoSeed {
        file_id: "f-2",
        content: DEMO_PLAN_MD,
        anchor: 8,
        opening: "Demo agent — I raised the attempt cap from 3 to 5 and added a 30-second ceiling on the whole retry window after replaying the March brownout: with three attempts, 11% of recoverable charges were still failing when the budget ran out. Can you confirm the 30 s ceiling fits the provider's 50 rps budget at peak checkout volume? Reply here, or press Resolve if it looks right.",
        followup: "Demo agent — Thanks for confirming. With full jitter, five attempts inside the 30 s ceiling keeps worst-case retry load under 10 rps per merchant — comfortably inside the reserved budget. I can tighten the wording in the Design section if you'd like; otherwise feel free to Resolve.",
    },
    // Revision B (plan.md): rollout table gains Stage 0 shadow mode and the
    // Stage 2 exit criterion tightens 0.5% -> 0.3%.
    DemoSeed {
        file_id: "f-2",
        content: DEMO_PLAN_MD,
        anchor: 12,
        opening: "Demo agent — Two edits to the rollout table: a new Stage 0 shadow mode that computes and logs retries without sending them, and a tighter Stage 2 exit criterion (failure rate under 0.3%, down from 0.5%). Is 0.3% achievable for the 5% cohort before we widen the rollout, or should Stage 2 keep the old threshold?",
        followup: "Demo agent — That matches what I saw in the replay: the 5% cohort landed at 0.28% after the fix, so 0.3% is tight but achievable. I'll keep Stage 2 as written unless you object — Resolve when you're happy with the table.",
    },
    // Revision C (notes.md): recovery estimate 84% -> 79% after excluding
    // 429-rate-limited attempts from the replay.
    DemoSeed {
        file_id: "f-3",
        content: DEMO_NOTES_MD,
        anchor: 2,
        opening: "Demo agent — I revised the recovery estimate from 84% down to 79%: the replay now excludes attempts that were rejected with 429 rate-limit responses, since retrying those would have been throttled again rather than recovered. Does the business case for the retry pipeline still hold at 79%?",
        followup: "Demo agent — Agreed. Even at 79%, the recovered volume covers the pipeline's operating cost within the first month, so the business case holds. I'll keep the conservative figure in the notes. Resolve when you're satisfied.",
    },
    // Revision D (notes.md): new paragraph reserving a 20% retry budget of
    // the 50 rps provider limit.
    DemoSeed {
        file_id: "f-3",
        content: DEMO_NOTES_MD,
        anchor: 4,
        opening: "Demo agent — This paragraph is new: it reserves at most 20% of the 50 rps provider limit for retries so first attempts always keep the other 80%. Can you confirm the 80/20 split with the merchant-traffic team before rollout, or suggest a different reserve?",
        followup: "Demo agent — Will do — I'll get the 80/20 split confirmed with the merchant-traffic team and note the outcome here. If they push back we can lower the retry reserve. Resolve if that plan works for you.",
    },
];

// Canned responses to threads the reviewer opens, keyed on the file kind of
// the annotated file. Each nudges the user toward the reply/resolve
// affordances the demo exists to show off.
const OPENER_MARKDOWN: &str = "Demo agent — Good catch — that passage is worth pinning down. In a real session your agent would fold this into the next revision of the document. Reply if you want to add detail, or press Resolve when you're satisfied.";
const OPENER_DIFF: &str = "Demo agent — Thanks — noted against this hunk. If you think it risks a regression, say so in a reply and I'd rework the change; otherwise Resolve closes it out.";
const OPENER_IMAGE: &str = "Demo agent — I see the spot you pinned. In a real session your agent would adjust the design there and post an updated screenshot. Reply with more direction, or Resolve if flagging it was enough.";
const OPENER_HTML: &str = "Demo agent — Got it — I've noted the element you selected. In a real session your agent would edit the prototype and let you re-review it. Reply to refine the request, or Resolve to close the thread.";

const FOLLOWUP_GENERIC: &str = "Demo agent — Understood. I've noted your follow-up and would fold it into the next pass. Anything else on this spot, or is it ready to Resolve?";
const CLOSER_GENERIC: &str =
    "Demo agent — Noted — I've logged that too. Resolve this thread whenever you're satisfied.";

/// Builds the six bundled demo files (GIF first, so it is selected on load)
/// plus the raw-bytes map for the two images. Paths are deliberately bare
/// filenames: with no directory component, the HTML asset route's
/// parent-canonicalize step can never resolve against the process CWD, so
/// `/files/{id}/assets/*` 404s deterministically (decision D15).
pub fn demo_source() -> (Source, HashMap<FileId, (Vec<u8>, &'static str)>) {
    let files = vec![
        File {
            id: FileId("f-1".to_string()),
            path: "demo.gif".to_string(),
            kind: FileKind::Image,
            content: String::new(),
        },
        File {
            id: FileId("f-2".to_string()),
            path: "plan.md".to_string(),
            kind: FileKind::Markdown,
            content: DEMO_PLAN_MD.to_string(),
        },
        File {
            id: FileId("f-3".to_string()),
            path: "notes.md".to_string(),
            kind: FileKind::Markdown,
            content: DEMO_NOTES_MD.to_string(),
        },
        File {
            id: FileId("f-4".to_string()),
            path: "retry.diff".to_string(),
            kind: FileKind::Diff,
            content: DEMO_RETRY_DIFF.to_string(),
        },
        File {
            id: FileId("f-5".to_string()),
            path: "mockup.png".to_string(),
            kind: FileKind::Image,
            content: String::new(),
        },
        File {
            id: FileId("f-6".to_string()),
            path: "prototype.html".to_string(),
            kind: FileKind::Html,
            content: DEMO_PROTOTYPE_HTML.to_string(),
        },
    ];

    let mut file_bytes = HashMap::new();
    file_bytes.insert(FileId("f-1".to_string()), (DEMO_GIF.to_vec(), "image/gif"));
    file_bytes.insert(
        FileId("f-5".to_string()),
        (DEMO_MOCKUP_PNG.to_vec(), "image/png"),
    );

    (Source { files }, file_bytes)
}

/// Returns and validates the deterministic synthetic PR import used by the
/// Example PR scenario. The fixture is decoded locally and never reaches the
/// real loader or an authenticated GitHub command.
pub fn demo_pr_import() -> Result<(GithubPrUrl, Vec<u8>)> {
    let identity = GithubPrUrl::parse(DEMO_PR_URL)?;
    let bundle: PrImportBundle =
        serde_json::from_slice(DEMO_PR_IMPORT_JSON).map_err(|error| DiscussError::ConfigError {
            message: format!("bundled demo PR fixture is invalid JSON: {error}"),
        })?;
    bundle.validate(&identity)?;
    Ok((identity, DEMO_PR_IMPORT_JSON.to_vec()))
}

/// Serves the bundled mini app entirely in process. Root-relative assets and
/// the app's `/api/dashboard` route deliberately share this upstream origin;
/// every other GET is the SPA document so pushState routes survive reloads.
pub async fn serve_demo_app_listener(
    listener: TcpListener,
    mut shutdown: watch::Receiver<bool>,
) -> Result<()> {
    let listening_addr = listener
        .local_addr()
        .map_err(|source| DiscussError::ServerBindError {
            addr: std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, 0)),
            source,
        })?;
    if listening_addr.ip() != std::net::Ipv4Addr::LOCALHOST {
        return Err(DiscussError::ServerBindError {
            addr: listening_addr,
            source: std::io::Error::new(
                std::io::ErrorKind::AddrNotAvailable,
                "discuss only binds to 127.0.0.1",
            ),
        });
    }

    let app = Router::new()
        .route("/demo-app.css", get(demo_app_css))
        .route("/demo-app.js", get(demo_app_js))
        .route("/api/dashboard", get(demo_app_dashboard))
        .fallback(get(demo_app_page));
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            while shutdown.changed().await.is_ok() {
                if *shutdown.borrow() {
                    break;
                }
            }
        })
        .await
        .map_err(|source| DiscussError::ServerBindError {
            addr: listening_addr,
            source,
        })
}

async fn demo_app_page() -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            ("content-type", "text/html; charset=utf-8"),
            ("cache-control", "no-store"),
            ("x-frame-options", "DENY"),
            (
                "content-security-policy",
                "default-src 'self'; frame-ancestors 'none'",
            ),
        ],
        DEMO_LOCAL_APP_HTML,
    )
}

async fn demo_app_css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        DEMO_LOCAL_APP_CSS,
    )
}

async fn demo_app_js() -> impl IntoResponse {
    (
        [(
            header::CONTENT_TYPE,
            "application/javascript; charset=utf-8",
        )],
        DEMO_LOCAL_APP_JS,
    )
}

async fn demo_app_dashboard() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "local API connected",
        "recoveryRate": "79%",
        "retryBudget": "10 req/s",
    }))
}

/// Seeds one agent-authored thread per `DEMO_SEEDS` entry, each with its
/// opening take, under one write lock and through the shared ID allocators so
/// later runtime allocation cannot collide. No events are emitted: this runs
/// before the server starts, and the state reaches the browser through the
/// initial snapshot exactly like restored state.
///
/// Returns the allocated thread ids in `DEMO_SEEDS` order. The responder keys
/// its tailored follow-ups off this vector rather than off literal `a-N`
/// strings, so the pairing survives any change to allocation order.
pub fn seed_demo_threads(app_state: &AppState) -> Vec<ThreadId> {
    let mut seeded = Vec::with_capacity(DEMO_SEEDS.len());
    let Ok(mut state) = app_state.state.write() else {
        return seeded;
    };
    let created_at = Utc::now();
    for seed in &DEMO_SEEDS {
        let blocks = markdown_blocks(seed.content);
        let block = &blocks[seed.anchor - 1];
        let thread_id = app_state.next_agent_thread_id();
        state.add_thread(Thread {
            id: thread_id.clone(),
            file_id: FileId(seed.file_id.to_string()),
            anchor_start: seed.anchor,
            anchor_end: seed.anchor,
            image_anchor: None,
            snippet: block.snippet.clone(),
            breadcrumb: block.breadcrumb.clone(),
            text: String::new(),
            created_at,
            kind: ThreadKind::Agent,
            line_range: None,
            orphaned: false,
            element_anchor: None,
        });
        state.add_take(Take {
            id: app_state.next_take_id(),
            thread_id: thread_id.clone(),
            text: seed.opening.to_string(),
            created_at,
        });
        seeded.push(thread_id);
    }

    seeded
}

/// Spawns the demo responder: a single background task that reacts to
/// user-created threads and user replies with one canned take each, after a
/// human-feeling delay. Responses are normal `Take` records published as
/// SSE-only `take.added` events (never stdout), exactly like
/// `POST /api/threads/{id}/takes`.
///
/// `seeded` is the `seed_demo_threads` return value, in `DEMO_SEEDS` order.
pub fn spawn_demo_responder(app_state: AppState, seeded: Vec<ThreadId>, delay: Duration) {
    let mut shutdown = app_state.subscribe_shutdown();
    let mut events = app_state.bus.subscribe();

    tokio::spawn(async move {
        // Belt-and-braces dedupe: broadcast lag drops events (never replays
        // them), so this guards the responder's own bookkeeping.
        let mut handled: HashSet<String> = HashSet::new();
        loop {
            let event = tokio::select! {
                biased;

                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                    continue;
                }
                event = events.recv() => match event {
                    Ok(event) => event,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                },
            };

            let Some((key, thread_id)) = responder_trigger(&event) else {
                continue;
            };
            if !handled.insert(key) {
                continue;
            }

            // The delay races shutdown so a Done during the pause cancels the
            // pending take. Triggers arriving meanwhile queue in the
            // broadcast buffer and are answered sequentially afterwards.
            tokio::select! {
                biased;

                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = tokio::time::sleep(delay) => {}
            }

            respond_to_thread(&app_state, &seeded, &thread_id);
        }
    });
}

/// Maps a broadcast event to a (dedupe key, thread to answer) pair. Only
/// user-created threads and replies trigger responses; the responder never
/// reacts to agent threads or to `take.added` (its own output), so it cannot
/// respond to itself or loop.
fn responder_trigger(event: &BroadcastEvent) -> Option<(String, ThreadId)> {
    match event.kind.as_str() {
        "thread.created" => {
            if event.payload.get("kind").and_then(|kind| kind.as_str()) != Some("user") {
                return None;
            }
            let id = event.payload.get("id").and_then(|id| id.as_str())?;
            Some((format!("T:{id}"), ThreadId(id.to_string())))
        }
        "reply.added" => {
            let reply_id = event.payload.get("id").and_then(|id| id.as_str())?;
            let thread_id = event
                .payload
                .get("threadId")
                .and_then(|thread_id| thread_id.as_str())?;
            Some((format!("R:{reply_id}"), ThreadId(thread_id.to_string())))
        }
        _ => None,
    }
}

/// Adds the canned take, re-validating under the write lock so a resolve,
/// delete, or shutdown that raced the delay wins and suppresses the response.
fn respond_to_thread(app_state: &AppState, seeded: &[ThreadId], thread_id: &ThreadId) {
    let take = {
        let Ok(mut state) = app_state.state.write() else {
            return;
        };
        // Checked under the state write lock (not before acquiring it) so a
        // Done that raced the responder's delay is observed before the take
        // is added rather than in the gap between check and write.
        //
        // `shutdown` alone is not enough: `post_api_done` builds and emits the
        // transcript before signalling shutdown, so a take added inside that
        // window would reach the browser over SSE while being absent from the
        // already-emitted transcript. `done_started` is latched before the
        // transcript read lock, which closes exactly that window.
        if app_state.done_started() || app_state.shutdown.is_signaled() {
            return;
        }
        // get_threads() excludes soft-deleted threads.
        let Some(thread) = state
            .get_threads()
            .into_iter()
            .find(|thread| &thread.id == thread_id)
        else {
            return;
        };
        if state.resolution_for_thread(thread_id).is_some() {
            return;
        }
        let kind = app_state
            .file_kind(&thread.file_id)
            .unwrap_or(FileKind::Markdown);
        let demo_take_count = state
            .takes_for_thread(thread_id)
            .iter()
            .filter(|take| take.text.starts_with(DEMO_AGENT_PREFIX))
            .count();
        let text = canned_response(seed_index(seeded, thread_id), kind, demo_take_count);
        state.add_take(Take {
            id: app_state.next_take_id(),
            thread_id: thread_id.clone(),
            text,
            created_at: Utc::now(),
        })
    };
    app_state.record_mutation();

    let Ok(payload) = serde_json::to_value(&take) else {
        return;
    };
    app_state.bus.publish(BroadcastEvent {
        kind: "take.added".to_string(),
        payload,
    });
}

/// Position of `thread_id` in the seeded thread list, i.e. its index into
/// `DEMO_SEEDS`. `None` for every thread the reviewer opened.
pub fn seed_index(seeded: &[ThreadId], thread_id: &ThreadId) -> Option<usize> {
    seeded.iter().position(|seeded_id| seeded_id == thread_id)
}

/// Deterministic canned response: a pure function of the seed index (seeded
/// threads get tailored follow-ups; `None` for reviewer-opened threads), the
/// annotated file's kind, and how many Demo-agent takes the thread already has.
pub fn canned_response(
    seed_index: Option<usize>,
    kind: FileKind,
    demo_take_count: usize,
) -> String {
    let text = if let Some(index) = seed_index {
        // The seeded opener is already the thread's first Demo-agent take.
        if demo_take_count <= 1 {
            DEMO_SEEDS[index].followup
        } else {
            CLOSER_GENERIC
        }
    } else {
        match demo_take_count {
            0 => match kind {
                FileKind::Markdown => OPENER_MARKDOWN,
                FileKind::Diff => OPENER_DIFF,
                FileKind::Image => OPENER_IMAGE,
                FileKind::Html => OPENER_HTML,
            },
            1 => FOLLOWUP_GENERIC,
            _ => CLOSER_GENERIC,
        }
    };
    text.to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn demo_source_returns_six_files_in_fixed_order_with_gif_first() {
        let (source, file_bytes) = demo_source();

        let summary: Vec<(&str, &str, FileKind)> = source
            .files
            .iter()
            .map(|file| (file.id.0.as_str(), file.path.as_str(), file.kind))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("f-1", "demo.gif", FileKind::Image),
                ("f-2", "plan.md", FileKind::Markdown),
                ("f-3", "notes.md", FileKind::Markdown),
                ("f-4", "retry.diff", FileKind::Diff),
                ("f-5", "mockup.png", FileKind::Image),
                ("f-6", "prototype.html", FileKind::Html),
            ]
        );

        for file in &source.files {
            if file.kind == FileKind::Image {
                assert!(file.content.is_empty(), "{} keeps content empty", file.path);
            } else {
                assert!(!file.content.is_empty(), "{} has content", file.path);
            }
        }

        assert_eq!(file_bytes.len(), 2);
        let (gif, gif_mime) = &file_bytes[&FileId("f-1".to_string())];
        assert_eq!(*gif_mime, "image/gif");
        assert!(gif.starts_with(b"GIF8"));
        let (png, png_mime) = &file_bytes[&FileId("f-5".to_string())];
        assert_eq!(*png_mime, "image/png");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']));
    }

    #[test]
    fn embedded_demo_binaries_fit_the_asset_size_budget() {
        // Re-recording the demo regenerates docs/demo.gif, which every release
        // binary embeds. demo/stitch.sh enforces the same GIF cap at record
        // time so a re-record fails there, with an actionable message, instead
        // of here.
        assert!(
            DEMO_GIF.len() <= DEMO_GIF_MAX_BYTES,
            "docs/demo.gif is {} B, over the {DEMO_GIF_MAX_BYTES} B embed cap; \
             re-tune the gifski quality/width in demo/stitch.sh",
            DEMO_GIF.len()
        );
        assert!(DEMO_GIF.len() + DEMO_MOCKUP_PNG.len() < 4 * 1024 * 1024);
    }

    #[test]
    fn demo_paths_have_no_directory_component() {
        // Bare filenames make Path::parent() the empty path, which never
        // canonicalizes, so /files/{id}/assets/* can never read the CWD.
        let (source, _) = demo_source();
        for file in &source.files {
            assert_eq!(
                Path::new(&file.path).parent(),
                Some(Path::new("")),
                "{} must be a bare filename",
                file.path
            );
        }
    }

    #[test]
    fn demo_prototype_html_is_self_contained() {
        assert!(!DEMO_PROTOTYPE_HTML.contains("src="));
        assert!(!DEMO_PROTOTYPE_HTML.contains("href="));
        assert!(DEMO_PROTOTYPE_HTML.contains("<style>"));
    }

    #[test]
    fn synthetic_pr_fixture_uses_real_validated_contracts() {
        let (identity, bytes) = demo_pr_import().expect("demo PR fixture should validate");
        assert_eq!(identity.canonical_url(), DEMO_PR_URL);
        let bundle: PrImportBundle = serde_json::from_slice(&bytes).expect("fixture JSON");
        assert_eq!(bundle.files.len(), 4);
        assert!(bundle.files.iter().any(|file| file.binary));
        assert!(
            bundle
                .files
                .iter()
                .any(|file| file.status == crate::pr::DiffStatus::Renamed)
        );
        assert_eq!(bundle.discussions.len(), 3);
        assert_eq!(bundle.seed_threads.len(), 1);
        assert!(bundle.overview_markdown.contains("Synthetic demo data"));
        assert!(bundle.overview_markdown.contains("Issue comment"));
        assert!(bundle.overview_markdown.contains("Review summary"));
        assert!(bundle.overview_markdown.contains("Inline review thread"));
    }

    #[test]
    fn bundled_local_app_exercises_live_app_contracts() {
        assert!(DEMO_LOCAL_APP_HTML.contains("href=\"/demo-app.css\""));
        assert!(DEMO_LOCAL_APP_HTML.contains("src=\"/demo-app.js\""));
        assert!(DEMO_LOCAL_APP_JS.contains("fetch('/api/dashboard')"));
        assert!(DEMO_LOCAL_APP_JS.contains("history.pushState"));
        assert!(DEMO_LOCAL_APP_JS.contains("popstate"));
        assert!(DEMO_LOCAL_APP_HTML.contains("id=\"deploy-card\""));
    }

    #[test]
    fn seed_anchors_match_the_revised_passages() {
        // Pins each seed's anchor to a distinctive phrase from the revision
        // it annotates, so the prose and the callout cannot drift apart.
        // notes.md has a single h1, so both of its seeds sit directly under it.
        let expectations = [
            ("RetryBudget::attempts(5)", "Design"),
            ("Shadow mode (log only)", "Rollout"),
            ("79% of failed charges", "Provider Brownout Notes"),
            ("20% of the 50 rps limit", "Provider Brownout Notes"),
        ];
        for (seed, (snippet_phrase, breadcrumb_suffix)) in DEMO_SEEDS.iter().zip(expectations) {
            // An empty suffix would make ends_with vacuously true and pin
            // nothing; every seed must name a real heading.
            assert!(!breadcrumb_suffix.is_empty());
            let blocks = markdown_blocks(seed.content);
            let block = &blocks[seed.anchor - 1];
            assert!(
                block.snippet.contains(snippet_phrase),
                "anchor {} snippet {:?} should contain {snippet_phrase:?}",
                seed.anchor,
                block.snippet
            );
            assert!(
                block.breadcrumb.ends_with(breadcrumb_suffix),
                "anchor {} breadcrumb {:?} should end with {breadcrumb_suffix:?}",
                seed.anchor,
                block.breadcrumb
            );
        }
    }

    #[test]
    fn seed_copy_is_agent_prefixed_and_describes_the_revision() {
        for seed in &DEMO_SEEDS {
            assert!(seed.opening.starts_with(DEMO_AGENT_PREFIX));
            assert!(seed.followup.starts_with(DEMO_AGENT_PREFIX));
            assert!(seed.opening.contains('?'), "opening asks for confirmation");
        }
        // Openings reference the exact changed values.
        assert!(DEMO_SEEDS[0].opening.contains("3 to 5"));
        assert!(DEMO_SEEDS[0].opening.contains("30"));
        assert!(DEMO_SEEDS[1].opening.contains("0.3%"));
        assert!(DEMO_SEEDS[2].opening.contains("84%"));
        assert!(DEMO_SEEDS[2].opening.contains("79%"));
        assert!(DEMO_SEEDS[3].opening.contains("20%"));
    }

    #[test]
    fn seed_demo_threads_seeds_four_agent_threads_with_takes() {
        let app_state = AppState::for_process();
        let seeded = seed_demo_threads(&app_state);

        let state = app_state.state.read().expect("state lock");
        let threads = state.get_threads();
        assert_eq!(threads.len(), 4);
        // The returned ids are the seeded threads, in DEMO_SEEDS order.
        assert_eq!(seeded.len(), DEMO_SEEDS.len());
        for (index, thread) in threads.iter().enumerate() {
            assert_eq!(seeded[index], thread.id);
            assert_eq!(seed_index(&seeded, &thread.id), Some(index));
            assert_eq!(thread.id.0, format!("a-{}", index + 1));
            assert_eq!(thread.kind, ThreadKind::Agent);
            assert!(thread.text.is_empty());
            assert!(!thread.snippet.is_empty());
            let takes = state.takes_for_thread(&thread.id);
            assert_eq!(takes.len(), 1);
            assert_eq!(takes[0].id, format!("t-{}", index + 1));
            assert!(takes[0].text.starts_with(DEMO_AGENT_PREFIX));
        }
        drop(state);

        // Later runtime allocation continues after the seeds.
        assert_eq!(app_state.next_agent_thread_id().0, "a-5");
        assert_eq!(app_state.next_take_id(), "t-5");
    }

    #[test]
    fn canned_responses_are_deterministic_prefixed_english() {
        let kinds = [
            FileKind::Markdown,
            FileKind::Diff,
            FileKind::Image,
            FileKind::Html,
        ];
        for kind in kinds {
            for depth in 0..4 {
                let first = canned_response(None, kind, depth);
                let second = canned_response(None, kind, depth);
                assert_eq!(first, second, "identical inputs give identical strings");
                assert!(first.starts_with(DEMO_AGENT_PREFIX));
                assert!(first.len() > DEMO_AGENT_PREFIX.len() + 20);
                assert!(!first.to_ascii_lowercase().contains("lorem"));
            }
        }
        // Openers are kind-specific.
        assert_ne!(
            canned_response(None, FileKind::Markdown, 0),
            canned_response(None, FileKind::Diff, 0)
        );
        // Seeded threads get their tailored follow-up first, then the closer.
        assert_eq!(
            canned_response(Some(0), FileKind::Markdown, 1),
            DEMO_SEEDS[0].followup
        );
        assert_eq!(
            canned_response(Some(0), FileKind::Markdown, 2),
            CLOSER_GENERIC
        );
        assert_eq!(
            canned_response(Some(2), FileKind::Markdown, 1),
            DEMO_SEEDS[2].followup
        );
    }

    #[test]
    fn seed_index_pairs_threads_with_seeds_without_hardcoded_ids() {
        let app_state = AppState::for_process();
        let seeded = seed_demo_threads(&app_state);

        for (index, thread_id) in seeded.iter().enumerate() {
            assert_eq!(seed_index(&seeded, thread_id), Some(index));
        }
        assert_eq!(seed_index(&seeded, &ThreadId("u-1".to_string())), None);
        // An agent thread allocated after the seeds is not a seeded thread.
        assert_eq!(seed_index(&seeded, &app_state.next_agent_thread_id()), None);
    }

    #[test]
    fn responder_suppresses_takes_once_done_has_started() {
        // post_api_done latches done_started before building the transcript and
        // only signals shutdown after emitting it, so the responder must honor
        // the flag on its own: shutdown is still unsignalled in this window.
        let app_state = AppState::for_process();
        let seeded = seed_demo_threads(&app_state);
        app_state.begin_done();
        assert!(!app_state.shutdown.is_signaled());

        respond_to_thread(&app_state, &seeded, &seeded[0]);

        let state = app_state.state.read().expect("state lock");
        assert_eq!(
            state.takes_for_thread(&seeded[0]).len(),
            1,
            "only the seeded opener; no take may land after Done starts"
        );
    }
}
