use std::future::pending;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use discuss::assets;
use discuss::state::{State, Thread, ThreadId, ThreadKind};
use discuss::{serve, AppState, BroadcastEvent, DiscussError, EventBus, EventEmitter, EventKind};
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};

#[tokio::test]
async fn get_root_renders_template_and_shutdown_completes() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_markdown_source("# Review Plan\n\nBody text.");
    let mut shutdown_rx = app_state.subscribe_shutdown();
    let (shutdown_tx, shutdown_rx_signal) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx_signal.await;
    }));

    wait_for_server(addr).await;

    let response = get_root(addr).await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(response
        .to_ascii_lowercase()
        .contains("content-type: text/html; charset=utf-8"));
    assert!(doc_content(response_body(&response)).contains("<h1>Review Plan</h1>"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), shutdown_rx.changed())
        .await
        .expect("shutdown signal within timeout")
        .expect("shutdown sender still active");
    assert!(*shutdown_rx.borrow());

    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn shutdown_allows_started_request_to_complete() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_markdown_source("# Started Request");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let mut stream = TcpStream::connect(addr)
        .await
        .expect("connect before shutdown");
    stream
        .write_all(b"GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .await
        .expect("write request before shutdown");
    sleep(Duration::from_millis(20)).await;

    shutdown_tx.send(()).expect("send shutdown signal");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read response");
    assert!(response.starts_with("HTTP/1.1 200"));

    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn get_root_seeds_current_state_for_reload() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_markdown_source("# State Seed");
    {
        let mut state = app_state
            .state
            .write()
            .expect("state lock should not be poisoned");
        state.add_thread(thread("u-one", 1));
        state.add_thread(thread("u-two", 4));
    }
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_root(addr).await;
    let initial_state = initial_state_script(response_body(&response));

    assert!(initial_state.contains("\"u-one\""));
    assert!(initial_state.contains("\"u-two\""));
    assert!(doc_content(response_body(&response)).contains("<h1>State Seed</h1>"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn get_api_state_returns_empty_snapshot_json() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_path(addr, "/api/state").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(
        response_json(&response),
        json!({
            "threads": [],
            "replies": {},
            "takes": {},
            "resolutions": {},
            "drafts": {
                "newThread": {},
                "followup": {}
            }
        })
    );

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn get_api_state_returns_seeded_threads() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process();
    {
        let mut state = app_state
            .state
            .write()
            .expect("state lock should not be poisoned");
        state.add_thread(thread("u-state", 2));
    }
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_path(addr, "/api/state").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    let state = response_json(&response);

    assert_eq!(state["threads"][0]["id"], "u-state");
    assert_eq!(state["threads"][0]["anchorStart"], 2);
    assert_eq!(state["threads"][0]["text"], "thread u-state");

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn get_api_events_streams_published_broadcast_event() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process();
    let bus = app_state.bus.clone();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let mut stream = open_get_path(addr, "/api/events").await;
    let headers = read_until(&mut stream, "\r\n\r\n").await;
    assert!(headers.starts_with("HTTP/1.1 200"));
    assert_sse_headers(&headers);

    bus.publish(BroadcastEvent {
        kind: "thread.created".to_string(),
        payload: json!({ "threadId": "u-1" }),
    });

    let event = read_until(&mut stream, "\n\n").await;
    assert!(event.contains("event: thread.created"));
    assert!(event.contains("data: {\"threadId\":\"u-1\"}"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn api_events_disconnect_does_not_break_new_connections() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process();
    let bus = app_state.bus.clone();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let mut first = open_get_path(addr, "/api/events").await;
    let headers = read_until(&mut first, "\r\n\r\n").await;
    assert_sse_headers(&headers);
    drop(first);

    let mut second = open_get_path(addr, "/api/events").await;
    let headers = read_until(&mut second, "\r\n\r\n").await;
    assert_sse_headers(&headers);

    bus.publish(BroadcastEvent {
        kind: "take.added".to_string(),
        payload: json!({ "threadId": "u-2" }),
    });

    let event = read_until(&mut second, "\n\n").await;
    assert!(event.contains("event: take.added"));
    assert!(event.contains("data: {\"threadId\":\"u-2\"}"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn api_events_stream_ends_cleanly_on_shutdown() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let mut stream = open_get_path(addr, "/api/events").await;
    let headers = read_until(&mut stream, "\r\n\r\n").await;
    assert_sse_headers(&headers);

    shutdown_tx.send(()).expect("send shutdown signal");
    let mut response_tail = String::new();
    timeout(
        Duration::from_secs(1),
        stream.read_to_string(&mut response_tail),
    )
    .await
    .expect("sse stream closes within timeout")
    .expect("read sse response tail");

    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_threads_creates_thread_and_emits_events() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    let bus = Arc::new(EventBus::new(16));
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let emitter = Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone())));
    let app_state = AppState::new(state.clone(), bus, emitter);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let mut sse = open_get_path(addr, "/api/events").await;
    let headers = read_until(&mut sse, "\r\n\r\n").await;
    assert_sse_headers(&headers);

    let response = post_json_path(
        addr,
        "/api/threads",
        r#"{"anchorStart":2,"anchorEnd":4,"snippet":"selected text","text":"Needs clarification"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["id"], "u-1");
    assert!(body["createdAt"].as_str().is_some());

    let snapshot = state
        .read()
        .expect("state lock should not be poisoned")
        .snapshot();
    assert_eq!(snapshot.threads.len(), 1);
    assert_eq!(snapshot.threads[0].id, ThreadId("u-1".to_string()));
    assert_eq!(snapshot.threads[0].anchor_start, 2);
    assert_eq!(snapshot.threads[0].anchor_end, 4);
    assert_eq!(snapshot.threads[0].snippet, "selected text");
    assert_eq!(snapshot.threads[0].text, "Needs clarification");

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: thread.created"));
    assert!(sse_event.contains("\"id\":\"u-1\""));
    assert!(sse_event.contains("\"anchorStart\":2"));
    assert!(sse_event.contains("\"text\":\"Needs clarification\""));

    let stdout = stdout_string(&stdout);
    assert_eq!(stdout.lines().count(), 1);
    let emitted: Value = serde_json::from_str(stdout.trim_end()).expect("stdout event JSON");
    assert_eq!(emitted["kind"], EventKind::ThreadCreated.to_string());
    assert_eq!(emitted["payload"]["id"], "u-1");
    assert_eq!(emitted["payload"]["anchorStart"], 2);
    assert_eq!(emitted["payload"]["anchorEnd"], 4);
    assert_eq!(emitted["payload"]["snippet"], "selected text");
    assert_eq!(emitted["payload"]["text"], "Needs clarification");
    assert_eq!(emitted["payload"]["createdAt"], body["createdAt"]);

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_threads_returns_structured_400_for_bad_json() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/threads", r#"{"anchorStart":2"#).await;
    assert!(response.starts_with("HTTP/1.1 400"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "bad_request");
    assert!(body["error"]["message"].as_str().is_some());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_thread_replies_appends_reply_and_emits_events() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    let thread_id = ThreadId("u-reply".to_string());
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-reply", 2));
    }
    let bus = Arc::new(EventBus::new(16));
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let emitter = Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone())));
    let app_state = AppState::new(state.clone(), bus, emitter);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let mut sse = open_get_path(addr, "/api/events").await;
    let headers = read_until(&mut sse, "\r\n\r\n").await;
    assert_sse_headers(&headers);

    let response = post_json_path(
        addr,
        "/api/threads/u-reply/replies",
        r#"{"text":"Follow-up question"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["id"], "r-1");
    assert_eq!(body["threadId"], "u-reply");
    assert_eq!(body["text"], "Follow-up question");
    assert!(body["createdAt"].as_str().is_some());

    let snapshot = state
        .read()
        .expect("state lock should not be poisoned")
        .snapshot();
    let replies = snapshot
        .replies
        .get(&thread_id)
        .expect("thread should have replies");
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].id, "r-1");
    assert_eq!(replies[0].thread_id, thread_id);
    assert_eq!(replies[0].text, "Follow-up question");

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: reply.added"));
    assert!(sse_event.contains("\"id\":\"r-1\""));
    assert!(sse_event.contains("\"threadId\":\"u-reply\""));
    assert!(sse_event.contains("\"text\":\"Follow-up question\""));

    let stdout = stdout_string(&stdout);
    assert_eq!(stdout.lines().count(), 1);
    let emitted: Value = serde_json::from_str(stdout.trim_end()).expect("stdout event JSON");
    assert_eq!(emitted["kind"], EventKind::ReplyAdded.to_string());
    assert_eq!(emitted["payload"]["id"], "r-1");
    assert_eq!(emitted["payload"]["threadId"], "u-reply");
    assert_eq!(emitted["payload"]["text"], "Follow-up question");
    assert_eq!(emitted["payload"]["createdAt"], body["createdAt"]);

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_thread_replies_returns_structured_404_for_unknown_thread() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response =
        post_json_path(addr, "/api/threads/missing/replies", r#"{"text":"Reply"}"#).await;
    assert!(response.starts_with("HTTP/1.1 404"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "not_found");
    assert!(body["error"]["message"]
        .as_str()
        .expect("message string")
        .contains("missing"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_thread_replies_returns_structured_400_for_empty_text() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process();
    {
        let mut state = app_state
            .state
            .write()
            .expect("state lock should not be poisoned");
        state.add_thread(thread("u-reply", 2));
    }
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/threads/u-reply/replies", r#"{"text":"   "}"#).await;
    assert!(response.starts_with("HTTP/1.1 400"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "validation_error");
    assert!(body["error"]["message"]
        .as_str()
        .expect("message string")
        .contains("text"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_threads_returns_structured_400_for_missing_fields() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/threads", r#"{"anchorStart":2}"#).await;
    assert!(response.starts_with("HTTP/1.1 400"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "bad_request");
    assert!(body["error"]["message"]
        .as_str()
        .expect("message string")
        .contains("anchorEnd"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn busy_port_maps_to_port_in_use() {
    let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind busy listener");
    let addr = listener.local_addr().expect("busy listener addr");

    let error = serve(addr, AppState::for_process(), pending())
        .await
        .expect_err("busy port should fail");

    assert!(matches!(
        error,
        DiscussError::PortInUse { port } if port == addr.port()
    ));
}

#[tokio::test]
async fn rejects_non_loopback_bind_addr() {
    let addr = SocketAddr::from(([0, 0, 0, 0], 0));

    let error = serve(addr, AppState::for_process(), pending())
        .await
        .expect_err("public bind addr should fail");

    assert!(matches!(
        error,
        DiscussError::ServerBindError { addr: rejected, .. } if rejected == addr
    ));
}

#[tokio::test]
async fn get_mermaid_js_asset_returns_bundled_bytes_with_cache_headers() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_path(addr, "/assets/mermaid.min.js").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_js_headers(&response);
    assert!(response_body(&response).starts_with(&assets::mermaid_js()[..20]));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn get_mermaid_shim_asset_returns_bundled_shim() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_path(addr, "/assets/mermaid-shim.js").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_js_headers(&response);
    let body = response_body(&response);
    assert!(body.contains("language-mermaid"));
    assert!(body.contains("/assets/mermaid.min.js"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn unknown_asset_path_returns_404() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_path(addr, "/assets/nope.js").await;
    assert!(response.starts_with("HTTP/1.1 404"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

fn free_loopback_addr() -> SocketAddr {
    let listener = StdTcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("allocate free port");
    listener.local_addr().expect("free listener addr")
}

async fn wait_for_server(addr: SocketAddr) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);

    loop {
        match TcpStream::connect(addr).await {
            Ok(_) => return,
            Err(error) if tokio::time::Instant::now() < deadline => {
                let _ = error;
                sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("server did not start at {addr}: {error}"),
        }
    }
}

async fn get_root(addr: SocketAddr) -> String {
    get_path(addr, "/").await
}

async fn get_path(addr: SocketAddr, path: &str) -> String {
    let mut stream = open_get_path(addr, path).await;

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read response");
    response
}

async fn post_json_path(addr: SocketAddr, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect to server");
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .await
        .expect("read response");
    response
}

async fn open_get_path(addr: SocketAddr, path: &str) -> TcpStream {
    let mut stream = TcpStream::connect(addr).await.expect("connect to server");
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");

    stream
}

#[derive(Clone, Debug)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("stdout capture lock should not be poisoned")
            .extend_from_slice(buf);

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn stdout_string(stdout: &Arc<Mutex<Vec<u8>>>) -> String {
    let bytes = stdout
        .lock()
        .expect("stdout capture lock should not be poisoned")
        .clone();

    String::from_utf8(bytes).expect("stdout capture should be utf-8")
}

fn assert_js_headers(response: &str) {
    let headers = response.to_ascii_lowercase();
    assert!(headers.contains("content-type: application/javascript"));
    assert!(headers.contains("cache-control: public, max-age=86400"));
}

fn assert_json_headers(response: &str) {
    let headers = response.to_ascii_lowercase();
    assert!(headers.contains("content-type: application/json"));
}

fn assert_sse_headers(response: &str) {
    let headers = response.to_ascii_lowercase();
    assert!(headers.contains("content-type: text/event-stream"));
    assert!(headers.contains("cache-control: no-cache"));
}

async fn read_until(stream: &mut TcpStream, needle: &str) -> String {
    let mut response = Vec::new();
    let needle = needle.as_bytes();

    loop {
        let mut chunk = [0; 1024];
        let read = timeout(Duration::from_secs(1), stream.read(&mut chunk))
            .await
            .expect("read before timeout")
            .expect("read response");
        if read == 0 {
            break;
        }

        response.extend_from_slice(&chunk[..read]);
        if response
            .windows(needle.len())
            .any(|window| window == needle)
        {
            break;
        }
    }

    String::from_utf8(response).expect("response should be utf-8")
}

fn timestamp(second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 23, 2, 30, second)
        .single()
        .expect("valid timestamp")
}

fn thread(id: &str, anchor_start: usize) -> Thread {
    Thread {
        id: ThreadId(id.to_string()),
        anchor_start,
        anchor_end: anchor_start + 1,
        snippet: format!("snippet {id}"),
        breadcrumb: "Overview".to_string(),
        text: format!("thread {id}"),
        created_at: timestamp(0),
        kind: ThreadKind::User,
    }
}

fn response_body(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .expect("http response should contain a body separator")
}

fn response_json(response: &str) -> Value {
    serde_json::from_str(response_body(response)).expect("response body should be JSON")
}

fn doc_content(body: &str) -> &str {
    let open = "<section id=\"doc-content\">";
    let close = "</section>";
    let start = body.find(open).expect("doc-content start") + open.len();
    let end = body[start..].find(close).expect("doc-content end") + start;

    &body[start..end]
}

fn initial_state_script(body: &str) -> &str {
    let open = "<script id=\"discuss-initial-state\">";
    let close = "</script>";
    let start = body.find(open).expect("initial-state script start") + open.len();
    let end = body[start..].find(close).expect("initial-state script end") + start;

    &body[start..end]
}
