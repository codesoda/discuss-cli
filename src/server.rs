use std::collections::BTreeMap;
use std::future::Future;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::Router;
use axum::body::Body;
use axum::extract::Path;
use axum::extract::Query;
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
    Draft, File, FileId, FileKind, FileMeta, LineRange, Reply, Resolution, SharedState, Source,
    State, StateSnapshot, Take, Thread, ThreadId, ThreadKind, default_file_id,
};
use crate::transcript::build_transcript;
use crate::{Config, DiscussError, Result, render, template};

const JAVASCRIPT_CONTENT_TYPE: &str = "application/javascript";
const ASSET_CACHE_CONTROL: &str = "public, max-age=86400";
const SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const MAX_IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(10);
const MIN_IDLE_CHECK_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub struct AppState {
    pub state: SharedState,
    pub bus: Arc<EventBus>,
    pub emitter: Arc<EventEmitter<Box<dyn Write + Send>>>,
    source: Arc<Source>,
    source_path: Arc<Option<PathBuf>>,
    history_dir: Arc<PathBuf>,
    no_save: Arc<AtomicBool>,
    shutdown: ShutdownSignal,
    activity: ActivityTracker,
    idle_timeout_secs: Arc<AtomicU64>,
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
            source: Arc::new(Source::default()),
            source_path: Arc::new(None),
            history_dir: Arc::new(history::default_history_dir()),
            no_save: Arc::new(AtomicBool::new(false)),
            shutdown: ShutdownSignal::new(),
            activity: ActivityTracker::new(),
            idle_timeout_secs: Arc::new(AtomicU64::new(Config::default().idle_timeout_secs)),
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

    pub fn with_source(mut self, source: Source) -> Self {
        self.source = Arc::new(source);
        self
    }

    pub fn with_markdown_source(self, markdown_source: impl Into<String>) -> Self {
        let content = markdown_source.into();
        let mut files = self.source.files.clone();
        if let Some(first) = files.first_mut() {
            first.content = content;
        } else {
            files.push(File {
                id: default_file_id(),
                path: "<stdin>".to_string(),
                kind: FileKind::Markdown,
                content,
            });
        }

        self.with_source(Source { files })
    }

    pub fn with_source_path(mut self, source_path: impl Into<PathBuf>) -> Self {
        self.source_path = Arc::new(Some(source_path.into()));
        self
    }

    pub fn source(&self) -> &Source {
        self.source.as_ref()
    }

    fn primary_file_id(&self) -> FileId {
        self.source
            .files
            .first()
            .map(|file| file.id.clone())
            .unwrap_or_else(default_file_id)
    }

    fn file_ids(&self) -> Vec<FileId> {
        self.source
            .files
            .iter()
            .map(|file| file.id.clone())
            .collect()
    }

    fn snapshot_with_files(&self) -> std::result::Result<StateSnapshot, String> {
        let mut snapshot = self
            .state
            .read()
            .map_err(|_| "state lock poisoned while reading state".to_string())?
            .snapshot();
        snapshot.files = self.source.files.iter().map(FileMeta::from).collect();
        Ok(snapshot)
    }

    fn primary_markdown_content(&self) -> &str {
        self.source
            .files
            .first()
            .map(|file| file.content.as_str())
            .unwrap_or("")
    }

    pub fn with_history_dir(mut self, history_dir: impl Into<PathBuf>) -> Self {
        self.history_dir = Arc::new(history_dir.into());
        self
    }

    pub fn with_no_save(self, no_save: bool) -> Self {
        self.no_save.store(no_save, Ordering::Relaxed);

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
        .route("/assets/preact.umd.js", get(get_preact_js))
        .route("/assets/preact-hooks.umd.js", get(get_preact_hooks_js))
        .route("/assets/htm.umd.js", get(get_htm_js))
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
    #[serde(default)]
    file_id: Option<FileId>,
    anchor_start: usize,
    anchor_end: usize,
    snippet: String,
    text: String,
    #[serde(default)]
    line_range: Option<LineRange>,
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
    anchor_start: usize,
    anchor_end: usize,
    text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClearNewThreadDraftRequest {
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
    anchor_start: usize,
    anchor_end: usize,
    text: String,
    updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct NewThreadDraftCleared {
    scope: &'static str,
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
    let (resolution, file_id) = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while resolving thread",
            );
        };

        let Some(file_id) = state
            .get_threads()
            .iter()
            .find(|thread| thread.id == thread_id)
            .map(|thread| thread.file_id.clone())
        else {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        };

        let resolution = state.set_resolution(
            thread_id.clone(),
            Resolution {
                decision: request.decision,
                resolved_at: Utc::now(),
            },
        );

        (resolution, file_id)
    };
    app_state.record_mutation();

    let payload = serde_json::json!({
        "threadId": thread_id,
        "fileId": file_id,
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

    let file_id = {
        let Ok(mut state) = app_state.state.write() else {
            return api_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                "state lock poisoned while unresolving thread",
            );
        };

        let Some(file_id) = state
            .get_threads()
            .iter()
            .find(|thread| thread.id == thread_id)
            .map(|thread| thread.file_id.clone())
        else {
            return api_error_response(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("thread not found: {}", thread_id.0),
            );
        };

        state.clear_resolution(&thread_id);

        file_id
    };
    app_state.record_mutation();

    let payload = serde_json::json!({ "threadId": thread_id, "fileId": file_id });

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

    let file_id = {
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

        let file_id = thread.file_id.clone();
        state.soft_delete_thread(&thread_id);
        file_id
    };
    app_state.record_mutation();

    let payload = serde_json::json!({ "threadId": thread_id, "fileId": file_id });

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

    if request.text.trim().is_empty() {
        return clear_new_thread_draft(
            &app_state,
            ClearNewThreadDraftRequest {
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

    if app_state
        .state
        .write()
        .map(|mut state| {
            state.upsert_new_thread_draft(request.anchor_start, request.anchor_end, draft.clone())
        })
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
    if app_state
        .state
        .write()
        .map(|mut state| state.clear_new_thread_draft(request.anchor_start, request.anchor_end))
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

async fn post_api_done(AxumState(app_state): AxumState<AppState>) -> Response {
    let transcript = match app_state.state.read() {
        Ok(state) => build_transcript(&state),
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
    let emitted_at = Utc::now();

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

#[derive(Debug, Default, Deserialize)]
struct UiVariantQuery {
    #[serde(default)]
    ui: Option<String>,
}

async fn get_root(
    AxumState(app_state): AxumState<AppState>,
    Query(query): Query<UiVariantQuery>,
) -> Response {
    let result = match query.ui.as_deref() {
        Some("v2") => render_root_page_v2(&app_state),
        _ => render_root_page(&app_state),
    };

    match result {
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
    let rendered_markdown = render::render(app_state.primary_markdown_content());

    Ok(template::render_page(
        &rendered_markdown,
        &initial_state_json,
    ))
}

fn render_root_page_v2(app_state: &AppState) -> std::result::Result<String, String> {
    let snapshot = app_state.snapshot_with_files()?;
    let initial_state_json = serde_json::to_string(&snapshot)
        .map_err(|error| format!("failed to serialize initial state: {error}"))?;
    let rendered_files = rendered_markdown_files(app_state.source());
    let rendered_files_json = serde_json::to_string(&rendered_files)
        .map_err(|error| format!("failed to serialize rendered files: {error}"))?;

    Ok(template::render_v2_page(
        &initial_state_json,
        &rendered_files_json,
    ))
}

fn rendered_markdown_files(source: &Source) -> BTreeMap<String, String> {
    source
        .files
        .iter()
        .filter(|file| file.kind == FileKind::Markdown)
        .map(|file| (file.id.0.clone(), render::render(&file.content)))
        .collect()
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

async fn get_preact_js() -> impl IntoResponse {
    javascript_response(assets::preact_js())
}

async fn get_preact_hooks_js() -> impl IntoResponse {
    javascript_response(assets::preact_hooks_js())
}

async fn get_htm_js() -> impl IntoResponse {
    javascript_response(assets::htm_js())
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
