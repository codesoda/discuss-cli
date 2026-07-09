use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::Path;
use axum::extract::State as AxumState;
use axum::extract::rejection::JsonRejection;
use axum::http::Request;
use axum::http::StatusCode;
use axum::http::header;
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::watch;
use tokio::time::MissedTickBehavior;
use tower_http::trace::TraceLayer;

use crate::assets;
use crate::events::{Event, EventEmitter, EventKind};
use crate::history;
use crate::sse::{BroadcastEvent, EventBus};
use crate::state::{
    Draft, File, FileId, FileKind, LineRange, NewThreadDraftKey, Reply, Resolution, SharedState,
    Source, State, Take, Thread, ThreadId, ThreadKind, default_file_id,
};
use crate::transcript::build_transcript_with_source;
use crate::verdict::{Verdict, VerdictConfig};
use crate::{Config, DiscussError, Result, render, template};

const JAVASCRIPT_CONTENT_TYPE: &str = "application/javascript";
const ASSET_CACHE_CONTROL: &str = "public, max-age=86400";
const SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const MAX_IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(10);
const MIN_IDLE_CHECK_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SourceUpdateRequest {
    markdown: String,
    #[serde(default)]
    file_id: Option<FileId>,
    thread_anchors: Vec<ThreadAnchorUpdate>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ThreadAnchorUpdate {
    thread_id: ThreadId,
    #[serde(default)]
    anchor_start: Option<usize>,
    #[serde(default)]
    anchor_end: Option<usize>,
    #[serde(default)]
    snippet: Option<String>,
    #[serde(default)]
    line_range: Option<LineRange>,
    #[serde(default)]
    orphaned: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceUpdatedPayload {
    markdown: String,
    file_id: FileId,
    rendered_html: String,
    thread_anchors: Vec<ThreadAnchorResponse>,
    orphaned_thread_ids: Vec<ThreadId>,
    source_version: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ThreadAnchorResponse {
    thread_id: ThreadId,
    anchor_start: usize,
    anchor_end: usize,
    orphaned: bool,
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub state: SharedState,
    pub bus: Arc<EventBus>,
    pub emitter: Arc<EventEmitter<Box<dyn Write + Send>>>,
    source: Arc<std::sync::RwLock<Source>>,
    source_path: Arc<Option<PathBuf>>,
    history_dir: Arc<PathBuf>,
    no_save: Arc<AtomicBool>,
    shutdown: ShutdownSignal,
    activity: ActivityTracker,
    idle_timeout_secs: Arc<AtomicU64>,
    verdict_config: Arc<Option<VerdictConfig>>,
    next_thread_number: Arc<AtomicU64>,
    next_reply_number: Arc<AtomicU64>,
    next_take_number: Arc<AtomicU64>,
}

impl AppState {
    pub fn new(
        state: SharedState,
        bus: Arc<EventBus>,
        emitter: Arc<EventEmitter<Box<dyn Write + Send>>>,
    ) -> Self {
        Self {
            state,
            bus,
            emitter,
            source: Arc::new(std::sync::RwLock::new(Source::default())),
            source_path: Arc::new(None),
            history_dir: Arc::new(history::default_history_dir()),
            no_save: Arc::new(AtomicBool::new(false)),
            shutdown: ShutdownSignal::new(),
            activity: ActivityTracker::new(),
            idle_timeout_secs: Arc::new(AtomicU64::new(Config::default().idle_timeout_secs)),
            verdict_config: Arc::new(None),
            next_thread_number: Arc::new(AtomicU64::new(1)),
            next_reply_number: Arc::new(AtomicU64::new(1)),
            next_take_number: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn for_process() -> Self {
        Self::new(
            State::new_shared(),
            Arc::new(EventBus::new(1024)),
            Arc::new(EventEmitter::stdout()),
        )
    }

    pub fn with_source(self, source: Source) -> Self {
        if let Ok(mut current) = self.source.write() {
            *current = source;
        }
        self
    }

    /// Single-file convenience used by tests and stdin sessions: replaces the
    /// first file's content (creating a default markdown file if none exist).
    pub fn with_markdown_source(self, markdown_source: impl Into<String>) -> Self {
        let content = markdown_source.into();
        if let Ok(mut source) = self.source.write() {
            if let Some(first) = source.files.first_mut() {
                first.content = content;
            } else {
                source.files.push(File {
                    id: default_file_id(),
                    path: "<stdin>".to_string(),
                    kind: FileKind::Markdown,
                    content,
                });
            }
        }
        self
    }

    fn current_source(&self) -> std::result::Result<Source, String> {
        self.source
            .read()
            .map(|source| source.clone())
            .map_err(|_| "source lock poisoned".to_string())
    }

    fn primary_file_id(&self) -> FileId {
        self.source
            .read()
            .ok()
            .and_then(|source| source.files.first().map(|file| file.id.clone()))
            .unwrap_or_else(default_file_id)
    }

    fn file_ids(&self) -> Vec<FileId> {
        self.source
            .read()
            .map(|source| source.files.iter().map(|file| file.id.clone()).collect())
            .unwrap_or_default()
    }

    fn files_count(&self) -> usize {
        self.source
            .read()
            .map(|source| source.files.len())
            .unwrap_or(0)
    }

    fn snapshot_with_files(&self) -> std::result::Result<crate::state::StateSnapshot, String> {
        let mut snapshot = self
            .state
            .read()
            .map_err(|_| "state lock poisoned while reading state".to_string())?
            .snapshot();
        snapshot.files = self
            .source
            .read()
            .map_err(|_| "source lock poisoned while reading state".to_string())?
            .files
            .iter()
            .map(crate::state::FileMeta::from)
            .collect();
        snapshot.verdict_config = self.verdict_config.as_ref().clone();
        Ok(snapshot)
    }

    pub fn with_source_path(mut self, source_path: impl Into<PathBuf>) -> Self {
        self.source_path = Arc::new(Some(source_path.into()));
        self
    }

    pub fn with_history_dir(mut self, history_dir: impl Into<PathBuf>) -> Self {
        self.history_dir = Arc::new(history_dir.into());
        self
    }

    pub fn with_no_save(self, no_save: bool) -> Self {
        self.no_save.store(no_save, Ordering::Relaxed);

        self
    }

    pub fn with_verdict_config(mut self, verdict_config: Option<VerdictConfig>) -> Self {
        self.verdict_config = Arc::new(verdict_config);

        self
    }

    pub fn with_idle_timeout_secs(self, idle_timeout_secs: u64) -> Self {
        self.idle_timeout_secs
            .store(idle_timeout_secs, Ordering::Relaxed);

        self
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    pub fn last_heartbeat_at(&self) -> std::result::Result<Instant, String> {
        self.activity.last_heartbeat_at()
    }

    fn record_heartbeat(&self) -> std::result::Result<Instant, String> {
        self.activity.record_heartbeat()
    }

    fn record_mutation(&self) {
        if let Err(error) = self.activity.record_mutation() {
            tracing::warn!(error, "failed to update last mutation timestamp");
        }
    }

    fn idle_timeout_secs(&self) -> u64 {
        self.idle_timeout_secs.load(Ordering::Relaxed)
    }

    fn no_save(&self) -> bool {
        self.no_save.load(Ordering::Relaxed)
    }

    fn next_user_thread_id(&self) -> ThreadId {
        let number = self.next_thread_number.fetch_add(1, Ordering::Relaxed);

        ThreadId(format!("u-{number}"))
    }

    fn next_reply_id(&self) -> String {
        let number = self.next_reply_number.fetch_add(1, Ordering::Relaxed);

        format!("r-{number}")
    }

    fn next_take_id(&self) -> String {
        let number = self.next_take_number.fetch_add(1, Ordering::Relaxed);

        format!("t-{number}")
    }
}

#[derive(Clone, Debug)]
struct ActivityTracker {
    inner: Arc<Mutex<ActivityState>>,
}

#[derive(Debug)]
struct ActivityState {
    last_heartbeat_at: Instant,
    last_mutation_at: Instant,
    last_idle_emit_at: Option<Instant>,
}

impl ActivityTracker {
    fn new() -> Self {
        let now = Instant::now();

        Self {
            inner: Arc::new(Mutex::new(ActivityState {
                last_heartbeat_at: now,
                last_mutation_at: now,
                last_idle_emit_at: None,
            })),
        }
    }

    fn last_heartbeat_at(&self) -> std::result::Result<Instant, String> {
        self.inner
            .lock()
            .map(|state| state.last_heartbeat_at)
            .map_err(|_| "activity lock poisoned".to_string())
    }

    fn record_heartbeat(&self) -> std::result::Result<Instant, String> {
        self.inner
            .lock()
            .map(|mut state| {
                let now = Instant::now();
                state.last_heartbeat_at = now;
                now
            })
            .map_err(|_| "activity lock poisoned".to_string())
    }

    fn record_mutation(&self) -> std::result::Result<Instant, String> {
        self.inner
            .lock()
            .map(|mut state| {
                let now = Instant::now();
                state.last_mutation_at = now;
                now
            })
            .map_err(|_| "activity lock poisoned".to_string())
    }

    fn record_idle_prompt_if_due(
        &self,
        now: Instant,
        idle_timeout: Duration,
    ) -> std::result::Result<Option<Duration>, String> {
        let mut state = self
            .inner
            .lock()
            .map_err(|_| "activity lock poisoned".to_string())?;
        let last_activity_at = state.last_heartbeat_at.max(state.last_mutation_at);
        let idle_for = now.saturating_duration_since(last_activity_at);

        if idle_for < idle_timeout {
            return Ok(None);
        }

        if let Some(last_idle_emit_at) = state.last_idle_emit_at {
            let already_emitted_for_current_window = last_idle_emit_at >= last_activity_at
                && now.saturating_duration_since(last_idle_emit_at) < idle_timeout;
            if already_emitted_for_current_window {
                return Ok(None);
            }
        }

        state.last_idle_emit_at = Some(now);

        Ok(Some(idle_for))
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::for_process()
    }
}

#[derive(Clone, Debug)]
struct ShutdownSignal {
    tx: watch::Sender<bool>,
}

impl ShutdownSignal {
    fn new() -> Self {
        let (tx, _) = watch::channel(false);

        Self { tx }
    }

    fn subscribe(&self) -> watch::Receiver<bool> {
        self.tx.subscribe()
    }

    fn signal(&self) {
        self.tx.send_replace(true);
    }

    fn is_signaled(&self) -> bool {
        *self.tx.borrow()
    }
}

pub async fn serve<F>(addr: SocketAddr, app_state: AppState, shutdown: F) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    serve_with_ready(addr, app_state, shutdown, |_| {}).await
}

pub async fn serve_with_ready<F, R>(
    addr: SocketAddr,
    app_state: AppState,
    shutdown: F,
    on_ready: R,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
    R: FnOnce(SocketAddr),
{
    ensure_loopback(addr)?;

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|error| bind_error(addr, error))?;
    let listening_addr = listener.local_addr().unwrap_or(addr);
    on_ready(listening_addr);

    spawn_idle_timer(app_state.clone());

    let router = build_router(app_state.clone());
    let shutdown_signal = app_state.shutdown.clone();
    let mut internal_shutdown = shutdown_signal.subscribe();

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            tokio::select! {
                _ = shutdown => {}
                _ = internal_shutdown.changed() => {}
            }
            shutdown_signal.signal();
        })
        .await
        .map_err(|source| DiscussError::ServerBindError { addr, source })
}

fn spawn_idle_timer(app_state: AppState) {
    let idle_timeout_secs = app_state.idle_timeout_secs();
    if idle_timeout_secs == 0 {
        return;
    }

    let idle_timeout = Duration::from_secs(idle_timeout_secs);
    let mut shutdown = app_state.subscribe_shutdown();
    let mut interval = tokio::time::interval(idle_check_interval(idle_timeout));
    interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;

                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                _ = interval.tick() => {
                    emit_idle_prompt_if_due(&app_state, idle_timeout);
                }
            }
        }
    });
}

