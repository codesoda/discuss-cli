//! Local HTTP server: routing, lifecycle, and the shared `AppState` handle.
//!
//! Route handlers live in focused submodules (`source`, `threads`, `drafts`,
//! `done`, `pages`) and share the helpers defined here.

mod anchors;
mod app_state;
mod blocks;
pub mod demo;
mod done;
mod drafts;
mod files;
mod pages;
mod pr;
mod response;
mod source;
mod threads;

use std::future::Future;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::Body;
use axum::extract::DefaultBodyLimit;
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
use crate::pr::MAX_IMPORT_BYTES;
use crate::state::FileId;
use crate::{DiscussError, Result};

pub use app_state::AppState;

use anchors::post_api_anchors_resolve;
use blocks::get_api_file_blocks;
use done::post_api_done;
use drafts::{
    delete_api_drafts_followup, delete_api_drafts_new_thread, post_api_drafts_followup,
    post_api_drafts_new_thread,
};
use files::{get_html_asset, get_html_file};
use pages::{
    get_api_events, get_api_file_raw, get_api_state, get_api_version, get_discuss_inspect_js,
    get_mermaid_js, get_mermaid_shim_js, get_root, post_api_heartbeat,
};
use pr::{
    delete_api_pr_file_viewed, get_api_pr_draft, post_api_pr_cancel, post_api_pr_confirm,
    post_api_pr_draft, post_api_pr_file_viewed, post_api_pr_import, post_api_pr_prepare,
    post_api_pr_publication_result, post_api_pr_publish, post_api_pr_summary,
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

pub async fn bind_loopback_listeners(
    requested_addrs: &[SocketAddr],
) -> Result<Vec<(TcpListener, SocketAddr)>> {
    for &addr in requested_addrs {
        ensure_loopback(addr)?;
    }

    let mut listeners = Vec::with_capacity(requested_addrs.len());
    for &addr in requested_addrs {
        let listener = TcpListener::bind(addr)
            .await
            .map_err(|error| bind_error(addr, error))?;
        let listening_addr = listener
            .local_addr()
            .map_err(|source| DiscussError::ServerBindError { addr, source })?;
        listeners.push((listener, listening_addr));
    }

    Ok(listeners)
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
    let mut listeners = bind_loopback_listeners(&[addr]).await?;
    let (listener, listening_addr) = listeners
        .pop()
        .expect("one requested listener should be returned");
    on_ready(listening_addr);

    serve_listener(listener, app_state, shutdown).await
}

pub async fn serve_listener<F>(
    listener: TcpListener,
    app_state: AppState,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let listening_addr = listener
        .local_addr()
        .map_err(|source| DiscussError::ServerBindError {
            addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            source,
        })?;
    ensure_loopback(listening_addr)?;

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
        .map_err(|source| DiscussError::ServerBindError {
            addr: listening_addr,
            source,
        })
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
        .route("/api/version", get(get_api_version))
        .route("/api/files/{id}/raw", get(get_api_file_raw))
        .route("/api/files/{id}/blocks", get(get_api_file_blocks))
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
        .route("/api/anchors/resolve", post(post_api_anchors_resolve))
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
        .route(
            "/api/pr/import",
            post(post_api_pr_import).layer(DefaultBodyLimit::max(MAX_IMPORT_BYTES)),
        )
        .route(
            "/api/pr/files/{id}/viewed",
            post(post_api_pr_file_viewed).delete(delete_api_pr_file_viewed),
        )
        .route("/api/pr/prepare", post(post_api_pr_prepare))
        .route("/api/pr/summary", post(post_api_pr_summary))
        .route(
            "/api/pr/draft",
            get(get_api_pr_draft).post(post_api_pr_draft),
        )
        .route("/api/pr/confirm", post(post_api_pr_confirm))
        .route("/api/pr/cancel", post(post_api_pr_cancel))
        .route("/api/pr/publish", post(post_api_pr_publish))
        .route(
            "/api/pr/publication-result",
            post(post_api_pr_publication_result),
        )
        .route("/files/{id}", get(get_html_file))
        .route("/files/{id}/assets/{*path}", get(get_html_asset))
        .route("/assets/mermaid.min.js", get(get_mermaid_js))
        .route("/assets/mermaid-shim.js", get(get_mermaid_shim_js))
        .route("/assets/discuss-inspect.js", get(get_discuss_inspect_js))
        .route_layer(middleware::from_fn(reject_cross_origin_mutations))
        .route_layer(middleware::from_fn_with_state(
            app_state.clone(),
            reject_pr_thread_mutations,
        ))
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

async fn reject_cross_origin_mutations(request: Request<Body>, next: Next) -> Response {
    let method = request.method();
    if matches!(
        *method,
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    ) {
        return next.run(request).await;
    }

    let headers = request.headers();
    let origin = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|value| value.to_str().ok());
    let browser_request = headers.contains_key("sec-fetch-site");
    let expected_origin = headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .and_then(loopback_origin_from_host);
    let allowed = match origin {
        Some(origin) => expected_origin.as_deref() == Some(origin),
        None => !browser_request,
    };
    if !allowed {
        return api_error_response(
            StatusCode::FORBIDDEN,
            "cross_origin_request",
            "browser mutations must originate from the Discuss UI origin",
        );
    }

    next.run(request).await
}

fn loopback_origin_from_host(host: &str) -> Option<String> {
    let url = url::Url::parse(&format!("http://{host}")).ok()?;
    (url.host_str() == Some("127.0.0.1")
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none())
    .then(|| url.origin().ascii_serialization())
}

async fn reject_pr_thread_mutations(
    AxumState(app_state): AxumState<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if request.uri().path().starts_with("/api/threads") && app_state.pr_mutations_locked() {
        return api_error_response(
            StatusCode::CONFLICT,
            "pr_publication_locked",
            "thread mutations are disabled while PR publication is pending or complete",
        );
    }
    next.run(request).await
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
    let read_only = matches!(
        *request.method(),
        axum::http::Method::GET | axum::http::Method::HEAD | axum::http::Method::OPTIONS
    );
    // A successful PR result remains visible for two seconds before shared
    // demo shutdown. Freeze every scenario during that grace period. The
    // Done and the authenticated publication-result callback may retry local
    // transcript emission after failure; their finalization claims still
    // reject a different demo scenario owner.
    if app_state.done_started()
        && !read_only
        && !matches!(
            request.uri().path(),
            "/api/done" | "/api/pr/publication-result"
        )
    {
        return api_error_response(
            StatusCode::CONFLICT,
            "review_complete",
            "this review session is already complete",
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
