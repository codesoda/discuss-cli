use std::future::Future;
use std::io::{self, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::rejection::JsonRejection;
use axum::extract::Path;
use axum::extract::State as AxumState;
use axum::http::header;
use axum::http::StatusCode;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use axum::Router;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::broadcast;
use tokio::sync::watch;
use tower_http::trace::TraceLayer;

use crate::assets;
use crate::events::{Event, EventEmitter, EventKind};
use crate::sse::{BroadcastEvent, EventBus};
use crate::state::{Reply, SharedState, State, Thread, ThreadId, ThreadKind};
use crate::{render, template, DiscussError, Result};

const JAVASCRIPT_CONTENT_TYPE: &str = "application/javascript";
const ASSET_CACHE_CONTROL: &str = "public, max-age=86400";
const SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

#[derive(Clone, Debug)]
pub struct AppState {
    pub state: SharedState,
    pub bus: Arc<EventBus>,
    pub emitter: Arc<EventEmitter<Box<dyn Write + Send>>>,
    markdown_source: Arc<str>,
    shutdown: ShutdownSignal,
    next_thread_number: Arc<AtomicU64>,
    next_reply_number: Arc<AtomicU64>,
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
            markdown_source: Arc::from(""),
            shutdown: ShutdownSignal::new(),
            next_thread_number: Arc::new(AtomicU64::new(1)),
            next_reply_number: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn for_process() -> Self {
        Self::new(
            State::new_shared(),
            Arc::new(EventBus::new(1024)),
            Arc::new(EventEmitter::stdout()),
        )
    }

    pub fn with_markdown_source(mut self, markdown_source: impl Into<String>) -> Self {
        let markdown_source = markdown_source.into();
        self.markdown_source = Arc::from(markdown_source.into_boxed_str());
        self
    }

    pub fn subscribe_shutdown(&self) -> watch::Receiver<bool> {
        self.shutdown.subscribe()
    }

    fn next_user_thread_id(&self) -> ThreadId {
        let number = self.next_thread_number.fetch_add(1, Ordering::Relaxed);

        ThreadId(format!("u-{number}"))
    }

    fn next_reply_id(&self) -> String {
        let number = self.next_reply_number.fetch_add(1, Ordering::Relaxed);

        format!("r-{number}")
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
}

pub async fn serve<F>(addr: SocketAddr, app_state: AppState, shutdown: F) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    ensure_loopback(addr)?;

    let listener = TcpListener::bind(addr)
        .await
        .map_err(|error| bind_error(addr, error))?;
    let router = build_router(app_state.clone());
    let shutdown_signal = app_state.shutdown.clone();

    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            shutdown.await;
            shutdown_signal.signal();
        })
        .await
        .map_err(|source| DiscussError::ServerBindError { addr, source })
}

fn build_router(app_state: AppState) -> Router {
    Router::new()
        .route("/", get(get_root))
        .route("/api/state", get(get_api_state))
        .route("/api/events", get(get_api_events))
        .route("/api/threads", post(post_api_threads))
        .route("/api/threads/{id}/replies", post(post_api_thread_replies))
        .route("/assets/mermaid.min.js", get(get_mermaid_js))
        .route("/assets/mermaid-shim.js", get(get_mermaid_shim_js))
        .fallback(not_found)
        .layer(TraceLayer::new_for_http())
        .with_state(app_state)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateThreadRequest {
    anchor_start: usize,
    anchor_end: usize,
    snippet: String,
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateThreadResponse {
    id: ThreadId,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct AddReplyRequest {
    text: String,
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
    let created_at = Utc::now();
    let thread = Thread {
        id: app_state.next_user_thread_id(),
        anchor_start: request.anchor_start,
        anchor_end: request.anchor_end,
        snippet: request.snippet,
        breadcrumb: String::new(),
        text: request.text,
        created_at,
        kind: ThreadKind::User,
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
        created_at,
    })
    .into_response()
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
    let snapshot = app_state
        .state
        .read()
        .map_err(|_| "state lock poisoned while rendering page".to_string())?
        .snapshot();
    let initial_state_json = serde_json::to_string(&snapshot)
        .map_err(|error| format!("failed to serialize initial state: {error}"))?;
    let rendered_markdown = render::render(app_state.markdown_source.as_ref());

    Ok(template::render_page(
        &rendered_markdown,
        &initial_state_json,
    ))
}

async fn get_api_state(AxumState(app_state): AxumState<AppState>) -> Response {
    match app_state.state.read() {
        Ok(state) => Json(state.snapshot()).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "state lock poisoned while reading state",
        )
            .into_response(),
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