fn idle_check_interval(idle_timeout: Duration) -> Duration {
    idle_timeout
        .saturating_mul(2)
        .clamp(MIN_IDLE_CHECK_INTERVAL, MAX_IDLE_CHECK_INTERVAL)
}

fn emit_idle_prompt_if_due(app_state: &AppState, idle_timeout: Duration) {
    let idle_for = match app_state
        .activity
        .record_idle_prompt_if_due(Instant::now(), idle_timeout)
    {
        Ok(Some(idle_for)) => idle_for,
        Ok(None) => return,
        Err(error) => {
            tracing::warn!(error, "failed to read idle activity timestamps");
            return;
        }
    };

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::PromptSuggestDone,
        at: Utc::now(),
        payload: serde_json::json!({
            "idle_for_secs": idle_for.as_secs(),
        }),
    }) {
        tracing::warn!(
            error = %error,
            "failed to emit prompt.suggest_done event"
        );
    }
}

fn build_router(app_state: AppState) -> Router {
    Router::new()
        .route("/", get(get_root))
        .route("/api/state", get(get_api_state))
        .route("/api/events", get(get_api_events))
        .route("/api/heartbeat", post(post_api_heartbeat))
        .route(
            "/api/drafts/new-thread",
            post(post_api_drafts_new_thread).delete(delete_api_drafts_new_thread),
        )
        .route(
            "/api/drafts/followup",
            post(post_api_drafts_followup).delete(delete_api_drafts_followup),
        )
        .route("/api/source", post(post_api_source))
        .route("/api/threads", post(post_api_threads))
        .route("/api/threads/{id}", delete(delete_api_thread))
        .route("/api/threads/{id}/replies", post(post_api_thread_replies))
        .route("/api/threads/{id}/takes", post(post_api_thread_takes))
        .route("/api/threads/{id}/resolve", post(post_api_thread_resolve))
        .route(
            "/api/threads/{id}/unresolve",
            post(post_api_thread_unresolve),
        )
        .route("/api/done", post(post_api_done))
        .route("/assets/mermaid.min.js", get(get_mermaid_js))
        .route("/assets/mermaid-shim.js", get(get_mermaid_shim_js))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            reject_during_shutdown,
        ))
        .fallback(not_found)
        .layer(TraceLayer::new_for_http())
        .with_state(app_state)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateThreadRequest {
    /// Which file the anchors refer to. Optional in single-file sessions
    /// (defaults to the only file), required when multiple files are loaded.
    #[serde(default)]
    file_id: Option<FileId>,
    anchor_start: usize,
    anchor_end: usize,
    snippet: String,
    text: String,
    #[serde(default)]
    line_range: Option<LineRange>,
    /// Optional optimistic-concurrency guard: when set, the thread is only
    /// created if the server's current source version matches, so anchors
    /// computed against an outdated document are rejected instead of drifting.
    #[serde(default)]
    source_version: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateThreadResponse {
    id: ThreadId,
    file_id: FileId,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct AddReplyRequest {
    text: String,
}

#[derive(Debug, Deserialize)]
struct AddTakeRequest {
    text: String,
}

#[derive(Debug, Deserialize)]
struct ResolveThreadRequest {
    decision: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertNewThreadDraftRequest {
    #[serde(default)]
    file_id: Option<FileId>,
    anchor_start: usize,
    anchor_end: usize,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClearNewThreadDraftRequest {
    #[serde(default)]
    file_id: Option<FileId>,
    anchor_start: usize,
    anchor_end: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpsertFollowupDraftRequest {
    thread_id: ThreadId,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClearFollowupDraftRequest {
    thread_id: ThreadId,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NewThreadDraftResponse {
    scope: &'static str,
    file_id: FileId,
    anchor_start: usize,
    anchor_end: usize,
    text: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NewThreadDraftCleared {
    scope: &'static str,
    file_id: FileId,
    anchor_start: usize,
    anchor_end: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FollowupDraftResponse {
    scope: &'static str,
    thread_id: ThreadId,
    text: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct FollowupDraftCleared {
    scope: &'static str,
    thread_id: ThreadId,
}

#[derive(Debug, Serialize)]
struct OkResponse {
    ok: bool,
}

#[derive(Debug, Serialize)]
struct DoneResponse {
    ok: bool,
    message: &'static str,
}

#[derive(Debug, Serialize)]
struct ApiErrorResponse {
    error: ApiError,
}

#[derive(Debug, Serialize)]
struct ApiError {
    code: &'static str,
    message: String,
}

/// `POST /api/source` — live source update with agent-pushed re-anchoring.
///
/// The agent that changed the markdown owns the re-anchor decision: it sends
/// the full new source plus one anchor entry per active thread (strict
/// coverage — every active thread must be re-anchored or explicitly orphaned).
/// The server swaps the source, rewrites anchors atomically under the state
/// lock, bumps the source version, and broadcasts `source.updated` over SSE
/// and stdout.
async fn post_api_source(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<SourceUpdateRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    let mut updates: HashMap<ThreadId, ThreadAnchorUpdate> = HashMap::new();
    for update in request.thread_anchors {
        if !update.orphaned {
            let (Some(start), Some(end)) = (update.anchor_start, update.anchor_end) else {
                return api_error_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    format!(
                        "thread {} must provide anchorStart and anchorEnd, or set orphaned: true",
                        update.thread_id.0
                    ),
                );
            };
            if start == 0 || end < start {
                return api_error_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    format!(
                        "thread {} anchors must satisfy 1 <= anchorStart <= anchorEnd",
                        update.thread_id.0
                    ),
                );
            }
            if let Some(line_range) = update.line_range
                && (line_range.start == 0 || line_range.end < line_range.start)
            {
                return api_error_response(
                    StatusCode::BAD_REQUEST,
                    "validation_error",
                    format!(
                        "thread {} lineRange must satisfy 1 <= start <= end",
                        update.thread_id.0
                    ),
                );
            }
        }
        let thread_id = update.thread_id.clone();
        if updates.insert(thread_id.clone(), update).is_some() {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                format!(
                    "thread {} appears more than once in threadAnchors",
                    thread_id.0
                ),
            );
        }
    }

    let file_id = match resolve_file_id(&app_state, request.file_id) {
        Ok(file_id) => file_id,
        Err(error) => return *error,
    };

    let markdown = request.markdown;
    let (threads, source_version, updated_file) = {
        let mut state = match app_state.state.write() {
            Ok(state) => state,
            Err(_) => {
                return api_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "state lock poisoned while updating source",
                );
            }
        };

        // Strict coverage in both directions, scoped to the updated file: a
        // silently-forgotten thread would drift onto wrong content, and an
        // unknown thread id is an agent bug worth surfacing.
        let active_ids: Vec<ThreadId> = state
            .get_threads()
            .into_iter()
            .filter(|t| t.file_id == file_id)
            .map(|t| t.id)
            .collect();
        let active_set: HashSet<&ThreadId> = active_ids.iter().collect();
        if let Some(missing) = active_ids.iter().find(|id| !updates.contains_key(id)) {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                format!(
                    "threadAnchors must cover every active thread on file {}: missing {} (re-anchor it or mark it orphaned)",
                    file_id.0, missing.0
                ),
            );
        }
        if let Some(unknown) = updates.keys().find(|id| !active_set.contains(id)) {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "validation_error",
                format!(
                    "threadAnchors references a thread that is not active on file {}: {}",
                    file_id.0, unknown.0
                ),
            );
        }

        for (thread_id, update) in &updates {
            let Some(thread) = state.thread_mut(thread_id) else {
                continue; // unreachable: validated active above
            };
            if update.orphaned {
                thread.orphaned = true;
            } else {
                thread.orphaned = false;
                thread.anchor_start = update.anchor_start.expect("validated anchorStart");
                thread.anchor_end = update.anchor_end.expect("validated anchorEnd");
                thread.line_range = update.line_range;
                if let Some(snippet) = &update.snippet {
                    thread.snippet = snippet.clone();
                }
            }
        }

        // Swap the file content while still holding the state write lock so
        // no reader can observe new anchors against the old source or vice
        // versa.
        let updated_file = match app_state.source.write() {
            Ok(mut source) => {
                let Some(file) = source.files.iter_mut().find(|file| file.id == file_id) else {
                    return api_error_response(
                        StatusCode::NOT_FOUND,
                        "unknown_file",
                        format!("unknown fileId: {}", file_id.0),
                    );
                };
                file.content = markdown.clone();
                file.clone()
            }
            Err(_) => {
                return api_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "source lock poisoned while updating source",
                );
            }
        };

        let source_version = state.bump_source_version();
        (state.get_threads(), source_version, updated_file)
    };
    app_state.record_mutation();

    let rendered_html = render_file_html(&updated_file);
    let thread_anchors = threads
        .iter()
        .filter(|thread| thread.file_id == file_id)
        .map(|thread| ThreadAnchorResponse {
            thread_id: thread.id.clone(),
            anchor_start: thread.anchor_start,
            anchor_end: thread.anchor_end,
            orphaned: thread.orphaned,
        })
        .collect();
    let orphaned_thread_ids = threads
        .iter()
        .filter(|thread| thread.orphaned && thread.file_id == file_id)
        .map(|thread| thread.id.clone())
        .collect();
    let payload = SourceUpdatedPayload {
        markdown,
        file_id,
        rendered_html,
        thread_anchors,
        orphaned_thread_ids,
        source_version,
    };
    let payload = match serde_json::to_value(&payload) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize source.updated payload: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::SourceUpdated.to_string(),
        payload: payload.clone(),
    });
    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::SourceUpdated,
        at: Utc::now(),
        payload: payload.clone(),
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit source.updated event: {error}"),
        );
    }

    Json(payload).into_response()
}

