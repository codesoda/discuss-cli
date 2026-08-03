//! Local HTTP server: routing, lifecycle, and the shared `AppState` handle.
//!
//! Route handlers live in focused submodules (`source`, `threads`, `drafts`,
//! `done`, `pages`) and share the helpers defined here.

mod app_state;
mod done;
mod drafts;
mod pages;
mod response;
mod source;
mod threads;

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::State as AxumState;
use axum::http::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get, post};
use chrono::Utc;
use tokio::net::TcpListener;
use tokio::time::MissedTickBehavior;
use tower_http::trace::TraceLayer;

use crate::events::{Event, EventKind};
use crate::state::FileId;
use crate::{DiscussError, Result};

pub use app_state::AppState;

use done::post_api_done;
use drafts::{
    delete_api_drafts_followup, delete_api_drafts_new_thread, post_api_drafts_followup,
    post_api_drafts_new_thread,
};
use pages::{
    get_api_events, get_api_state, get_mermaid_js, get_mermaid_shim_js, get_root,
    post_api_heartbeat,
};
use response::{api_error_response, not_found};
use source::post_api_source;
use threads::{
    delete_api_thread, post_api_thread_replies, post_api_thread_resolve, post_api_thread_takes,
    post_api_thread_unresolve, post_api_threads,
};

const JAVASCRIPT_CONTENT_TYPE: &str = "application/javascript";
const ASSET_CACHE_CONTROL: &str = "public, max-age=86400";
const SSE_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);
const MAX_IDLE_CHECK_INTERVAL: Duration = Duration::from_secs(10);
const MIN_IDLE_CHECK_INTERVAL: Duration = Duration::from_millis(100);

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

/// Resolves an optional client-supplied file id against the loaded files:
/// missing means "the only file" in single-file sessions but is an error when
/// several files are loaded; unknown ids are always an error.
/// Resolves an optional client-supplied file id against the loaded files:
/// missing means "the only file" in single-file sessions but is an error when
/// several files are loaded; unknown ids are always an error.
pub(super) fn resolve_file_id(
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
