use std::fs;
use std::future::pending;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use discuss::assets;
use discuss::state::{
    Draft, NewThreadDraftKey, Resolution, State, Thread, ThreadId, ThreadKind, default_file_id,
};
use discuss::{
    AppState, BroadcastEvent, DiscussError, EventBus, EventEmitter, EventKind, Transcript, serve,
    serve_with_ready,
};
use serde_json::{Value, json};
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
    assert!(
        response
            .to_ascii_lowercase()
            .contains("content-type: text/html; charset=utf-8")
    );
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
async fn get_root_serves_v2_template_when_ui_query_param_set() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_markdown_source("# Hello v2");
    {
        let mut state = app_state
            .state
            .write()
            .expect("state lock should not be poisoned");
        state.add_thread(thread("u-v2", 1));
    }
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_path(addr, "/?ui=v2").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    let body = response_body(&response);
    assert!(body.contains("/assets/preact.umd.js"));
    assert!(body.contains("/assets/preact-hooks.umd.js"));
    assert!(body.contains("/assets/htm.umd.js"));
    assert!(body.contains(r#"id="app""#));
    assert!(body.contains(r#"id="discuss-initial-state""#));
    assert!(body.contains(r#"id="discuss-rendered-files""#));
    assert!(body.contains("\"u-v2\""));
    assert!(body.contains("__DISCUSS_RENDERED_FILES__"));
    assert!(
        body.contains("Hello v2"),
        "rendered markdown HTML should be seeded into v2 page"
    );

    let v1_response = get_path(addr, "/").await;
    assert!(v1_response.starts_with("HTTP/1.1 200"));
    let v1_body = response_body(&v1_response);
    assert!(doc_content(v1_body).contains("<h1>Hello v2</h1>"));
    assert!(!v1_body.contains("/assets/preact.umd.js"));

    shutdown_tx.send(()).expect("send shutdown signal");
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
            },
            "files": []
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
async fn post_api_heartbeat_updates_timestamp_silently() {
    let addr = free_loopback_addr();
    let bus = Arc::new(EventBus::new(16));
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        bus,
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    );
    let before = app_state
        .last_heartbeat_at()
        .expect("heartbeat timestamp should be readable");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state.clone(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let mut sse = open_get_path(addr, "/api/events").await;
    let headers = read_until(&mut sse, "\r\n\r\n").await;
    assert_sse_headers(&headers);

    sleep(Duration::from_millis(2)).await;
    let response = post_json_path(addr, "/api/heartbeat", "").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(response_json(&response), json!({ "ok": true }));
    let first = app_state
        .last_heartbeat_at()
        .expect("heartbeat timestamp should be readable after POST");
    assert!(first > before);

    sleep(Duration::from_millis(2)).await;
    let response = post_json_path(addr, "/api/heartbeat", "").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    let second = app_state
        .last_heartbeat_at()
        .expect("heartbeat timestamp should be readable after second POST");
    assert!(second > first);

    assert!(stdout_string(&stdout).is_empty());
    assert_no_sse_event(&mut sse).await;

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn idle_timer_emits_prompt_suggest_done_once_per_idle_window() {
    let addr = free_loopback_addr();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_idle_timeout_secs(1);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let events = wait_for_stdout_events(&stdout, 1, Duration::from_secs(3)).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], EventKind::PromptSuggestDone.to_string());
    assert!(
        events[0]["payload"]["idle_for_secs"]
            .as_u64()
            .expect("idle_for_secs should be an integer")
            >= 1
    );

    let events = wait_for_stdout_events(&stdout, 2, Duration::from_secs(3)).await;
    assert_eq!(events.len(), 2);
    assert!(
        events
            .iter()
            .all(|event| event["kind"] == EventKind::PromptSuggestDone.to_string())
    );

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn heartbeat_prevents_idle_prompt_suggest_done_event() {
    let addr = free_loopback_addr();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_idle_timeout_secs(1);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let heartbeat = tokio::spawn(async move {
        loop {
            let response = post_json_path(addr, "/api/heartbeat", "").await;
            assert!(response.starts_with("HTTP/1.1 200"));
            sleep(Duration::from_millis(500)).await;
        }
    });

    sleep(Duration::from_secs(3)).await;
    heartbeat.abort();
    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn idle_timeout_zero_disables_prompt_suggest_done_event() {
    let addr = free_loopback_addr();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_idle_timeout_secs(0);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;
    sleep(Duration::from_millis(1200)).await;

    assert!(stdout_string(&stdout).is_empty());

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
async fn post_api_threads_round_trips_line_range_for_code_block_threads() {
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

    let response = post_json_path(
        addr,
        "/api/threads",
        r#"{"anchorStart":7,"anchorEnd":7,"snippet":"fn main()","text":"why?","lineRange":{"start":3,"end":5}}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));

    let snapshot = state
        .read()
        .expect("state lock should not be poisoned")
        .snapshot();
    assert_eq!(
        snapshot.threads[0].line_range,
        Some(discuss::state::LineRange { start: 3, end: 5 })
    );

    let stdout_text = stdout_string(&stdout);
    let emitted: Value = serde_json::from_str(stdout_text.trim_end()).expect("stdout event JSON");
    assert_eq!(emitted["payload"]["lineRange"]["start"], 3);
    assert_eq!(emitted["payload"]["lineRange"]["end"], 5);

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_threads_rejects_invalid_line_range() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = post_json_path(
        addr,
        "/api/threads",
        r#"{"anchorStart":7,"anchorEnd":7,"snippet":"x","text":"y","lineRange":{"start":5,"end":3}}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 400"));
    assert!(response.contains("validation_error"));

    let zero_start = post_json_path(
        addr,
        "/api/threads",
        r#"{"anchorStart":7,"anchorEnd":7,"snippet":"x","text":"y","lineRange":{"start":0,"end":2}}"#,
    )
    .await;
    assert!(zero_start.starts_with("HTTP/1.1 400"));

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
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("missing")
    );

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
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("text")
    );

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_thread_takes_appends_take_and_emits_events() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    let thread_id = ThreadId("u-take".to_string());
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-take", 2));
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
        "/api/threads/u-take/takes",
        r#"{"text":"Agent recommends tightening this section"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["id"], "t-1");
    assert_eq!(body["threadId"], "u-take");
    assert_eq!(body["text"], "Agent recommends tightening this section");
    assert!(body["createdAt"].as_str().is_some());

    let snapshot = state
        .read()
        .expect("state lock should not be poisoned")
        .snapshot();
    let takes = snapshot
        .takes
        .get(&thread_id)
        .expect("thread should have takes");
    assert_eq!(takes.len(), 1);
    assert_eq!(takes[0].id, "t-1");
    assert_eq!(takes[0].thread_id, thread_id);
    assert_eq!(takes[0].text, "Agent recommends tightening this section");

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: take.added"));
    assert!(sse_event.contains("\"id\":\"t-1\""));
    assert!(sse_event.contains("\"threadId\":\"u-take\""));
    assert!(sse_event.contains("\"text\":\"Agent recommends tightening this section\""));

    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_thread_takes_returns_structured_404_for_unknown_thread() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/threads/missing/takes", r#"{"text":"Take"}"#).await;
    assert!(response.starts_with("HTTP/1.1 404"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "not_found");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("missing")
    );

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_thread_takes_returns_structured_400_for_empty_text() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process();
    {
        let mut state = app_state
            .state
            .write()
            .expect("state lock should not be poisoned");
        state.add_thread(thread("u-take", 2));
    }
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/threads/u-take/takes", r#"{"text":"   "}"#).await;
    assert!(response.starts_with("HTTP/1.1 400"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "validation_error");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("text")
    );

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_thread_resolve_sets_and_replaces_resolution_and_emits_events() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    let thread_id = ThreadId("u-resolve".to_string());
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-resolve", 2));
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
        "/api/threads/u-resolve/resolve",
        r#"{"decision":"accepted"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["decision"], "accepted");
    assert!(body["resolvedAt"].as_str().is_some());

    let snapshot = state
        .read()
        .expect("state lock should not be poisoned")
        .snapshot();
    assert_eq!(
        snapshot.resolutions[&thread_id].decision,
        Some("accepted".to_string())
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: thread.resolved"));
    assert!(sse_event.contains("\"threadId\":\"u-resolve\""));
    assert!(sse_event.contains("\"decision\":\"accepted\""));

    let stdout_after_first = stdout_string(&stdout);
    assert_eq!(stdout_after_first.lines().count(), 1);
    let emitted: Value =
        serde_json::from_str(stdout_after_first.trim_end()).expect("stdout event JSON");
    assert_eq!(emitted["kind"], EventKind::ThreadResolved.to_string());
    assert_eq!(emitted["payload"]["threadId"], "u-resolve");
    assert_eq!(emitted["payload"]["resolution"]["decision"], "accepted");
    assert_eq!(
        emitted["payload"]["resolution"]["resolvedAt"],
        body["resolvedAt"]
    );

    let response = post_json_path(
        addr,
        "/api/threads/u-resolve/resolve",
        r#"{"decision":"revised"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["decision"], "revised");

    let snapshot = state
        .read()
        .expect("state lock should not be poisoned")
        .snapshot();
    assert_eq!(
        snapshot.resolutions[&thread_id].decision,
        Some("revised".to_string())
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: thread.resolved"));
    assert!(sse_event.contains("\"threadId\":\"u-resolve\""));
    assert!(sse_event.contains("\"decision\":\"revised\""));

    let stdout_after_second = stdout_string(&stdout);
    assert_eq!(stdout_after_second.lines().count(), 2);
    let emitted: Value = serde_json::from_str(
        stdout_after_second
            .lines()
            .nth(1)
            .expect("second stdout event"),
    )
    .expect("stdout event JSON");
    assert_eq!(emitted["kind"], EventKind::ThreadResolved.to_string());
    assert_eq!(emitted["payload"]["threadId"], "u-resolve");
    assert_eq!(emitted["payload"]["resolution"]["decision"], "revised");
    assert_eq!(
        emitted["payload"]["resolution"]["resolvedAt"],
        body["resolvedAt"]
    );

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_thread_unresolve_clears_resolution_idempotently_and_emits_events() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    let thread_id = ThreadId("u-unresolve".to_string());
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-unresolve", 2));
        state_guard.set_resolution(
            thread_id.clone(),
            Resolution {
                decision: Some("accepted".to_string()),
                resolved_at: timestamp(1),
            },
        );
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

    let response = post_json_path(addr, "/api/threads/u-unresolve/unresolve", "").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(response_json(&response), json!({ "ok": true }));
    assert!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .resolutions
            .is_empty()
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: thread.unresolved"));
    assert!(sse_event.contains("\"threadId\":\"u-unresolve\""));

    let stdout_after_first = stdout_string(&stdout);
    assert_eq!(stdout_after_first.lines().count(), 1);
    let emitted: Value =
        serde_json::from_str(stdout_after_first.trim_end()).expect("stdout event JSON");
    assert_eq!(emitted["kind"], EventKind::ThreadUnresolved.to_string());
    assert_eq!(emitted["payload"]["threadId"], "u-unresolve");

    let response = post_json_path(addr, "/api/threads/u-unresolve/unresolve", "").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(response_json(&response), json!({ "ok": true }));
    assert!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .resolutions
            .is_empty()
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: thread.unresolved"));
    assert!(sse_event.contains("\"threadId\":\"u-unresolve\""));

    let stdout_after_second = stdout_string(&stdout);
    assert_eq!(stdout_after_second.lines().count(), 2);
    let emitted: Value = serde_json::from_str(
        stdout_after_second
            .lines()
            .nth(1)
            .expect("second stdout event"),
    )
    .expect("stdout event JSON");
    assert_eq!(emitted["kind"], EventKind::ThreadUnresolved.to_string());
    assert_eq!(emitted["payload"]["threadId"], "u-unresolve");

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn delete_api_thread_soft_deletes_user_thread_and_emits_events() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-delete", 2));
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

    let response = delete_path(addr, "/api/threads/u-delete").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(response_json(&response), json!({ "ok": true }));
    assert!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .threads
            .is_empty()
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: thread.deleted"));
    assert!(sse_event.contains("\"threadId\":\"u-delete\""));

    let stdout = stdout_string(&stdout);
    assert_eq!(stdout.lines().count(), 1);
    let emitted: Value = serde_json::from_str(stdout.trim_end()).expect("stdout event JSON");
    assert_eq!(emitted["kind"], EventKind::ThreadDeleted.to_string());
    assert_eq!(emitted["payload"]["threadId"], "u-delete");

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn delete_api_thread_rejects_prepopulated_thread_without_events() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread_with_kind("p-delete", 2, ThreadKind::Prepopulated));
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

    let response = delete_path(addr, "/api/threads/p-delete").await;
    assert!(response.starts_with("HTTP/1.1 403"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "prepopulated_thread");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("p-delete")
    );
    assert_eq!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .threads
            .len(),
        1
    );

    assert_no_sse_event(&mut sse).await;
    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn delete_api_thread_returns_structured_404_for_unknown_thread() {
    let addr = free_loopback_addr();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = delete_path(addr, "/api/threads/missing").await;
    assert!(response.starts_with("HTTP/1.1 404"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "not_found");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("missing")
    );
    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_drafts_new_thread_upserts_replaces_and_emits_events() {
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
        "/api/drafts/new-thread",
        r#"{"anchorStart":2,"anchorEnd":4,"text":"First draft"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["scope"], "newThread");
    assert_eq!(body["anchorStart"], 2);
    assert_eq!(body["anchorEnd"], 4);
    assert_eq!(body["text"], "First draft");
    assert!(body["updatedAt"].as_str().is_some());

    assert_eq!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .drafts
            .new_thread[&NewThreadDraftKey::new(default_file_id(), 2, 4)]
            .text,
        "First draft"
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: draft.updated"));
    assert!(sse_event.contains("\"scope\":\"newThread\""));
    assert!(sse_event.contains("\"anchorStart\":2"));
    assert!(sse_event.contains("\"anchorEnd\":4"));
    assert!(sse_event.contains("\"text\":\"First draft\""));

    assert!(stdout_string(&stdout).is_empty());

    let response = post_json_path(
        addr,
        "/api/drafts/new-thread",
        r#"{"anchorStart":2,"anchorEnd":4,"text":"Revised draft"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["text"], "Revised draft");
    assert_eq!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .drafts
            .new_thread[&NewThreadDraftKey::new(default_file_id(), 2, 4)]
            .text,
        "Revised draft"
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: draft.updated"));
    assert!(sse_event.contains("\"text\":\"Revised draft\""));

    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_drafts_new_thread_whitespace_text_clears_draft() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.upsert_new_thread_draft(
            NewThreadDraftKey::new(default_file_id(), 5, 7),
            draft("stashed draft", 1),
        );
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
        "/api/drafts/new-thread",
        r#"{"anchorStart":5,"anchorEnd":7,"text":"   "}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(response_json(&response), json!({ "ok": true }));
    assert!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .drafts
            .new_thread
            .is_empty()
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: draft.cleared"));
    assert!(sse_event.contains("\"scope\":\"newThread\""));
    assert!(sse_event.contains("\"anchorStart\":5"));
    assert!(sse_event.contains("\"anchorEnd\":7"));

    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn delete_api_drafts_new_thread_clears_idempotently_and_emits_events() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.upsert_new_thread_draft(
            NewThreadDraftKey::new(default_file_id(), 8, 9),
            draft("delete me", 1),
        );
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

    let response = delete_json_path(
        addr,
        "/api/drafts/new-thread",
        r#"{"anchorStart":8,"anchorEnd":9}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(response_json(&response), json!({ "ok": true }));
    assert!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .drafts
            .new_thread
            .is_empty()
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: draft.cleared"));
    assert!(sse_event.contains("\"anchorStart\":8"));
    assert!(sse_event.contains("\"anchorEnd\":9"));

    let response = delete_json_path(
        addr,
        "/api/drafts/new-thread",
        r#"{"anchorStart":8,"anchorEnd":9}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(response_json(&response), json!({ "ok": true }));

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: draft.cleared"));
    assert!(sse_event.contains("\"anchorStart\":8"));
    assert!(sse_event.contains("\"anchorEnd\":9"));

    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_drafts_followup_upserts_replaces_and_emits_events() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    let thread_id = ThreadId("u-followup".to_string());
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-followup", 2));
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
        "/api/drafts/followup",
        r#"{"threadId":"u-followup","text":"First follow-up"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["scope"], "followup");
    assert_eq!(body["threadId"], "u-followup");
    assert_eq!(body["text"], "First follow-up");
    assert!(body["updatedAt"].as_str().is_some());

    assert_eq!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .drafts
            .followup[&thread_id]
            .text,
        "First follow-up"
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: draft.updated"));
    assert!(sse_event.contains("\"scope\":\"followup\""));
    assert!(sse_event.contains("\"threadId\":\"u-followup\""));
    assert!(sse_event.contains("\"text\":\"First follow-up\""));

    assert!(stdout_string(&stdout).is_empty());

    let response = post_json_path(
        addr,
        "/api/drafts/followup",
        r#"{"threadId":"u-followup","text":"Revised follow-up"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["text"], "Revised follow-up");
    assert_eq!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .drafts
            .followup[&thread_id]
            .text,
        "Revised follow-up"
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: draft.updated"));
    assert!(sse_event.contains("\"text\":\"Revised follow-up\""));

    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_drafts_followup_whitespace_text_clears_draft() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    let thread_id = ThreadId("u-followup".to_string());
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-followup", 5));
        state_guard.upsert_followup_draft(thread_id.clone(), draft("stashed follow-up", 1));
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
        "/api/drafts/followup",
        r#"{"threadId":"u-followup","text":"   "}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(response_json(&response), json!({ "ok": true }));
    assert!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .drafts
            .followup
            .is_empty()
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: draft.cleared"));
    assert!(sse_event.contains("\"scope\":\"followup\""));
    assert!(sse_event.contains("\"threadId\":\"u-followup\""));

    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn delete_api_drafts_followup_clears_idempotently_and_emits_events() {
    let addr = free_loopback_addr();
    let state = State::new_shared();
    let thread_id = ThreadId("u-followup".to_string());
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-followup", 8));
        state_guard.upsert_followup_draft(thread_id, draft("delete me", 1));
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

    let response =
        delete_json_path(addr, "/api/drafts/followup", r#"{"threadId":"u-followup"}"#).await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(response_json(&response), json!({ "ok": true }));
    assert!(
        state
            .read()
            .expect("state lock should not be poisoned")
            .snapshot()
            .drafts
            .followup
            .is_empty()
    );

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: draft.cleared"));
    assert!(sse_event.contains("\"scope\":\"followup\""));
    assert!(sse_event.contains("\"threadId\":\"u-followup\""));

    let response =
        delete_json_path(addr, "/api/drafts/followup", r#"{"threadId":"u-followup"}"#).await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(response_json(&response), json!({ "ok": true }));

    let sse_event = read_until(&mut sse, "\n\n").await;
    assert!(sse_event.contains("event: draft.cleared"));
    assert!(sse_event.contains("\"threadId\":\"u-followup\""));

    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn api_drafts_followup_returns_structured_404_for_unknown_thread() {
    let addr = free_loopback_addr();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    );
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = post_json_path(
        addr,
        "/api/drafts/followup",
        r#"{"threadId":"missing","text":"Draft"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 404"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "not_found");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("missing")
    );

    let response =
        delete_json_path(addr, "/api/drafts/followup", r#"{"threadId":"missing"}"#).await;
    assert!(response.starts_with("HTTP/1.1 404"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "not_found");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("missing")
    );
    assert!(stdout_string(&stdout).is_empty());

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_drafts_new_thread_returns_structured_400_for_bad_json() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/drafts/new-thread", r#"{"anchorStart":2"#).await;
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
async fn post_api_thread_resolution_routes_return_structured_404_for_unknown_thread() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response =
        post_json_path(addr, "/api/threads/missing/resolve", r#"{"decision":null}"#).await;
    assert!(response.starts_with("HTTP/1.1 404"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "not_found");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("missing")
    );

    let response = post_json_path(addr, "/api/threads/missing/unresolve", "").await;
    assert!(response.starts_with("HTTP/1.1 404"));
    assert_json_headers(&response);
    let body = response_json(&response);
    assert_eq!(body["error"]["code"], "not_found");
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("missing")
    );

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
    assert!(
        body["error"]["message"]
            .as_str()
            .expect("message string")
            .contains("anchorEnd")
    );

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_done_emits_transcript_and_triggers_shutdown() {
    let addr = free_loopback_addr();
    let history_dir = tempfile::tempdir().expect("history tempdir");
    let state = State::new_shared();
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-done", 3));
    }
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        state,
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_source_path("docs/review:plan.md")
    .with_history_dir(history_dir.path())
    .with_idle_timeout_secs(0);
    let server = tokio::spawn(serve(addr, app_state, pending()));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/done", "").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(
        response_json(&response),
        json!({ "ok": true, "message": "transcript emitted" })
    );

    let followup_response = try_post_json_path(
        addr,
        "/api/threads",
        r#"{"anchorStart":1,"anchorEnd":1,"snippet":"late","text":"too late"}"#,
    )
    .await;
    if let Ok(response) = followup_response {
        assert!(
            response.starts_with("HTTP/1.1 503") || response.is_empty(),
            "expected 503 or closed connection during shutdown, got {response:?}"
        );
    }

    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");

    let stdout = stdout_string(&stdout);
    assert_eq!(stdout.lines().count(), 1);
    let emitted: Value = serde_json::from_str(stdout.trim_end()).expect("stdout event JSON");
    assert_eq!(emitted["kind"], EventKind::SessionDone.to_string());
    let transcript: Transcript =
        serde_json::from_value(emitted["payload"].clone()).expect("payload is transcript");
    assert_eq!(transcript.threads.len(), 1);
    assert_eq!(transcript.threads[0].id, ThreadId("u-done".to_string()));
    assert_eq!(transcript.threads[0].anchor_start, 3);

    let archive_path = single_json_file(&history_dir.path().join("review_plan"));
    let archived: Value = serde_json::from_str(
        &fs::read_to_string(archive_path).expect("history archive should be readable"),
    )
    .expect("history archive JSON");
    assert_eq!(archived, emitted["payload"]);
}

#[tokio::test]
async fn post_api_done_history_write_failure_still_succeeds_and_shutdown_exits() {
    let addr = free_loopback_addr();
    let tempdir = tempfile::tempdir().expect("history tempdir");
    let blocked_history_root = tempdir.path().join("not-a-directory");
    fs::write(&blocked_history_root, "blocks create_dir_all")
        .expect("blocking file should be created");
    let state = State::new_shared();
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-done", 3));
    }
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        state,
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_source_path("review.md")
    .with_history_dir(&blocked_history_root)
    .with_idle_timeout_secs(0);
    let server = tokio::spawn(serve(addr, app_state, pending()));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/done", "").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);
    assert_eq!(
        response_json(&response),
        json!({ "ok": true, "message": "transcript emitted" })
    );

    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");

    let stdout = stdout_string(&stdout);
    assert_eq!(stdout.lines().count(), 1);
    let emitted: Value = serde_json::from_str(stdout.trim_end()).expect("stdout event JSON");
    assert_eq!(emitted["kind"], EventKind::SessionDone.to_string());
    assert!(blocked_history_root.is_file());
}

#[tokio::test]
async fn post_api_done_no_save_emits_transcript_without_history_archive() {
    let addr = free_loopback_addr();
    let history_dir = tempfile::tempdir().expect("history tempdir");
    let state = State::new_shared();
    {
        let mut state_guard = state.write().expect("state lock should not be poisoned");
        state_guard.add_thread(thread("u-done", 3));
    }
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        state,
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_source_path("review.md")
    .with_history_dir(history_dir.path())
    .with_no_save(true)
    .with_idle_timeout_secs(0);
    let server = tokio::spawn(serve(addr, app_state, pending()));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/done", "").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_json_headers(&response);

    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");

    let stdout = stdout_string(&stdout);
    assert_eq!(stdout.lines().count(), 1);
    let emitted: Value = serde_json::from_str(stdout.trim_end()).expect("stdout event JSON");
    assert_eq!(emitted["kind"], EventKind::SessionDone.to_string());
    assert!(
        !history_dir.path().join("review").exists(),
        "no_save should suppress history archive writes"
    );
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
async fn serve_with_ready_reports_listener_address_after_bind() {
    let addr = free_loopback_addr();
    let (ready_tx, ready_rx) = oneshot::channel();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve_with_ready(
        addr,
        AppState::for_process(),
        async move {
            let _ = shutdown_rx.await;
        },
        move |listening_addr| {
            ready_tx
                .send(listening_addr)
                .expect("ready receiver should be active");
        },
    ));

    let listening_addr = timeout(Duration::from_secs(1), ready_rx)
        .await
        .expect("ready callback should run")
        .expect("ready callback should send address");
    assert_eq!(listening_addr, addr);

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
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
async fn get_preact_asset_returns_bundled_bytes_with_cache_headers() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_path(addr, "/assets/preact.umd.js").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_js_headers(&response);
    assert!(response_body(&response).starts_with(&assets::preact_js()[..20]));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn get_preact_hooks_asset_returns_bundled_bytes_with_cache_headers() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_path(addr, "/assets/preact-hooks.umd.js").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_js_headers(&response);
    assert!(response_body(&response).starts_with(&assets::preact_hooks_js()[..20]));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn get_htm_asset_returns_bundled_bytes_with_cache_headers() {
    let addr = free_loopback_addr();
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, AppState::for_process(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_path(addr, "/assets/htm.umd.js").await;
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_js_headers(&response);
    assert!(response_body(&response).starts_with(&assets::htm_js()[..20]));

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

fn single_json_file(dir: &Path) -> PathBuf {
    let mut entries = fs::read_dir(dir)
        .expect("history source directory should exist")
        .map(|entry| entry.expect("history entry should be readable").path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    entries.sort();

    assert_eq!(entries.len(), 1, "expected exactly one archive in {dir:?}");
    entries.remove(0)
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
    try_post_json_path(addr, path, body)
        .await
        .expect("POST request should succeed")
}

async fn try_post_json_path(addr: SocketAddr, path: &str, body: &str) -> io::Result<String> {
    let mut stream = TcpStream::connect(addr).await?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await?;

    let mut response = String::new();
    stream.read_to_string(&mut response).await?;

    Ok(response)
}

async fn delete_path(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect to server");
    let request = format!("DELETE {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
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

async fn delete_json_path(addr: SocketAddr, path: &str, body: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect to server");
    let request = format!(
        "DELETE {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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

async fn wait_for_stdout_events(
    stdout: &Arc<Mutex<Vec<u8>>>,
    expected_count: usize,
    max_wait: Duration,
) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + max_wait;

    loop {
        let output = stdout_string(stdout);
        let events = output
            .lines()
            .map(|line| serde_json::from_str(line).expect("stdout event should be JSON"))
            .collect::<Vec<Value>>();

        if events.len() >= expected_count {
            return events;
        }

        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {expected_count} stdout events; saw {}: {output}",
            events.len()
        );
        sleep(Duration::from_millis(25)).await;
    }
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

async fn assert_no_sse_event(stream: &mut TcpStream) {
    let mut chunk = [0; 128];
    let read = timeout(Duration::from_millis(100), stream.read(&mut chunk)).await;

    assert!(read.is_err(), "unexpected SSE bytes: {read:?}");
}

fn timestamp(second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 23, 2, 30, second)
        .single()
        .expect("valid timestamp")
}

fn thread(id: &str, anchor_start: usize) -> Thread {
    thread_with_kind(id, anchor_start, ThreadKind::User)
}

fn draft(text: &str, second: u32) -> Draft {
    Draft {
        text: text.to_string(),
        updated_at: timestamp(second),
    }
}

fn thread_with_kind(id: &str, anchor_start: usize, kind: ThreadKind) -> Thread {
    Thread {
        id: ThreadId(id.to_string()),
        file_id: default_file_id(),
        anchor_start,
        anchor_end: anchor_start + 1,
        snippet: format!("snippet {id}"),
        breadcrumb: "Overview".to_string(),
        text: format!("thread {id}"),
        created_at: timestamp(0),
        kind,
        line_range: None,
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