async fn post_api_threads(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<CreateThreadRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };
    if let Some(line_range) = request.line_range
        && (line_range.start == 0 || line_range.end < line_range.start)
    {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "lineRange must satisfy 1 <= start <= end",
        );
    }
    if let Some(requested_version) = request.source_version {
        let current_version = match app_state.state.read() {
            Ok(state) => state.source_version(),
            Err(_) => {
                return api_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "state lock poisoned while checking source version",
                );
            }
        };
        if requested_version != current_version {
            return api_error_response(
                StatusCode::CONFLICT,
                "stale_source_version",
                format!(
                    "sourceVersion {requested_version} is stale; the document is now at version {current_version}, refresh anchors against the current source"
                ),
            );
        }
    }
    let file_id = match resolve_file_id(&app_state, request.file_id) {
        Ok(file_id) => file_id,
        Err(error) => return *error,
    };
    let created_at = Utc::now();
    let thread = Thread {
        id: app_state.next_user_thread_id(),
        file_id: file_id.clone(),
        anchor_start: request.anchor_start,
        anchor_end: request.anchor_end,
        snippet: request.snippet,
        breadcrumb: String::new(),
        text: request.text,
        created_at,
        kind: ThreadKind::User,
        line_range: request.line_range,
        orphaned: false,
    };

    if app_state
        .state
        .write()
        .map(|mut state| state.add_thread(thread.clone()))
        .is_err()
    {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "state lock poisoned while creating thread",
        );
    }
    app_state.record_mutation();

    let payload = match serde_json::to_value(&thread) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize created thread: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::ThreadCreated.to_string(),
        payload: payload.clone(),
    });

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::ThreadCreated,
        at: created_at,
        payload,
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit thread.created event: {error}"),
        );
    }

    Json(CreateThreadResponse {
        id: thread.id,
        file_id,
        created_at,
    })
    .into_response()
}

/// Resolves an optional client-supplied file id against the loaded files:
/// missing means "the only file" in single-file sessions but is an error when
/// several files are loaded; unknown ids are always an error.
fn resolve_file_id(
    app_state: &AppState,
    requested: Option<FileId>,
) -> std::result::Result<FileId, Box<Response>> {
    let known = app_state.file_ids();

    match requested {
        Some(file_id) => {
            if known.is_empty() || known.contains(&file_id) {
                Ok(file_id)
            } else {
                Err(Box::new(api_error_response(
                    StatusCode::NOT_FOUND,
                    "unknown_file",
                    format!("unknown fileId: {}", file_id.0),
                )))
            }
        }
        None => {
            if known.len() > 1 {
                Err(Box::new(api_error_response(
                    StatusCode::BAD_REQUEST,
                    "missing_file_id",
                    "fileId is required when multiple files are loaded",
                )))
            } else {
                Ok(app_state.primary_file_id())
            }
        }
    }
}

async fn post_api_thread_replies(
    AxumState(app_state): AxumState<AppState>,
    Path(thread_id): Path<String>,
    payload: std::result::Result<Json<AddReplyRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    if request.text.trim().is_empty() {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "reply text must not be empty",
        );
    }

    let thread_id = ThreadId(thread_id);
    let reply = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while adding reply",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        }

        state.add_reply(Reply {
            id: app_state.next_reply_id(),
            thread_id: thread_id.clone(),
            text: request.text,
            created_at: Utc::now(),
        })
    };
    app_state.record_mutation();

    let payload = match serde_json::to_value(&reply) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize reply: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::ReplyAdded.to_string(),
        payload: payload.clone(),
    });

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::ReplyAdded,
        at: reply.created_at,
        payload,
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit reply.added event: {error}"),
        );
    }

    Json(reply).into_response()
}

async fn post_api_thread_takes(
    AxumState(app_state): AxumState<AppState>,
    Path(thread_id): Path<String>,
    payload: std::result::Result<Json<AddTakeRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    if request.text.trim().is_empty() {
        return api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            "take text must not be empty",
        );
    }

    let thread_id = ThreadId(thread_id);
    let take = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while adding take",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        }

        state.add_take(Take {
            id: app_state.next_take_id(),
            thread_id: thread_id.clone(),
            text: request.text,
            created_at: Utc::now(),
        })
    };
    app_state.record_mutation();

    let payload = match serde_json::to_value(&take) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize take: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: "take.added".to_string(),
        payload: payload.clone(),
    });

    Json(take).into_response()
}

async fn post_api_thread_resolve(
    AxumState(app_state): AxumState<AppState>,
    Path(thread_id): Path<String>,
    payload: std::result::Result<Json<ResolveThreadRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    let thread_id = ThreadId(thread_id);
    let resolution = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while resolving thread",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        }

        state.set_resolution(
            thread_id.clone(),
            Resolution {
                decision: request.decision,
                resolved_at: Utc::now(),
            },
        )
    };
    app_state.record_mutation();

    let payload = serde_json::json!({
        "threadId": thread_id,
        "resolution": resolution,
    });

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::ThreadResolved.to_string(),
        payload: payload.clone(),
    });

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::ThreadResolved,
        at: resolution.resolved_at,
        payload,
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit thread.resolved event: {error}"),
        );
    }

    Json(resolution).into_response()
}

async fn post_api_thread_unresolve(
    AxumState(app_state): AxumState<AppState>,
    Path(thread_id): Path<String>,
) -> Response {
    let thread_id = ThreadId(thread_id);
    let emitted_at = Utc::now();

    {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while unresolving thread",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        }

        state.clear_resolution(&thread_id);
    }
    app_state.record_mutation();

    let payload = serde_json::json!({ "threadId": thread_id });

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::ThreadUnresolved.to_string(),
        payload: payload.clone(),
    });

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::ThreadUnresolved,
        at: emitted_at,
        payload,
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit thread.unresolved event: {error}"),
        );
    }

    Json(OkResponse { ok: true }).into_response()
}

async fn delete_api_thread(
    AxumState(app_state): AxumState<AppState>,
    Path(thread_id): Path<String>,
) -> Response {
    let thread_id = ThreadId(thread_id);
    let emitted_at = Utc::now();

    {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while deleting thread",
            );
        };

        let Some(thread) = state
            .get_threads()
            .into_iter()
            .find(|thread| thread.id == thread_id)
        else {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        };

        if thread.kind == ThreadKind::Prepopulated {
            return api_error_response(
                StatusCode::FORBIDDEN,
                "prepopulated_thread",
                format!("prepopulated thread cannot be deleted: {}", thread_id.0),
            );
        }

        state.soft_delete_thread(&thread_id);
    }
    app_state.record_mutation();

    let payload = serde_json::json!({ "threadId": thread_id });

    app_state.bus.publish(BroadcastEvent {
        kind: EventKind::ThreadDeleted.to_string(),
        payload: payload.clone(),
    });

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::ThreadDeleted,
        at: emitted_at,
        payload,
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit thread.deleted event: {error}"),
        );
    }

    Json(OkResponse { ok: true }).into_response()
}

async fn post_api_drafts_new_thread(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<UpsertNewThreadDraftRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    let file_id = match resolve_file_id(&app_state, request.file_id) {
        Ok(file_id) => file_id,
        Err(error) => return *error,
    };

    if request.text.trim().is_empty() {
        return clear_new_thread_draft(
            &app_state,
            ClearNewThreadDraftRequest {
                file_id: Some(file_id),
                anchor_start: request.anchor_start,
                anchor_end: request.anchor_end,
            },
        );
    }

    let updated_at = Utc::now();
    let draft = Draft {
        text: request.text,
        updated_at,
    };

    let key = NewThreadDraftKey::new(file_id.clone(), request.anchor_start, request.anchor_end);
    if app_state
        .state
        .write()
        .map(|mut state| state.upsert_new_thread_draft(key, draft.clone()))
        .is_err()
    {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "state lock poisoned while saving new-thread draft",
        );
    }
    app_state.record_mutation();

    let response = NewThreadDraftResponse {
        scope: "newThread",
        file_id,
        anchor_start: request.anchor_start,
        anchor_end: request.anchor_end,
        text: draft.text,
        updated_at,
    };
    let payload = match serde_json::to_value(&response) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize new-thread draft: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: "draft.updated".to_string(),
        payload: payload.clone(),
    });

    Json(response).into_response()
}

async fn delete_api_drafts_new_thread(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<ClearNewThreadDraftRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    clear_new_thread_draft(&app_state, request)
}

fn clear_new_thread_draft(app_state: &AppState, request: ClearNewThreadDraftRequest) -> Response {
    let file_id = match resolve_file_id(app_state, request.file_id) {
        Ok(file_id) => file_id,
        Err(error) => return *error,
    };
    let key = NewThreadDraftKey::new(file_id.clone(), request.anchor_start, request.anchor_end);
    if app_state
        .state
        .write()
        .map(|mut state| state.clear_new_thread_draft(&key))
        .is_err()
    {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "state lock poisoned while clearing new-thread draft",
        );
    }
    app_state.record_mutation();

    let cleared = NewThreadDraftCleared {
        scope: "newThread",
        file_id,
        anchor_start: request.anchor_start,
        anchor_end: request.anchor_end,
    };
    let payload = match serde_json::to_value(cleared) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize cleared new-thread draft: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: "draft.cleared".to_string(),
        payload: payload.clone(),
    });

    Json(OkResponse { ok: true }).into_response()
}

async fn post_api_drafts_followup(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<UpsertFollowupDraftRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    if request.text.trim().is_empty() {
        return clear_followup_draft(
            &app_state,
            ClearFollowupDraftRequest {
                thread_id: request.thread_id,
            },
        );
    }

    let updated_at = Utc::now();
    let draft = Draft {
        text: request.text,
        updated_at,
    };
    let response = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while saving follow-up draft",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == request.thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", request.thread_id.0),
            );
        }

        state.upsert_followup_draft(request.thread_id.clone(), draft.clone());

        FollowupDraftResponse {
            scope: "followup",
            thread_id: request.thread_id,
            text: draft.text,
            updated_at,
        }
    };
    app_state.record_mutation();
    let payload = match serde_json::to_value(&response) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize follow-up draft: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: "draft.updated".to_string(),
        payload: payload.clone(),
    });

    Json(response).into_response()
}

async fn delete_api_drafts_followup(
    AxumState(app_state): AxumState<AppState>,
    payload: std::result::Result<Json<ClearFollowupDraftRequest>, JsonRejection>,
) -> Response {
    let Json(request) = match payload {
        Ok(payload) => payload,
        Err(rejection) => {
            return api_error_response(
                StatusCode::BAD_REQUEST,
                "bad_request",
                rejection.body_text(),
            );
        }
    };

    clear_followup_draft(&app_state, request)
}

fn clear_followup_draft(app_state: &AppState, request: ClearFollowupDraftRequest) -> Response {
    {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while clearing follow-up draft",
            );
        };

        if !state
            .get_threads()
            .iter()
            .any(|thread| thread.id == request.thread_id)
        {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", request.thread_id.0),
            );
        }

        state.clear_followup_draft(&request.thread_id);
    }
    app_state.record_mutation();

    let cleared = FollowupDraftCleared {
        scope: "followup",
        thread_id: request.thread_id,
    };
    let payload = match serde_json::to_value(cleared) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize cleared follow-up draft: {error}"),
            );
        }
    };

    app_state.bus.publish(BroadcastEvent {
        kind: "draft.cleared".to_string(),
        payload: payload.clone(),
    });

    Json(OkResponse { ok: true }).into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DoneRequest {
    verdict: DoneVerdictRequest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DoneVerdictRequest {
    option_id: String,
    #[serde(default)]
    feedback: Option<String>,
}

async fn post_api_done(AxumState(app_state): AxumState<AppState>, body: Bytes) -> Response {
    let emitted_at = Utc::now();
    let verdict = match validate_done_verdict(&app_state, &body, emitted_at) {
        Ok(verdict) => verdict,
        Err(response) => return *response,
    };

    let source = match app_state.current_source() {
        Ok(source) => source,
        Err(message) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                message,
            );
        }
    };
    let transcript = match app_state.state.read() {
        Ok(state) => {
            let transcript = build_transcript_with_source(&state, &source);
            match verdict {
                Some(verdict) => transcript.with_verdict(verdict),
                None => transcript,
            }
        }
        Err(_) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while building transcript",
            );
        }
    };
    let payload = match serde_json::to_value(transcript) {
        Ok(payload) => payload,
        Err(error) => {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                format!("failed to serialize transcript: {error}"),
            );
        }
    };

    if let Err(error) = app_state.emitter.emit(&Event {
        kind: EventKind::SessionDone,
        at: emitted_at,
        payload: payload.clone(),
    }) {
        return api_error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            format!("failed to emit session.done event: {error}"),
        );
    }

    if !app_state.no_save() {
        let history_path = history::history_archive_path(
            app_state.history_dir.as_ref().as_path(),
            app_state.source_path.as_ref().as_deref(),
            app_state.files_count(),
            emitted_at,
        );
        if let Err(error) = history::write_history_archive(&history_path, &payload) {
            warn_history_archive_failure(&history_path, &error);
        }
    }

    app_state.record_mutation();
    app_state.shutdown.signal();

    Json(DoneResponse {
        ok: true,
        message: "transcript emitted",
    })
    .into_response()
}

fn validate_done_verdict(
    app_state: &AppState,
    body: &[u8],
    decided_at: DateTime<Utc>,
) -> std::result::Result<Option<Verdict>, Box<Response>> {
    let Some(config) = app_state.verdict_config.as_ref() else {
        return Ok(None);
    };

    if body.is_empty() {
        return Err(Box::new(api_error_response(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "verdict request body is required",
        )));
    }

    let request = serde_json::from_slice::<DoneRequest>(body).map_err(|error| {
        Box::new(api_error_response(
            StatusCode::BAD_REQUEST,
            "bad_request",
            format!("invalid verdict request body: {error}"),
        ))
    })?;

    let Some(option) = config
        .options
        .iter()
        .find(|option| option.id == request.verdict.option_id)
    else {
        return Err(Box::new(api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            format!("unknown verdict optionId: {}", request.verdict.option_id),
        )));
    };

    let feedback = request
        .verdict
        .feedback
        .as_deref()
        .map(str::trim)
        .filter(|feedback| !feedback.is_empty())
        .map(ToOwned::to_owned);

    if option.feedback_required && feedback.is_none() {
        return Err(Box::new(api_error_response(
            StatusCode::BAD_REQUEST,
            "validation_error",
            format!("feedback is required for verdict optionId: {}", option.id),
        )));
    }

    Ok(Some(Verdict {
        option_id: option.id.clone(),
        label: option.label.clone(),
        feedback,
        decided_at,
    }))
}

fn warn_history_archive_failure(path: &FsPath, error: &io::Error) {
    tracing::warn!(
        path = %path.display(),
        error = %error,
        "failed to write history archive"
    );
    let _ = writeln!(
        io::stderr(),
        "warning: failed to write history archive to {}: {error}",
        path.display()
    );
}

fn api_error_response(
    status: StatusCode,
    code: &'static str,
    message: impl Into<String>,
) -> Response {
    (
        status,
        Json(ApiErrorResponse {
            error: ApiError {
                code,
                message: message.into(),
            },
        }),
    )
        .into_response()
}

async fn reject_during_shutdown(
    AxumState(app_state): AxumState<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if app_state.shutdown.is_signaled() {
        return api_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "shutting_down",
            "discuss session is shutting down",
        );
    }

    next.run(request).await
}

async fn get_root(AxumState(app_state): AxumState<AppState>) -> Response {
    match render_root_page(&app_state) {
        Ok(page) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            page,
        )
            .into_response(),
        Err(message) => (StatusCode::INTERNAL_SERVER_ERROR, message).into_response(),
    }
}

fn render_root_page(app_state: &AppState) -> std::result::Result<String, String> {
    let snapshot = app_state.snapshot_with_files()?;
    let initial_state_json = serde_json::to_string(&snapshot)
        .map_err(|error| format!("failed to serialize initial state: {error}"))?;
    let source = app_state.current_source()?;

    // Every file is pre-rendered and seeded into the page so switching files
    // in the sidebar is a client-side swap with no extra round trip.
    let rendered_files: Vec<RenderedFile> = source
        .files
        .iter()
        .map(|file| RenderedFile {
            id: file.id.clone(),
            html: render_file_html(file),
        })
        .collect();
    let rendered_files_json = serde_json::to_string(&rendered_files)
        .map_err(|error| format!("failed to serialize rendered files: {error}"))?;

    let first_file_html = rendered_files
        .first()
        .map(|file| file.html.clone())
        .unwrap_or_default();

    Ok(template::render_page(
        &first_file_html,
        &initial_state_json,
        &rendered_files_json,
    ))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RenderedFile {
    id: FileId,
    html: String,
}

/// Renders one source file to HTML: markdown files through the markdown
/// renderer directly, diff files through a synthesized markdown document
/// (heading + one fenced `diff-<lang>` block per hunk).
fn render_file_html(file: &File) -> String {
    match file.kind {
        FileKind::Markdown => render::render(&file.content),
        FileKind::Diff => render::render(&crate::diff::diff_content_to_markdown(
            &file.path,
            &file.content,
        )),
    }
}

async fn get_api_state(AxumState(app_state): AxumState<AppState>) -> Response {
    match app_state.snapshot_with_files() {
        Ok(snapshot) => Json(snapshot).into_response(),
        Err(message) => {
            api_error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
        }
    }
}

async fn post_api_heartbeat(AxumState(app_state): AxumState<AppState>) -> Response {
    match app_state.record_heartbeat() {
        Ok(_) => Json(OkResponse { ok: true }).into_response(),
        Err(message) => {
            api_error_response(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
        }
    }
}

async fn get_api_events(AxumState(app_state): AxumState<AppState>) -> impl IntoResponse {
    let mut events = app_state.bus.subscribe();
    let mut shutdown = app_state.subscribe_shutdown();
    let stream = async_stream::stream! {
        loop {
            tokio::select! {
                biased;

                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
                event = events.recv() => {
                    match event {
                        Ok(event) => {
                            let Ok(payload) = serde_json::to_string(&event.payload) else {
                                continue;
                            };
                            yield Ok::<_, std::convert::Infallible>(
                                SseEvent::default().event(event.kind).data(payload),
                            );
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    };

    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(SSE_HEARTBEAT_INTERVAL)
            .text("keep-alive"),
    )
}

async fn get_mermaid_js() -> impl IntoResponse {
    javascript_response(assets::mermaid_js())
}

async fn get_mermaid_shim_js() -> impl IntoResponse {
    javascript_response(assets::mermaid_shim_js())
}

fn javascript_response(body: &'static str) -> impl IntoResponse {
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, JAVASCRIPT_CONTENT_TYPE),
            (header::CACHE_CONTROL, ASSET_CACHE_CONTROL),
        ],
        body,
    )
}

async fn not_found() -> StatusCode {
    StatusCode::NOT_FOUND
}

fn ensure_loopback(addr: SocketAddr) -> Result<()> {
    if addr.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Ok(());
    }

    Err(DiscussError::ServerBindError {
        addr,
        source: io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "discuss only binds to 127.0.0.1",
        ),
    })
}

fn bind_error(addr: SocketAddr, error: io::Error) -> DiscussError {
    if error.kind() == io::ErrorKind::AddrInUse {
        DiscussError::PortInUse { port: addr.port() }
    } else {
        DiscussError::ServerBindError {
            addr,
            source: error,
        }
    }
}
