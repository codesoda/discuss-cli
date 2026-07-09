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
    AppState, BroadcastEvent, DiscussError, EventBus, EventEmitter, EventKind, Transcript,
    VerdictConfig, VerdictOption, VerdictStyle, serve, serve_with_ready,
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
            "files": [],
            "drafts": {
                "newThread": {},
                "followup": {}
            },
            "sourceVersion": 0
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
async fn post_api_done_with_verdict_config_accepts_valid_body_and_emits_verdict() {
    let addr = free_loopback_addr();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_no_save(true)
    .with_idle_timeout_secs(0)
    .with_verdict_config(Some(test_verdict_config()));
    let server = tokio::spawn(serve(addr, app_state, pending()));

    wait_for_server(addr).await;

    let response = post_json_path(
        addr,
        "/api/done",
        r#"{"verdict":{"optionId":"approved","feedback":"  ship it  "}}"#,
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
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
    let events = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("stdout line should be JSON"))
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1, "expected one event, got {stdout}");
    assert_eq!(events[0]["kind"], "session.done");
    assert_eq!(events[0]["payload"]["verdict"]["optionId"], "approved");
    assert_eq!(events[0]["payload"]["verdict"]["label"], "Approve");
    assert_eq!(events[0]["payload"]["verdict"]["feedback"], "ship it");
    assert!(events[0]["payload"]["verdict"]["decidedAt"].is_string());
}

#[tokio::test]
async fn get_api_state_includes_configured_verdict_config() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process()
        .with_idle_timeout_secs(0)
        .with_verdict_config(Some(test_verdict_config()));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let state = response_json(&get_path(addr, "/api/state").await);
    assert_eq!(state["verdictConfig"]["prompt"], "Final verdict?");
    assert_eq!(state["verdictConfig"]["options"][0]["id"], "approved");
    assert_eq!(state["verdictConfig"]["options"][0]["style"], "positive");
    assert_eq!(
        state["verdictConfig"]["options"][1]["feedbackRequired"],
        true
    );

    shutdown_tx.send(()).expect("send shutdown");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_done_with_verdict_config_rejects_unknown_option_without_side_effects() {
    assert_verdict_rejection_is_noop_then_valid(
        r#"{"verdict":{"optionId":"missing","feedback":"nope"}}"#,
        "validation_error",
    )
    .await;
}

#[tokio::test]
async fn post_api_done_with_verdict_config_rejects_missing_body_without_side_effects() {
    assert_verdict_rejection_is_noop_then_valid("", "bad_request").await;
}

#[tokio::test]
async fn post_api_done_with_verdict_config_rejects_missing_required_feedback_without_side_effects()
{
    assert_verdict_rejection_is_noop_then_valid(
        r#"{"verdict":{"optionId":"declined","feedback":"   "}}"#,
        "validation_error",
    )
    .await;
}

#[tokio::test]
async fn post_api_done_without_verdict_config_ignores_stray_body() {
    let addr = free_loopback_addr();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_no_save(true)
    .with_idle_timeout_secs(0);
    let server = tokio::spawn(serve(addr, app_state, pending()));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/done", "not json at all").await;

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert_eq!(
        response_json(&response),
        json!({ "ok": true, "message": "transcript emitted" })
    );
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
    assert!(stdout_string(&stdout).contains("session.done"));
}

#[tokio::test]
async fn post_api_done_without_verdict_config_accepts_browser_empty_request() {
    let addr = free_loopback_addr();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_no_save(true)
    .with_idle_timeout_secs(0);
    let server = tokio::spawn(serve(addr, app_state, pending()));

    wait_for_server(addr).await;

    let response = post_path_no_body(addr, "/api/done").await;

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    assert_eq!(
        response_json(&response),
        json!({ "ok": true, "message": "transcript emitted" })
    );
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
    assert!(stdout_string(&stdout).contains("session.done"));
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

#[tokio::test]
async fn post_api_source_swaps_source_reanchors_threads_and_broadcasts() {
    let addr = free_loopback_addr();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_markdown_source("# Old Title\n\nOld body.");
    {
        let mut state = app_state
            .state
            .write()
            .expect("state lock should not be poisoned");
        state.add_thread(thread("u-keep", 2));
        state.add_thread(thread("u-lost", 4));
    }
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state.clone(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let mut sse = open_get_path(addr, "/api/events").await;
    let headers = read_until(&mut sse, "\r\n\r\n").await;
    assert_sse_headers(&headers);

    let body = json!({
        "markdown": "# New Title\n\nNew body.",
        "threadAnchors": [
            { "threadId": "u-keep", "anchorStart": 1, "anchorEnd": 1, "snippet": "New Title" },
            { "threadId": "u-lost", "orphaned": true }
        ]
    });
    let response = post_json_path(addr, "/api/source", &body.to_string()).await;
    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");
    assert_json_headers(&response);

    let payload = response_json(&response);
    assert_eq!(payload["markdown"], "# New Title\n\nNew body.");
    assert!(
        payload["renderedHtml"]
            .as_str()
            .expect("renderedHtml string")
            .contains("<h1>New Title</h1>")
    );
    assert_eq!(payload["sourceVersion"], 1);
    assert_eq!(payload["orphanedThreadIds"], json!(["u-lost"]));
    let anchors = payload["threadAnchors"]
        .as_array()
        .expect("threadAnchors array");
    let keep = anchors
        .iter()
        .find(|a| a["threadId"] == "u-keep")
        .expect("u-keep anchor");
    assert_eq!(keep["anchorStart"], 1);
    assert_eq!(keep["anchorEnd"], 1);
    assert_eq!(keep["orphaned"], false);
    let lost = anchors
        .iter()
        .find(|a| a["threadId"] == "u-lost")
        .expect("u-lost anchor");
    assert_eq!(lost["orphaned"], true);

    // SSE broadcast carries the same payload.
    let event = read_until(&mut sse, "\n\n").await;
    assert!(event.contains("event: source.updated"), "event: {event}");
    assert!(event.contains("<h1>New Title</h1>"));

    // Stdout event emitted for observing agents.
    let stdout_line = stdout_string(&stdout);
    assert!(
        stdout_line.contains("\"source.updated\""),
        "stdout: {stdout_line}"
    );

    // Root page renders the new source; state reflects new anchors + version.
    let response = get_root(addr).await;
    assert!(doc_content(response_body(&response)).contains("<h1>New Title</h1>"));
    let state = response_json(&get_path(addr, "/api/state").await);
    assert_eq!(state["sourceVersion"], 1);
    let threads = state["threads"].as_array().expect("threads array");
    let keep = threads
        .iter()
        .find(|t| t["id"] == "u-keep")
        .expect("u-keep thread");
    assert_eq!(keep["anchorStart"], 1);
    assert!(keep.get("orphaned").is_none());
    let lost = threads
        .iter()
        .find(|t| t["id"] == "u-lost")
        .expect("u-lost thread");
    assert_eq!(lost["orphaned"], true);

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_source_enforces_strict_thread_coverage() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_markdown_source("# Doc");
    {
        let mut state = app_state
            .state
            .write()
            .expect("state lock should not be poisoned");
        state.add_thread(thread("u-one", 1));
        state.add_thread(thread("u-two", 2));
    }
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state.clone(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    // Missing an active thread -> rejected.
    let body = json!({
        "markdown": "# Doc v2",
        "threadAnchors": [{ "threadId": "u-one", "anchorStart": 1, "anchorEnd": 1 }]
    });
    let response = post_json_path(addr, "/api/source", &body.to_string()).await;
    assert!(response.starts_with("HTTP/1.1 400"), "response: {response}");
    assert!(
        response_json(&response)["error"]["message"]
            .as_str()
            .expect("message")
            .contains("u-two")
    );

    // Unknown thread id -> rejected.
    let body = json!({
        "markdown": "# Doc v2",
        "threadAnchors": [
            { "threadId": "u-one", "anchorStart": 1, "anchorEnd": 1 },
            { "threadId": "u-two", "anchorStart": 2, "anchorEnd": 2 },
            { "threadId": "u-ghost", "anchorStart": 3, "anchorEnd": 3 }
        ]
    });
    let response = post_json_path(addr, "/api/source", &body.to_string()).await;
    assert!(response.starts_with("HTTP/1.1 400"), "response: {response}");

    // Invalid anchors (end < start) -> rejected.
    let body = json!({
        "markdown": "# Doc v2",
        "threadAnchors": [
            { "threadId": "u-one", "anchorStart": 3, "anchorEnd": 1 },
            { "threadId": "u-two", "anchorStart": 2, "anchorEnd": 2 }
        ]
    });
    let response = post_json_path(addr, "/api/source", &body.to_string()).await;
    assert!(response.starts_with("HTTP/1.1 400"), "response: {response}");

    // Anchors omitted without orphaned -> rejected.
    let body = json!({
        "markdown": "# Doc v2",
        "threadAnchors": [
            { "threadId": "u-one" },
            { "threadId": "u-two", "anchorStart": 2, "anchorEnd": 2 }
        ]
    });
    let response = post_json_path(addr, "/api/source", &body.to_string()).await;
    assert!(response.starts_with("HTTP/1.1 400"), "response: {response}");

    // A failed update must not touch state.
    let state = response_json(&get_path(addr, "/api/state").await);
    assert_eq!(state["sourceVersion"], 0);
    let response = get_root(addr).await;
    assert!(doc_content(response_body(&response)).contains("<h1>Doc</h1>"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_threads_rejects_stale_source_version() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_markdown_source("# Doc");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state.clone(), async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    // Bump the source version via a live update (no threads yet, so empty coverage).
    let body = json!({ "markdown": "# Doc v2", "threadAnchors": [] });
    let response = post_json_path(addr, "/api/source", &body.to_string()).await;
    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");

    // A thread created against the old document version is rejected.
    let body = json!({
        "anchorStart": 1,
        "anchorEnd": 1,
        "snippet": "Doc",
        "text": "comment",
        "sourceVersion": 0
    });
    let response = post_json_path(addr, "/api/threads", &body.to_string()).await;
    assert!(response.starts_with("HTTP/1.1 409"), "response: {response}");
    assert_eq!(
        response_json(&response)["error"]["code"],
        "stale_source_version"
    );

    // The current version (or omitting sourceVersion) still works.
    let body = json!({
        "anchorStart": 1,
        "anchorEnd": 1,
        "snippet": "Doc v2",
        "text": "comment",
        "sourceVersion": 1
    });
    let response = post_json_path(addr, "/api/threads", &body.to_string()).await;
    assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

fn multi_file_source() -> discuss::state::Source {
    use discuss::state::{File, FileId, FileKind, Source};

    Source {
        files: vec![
            File {
                id: FileId("f-1".to_string()),
                path: "alpha.md".to_string(),
                kind: FileKind::Markdown,
                content: "# Alpha\n\nAlpha body.\n".to_string(),
            },
            File {
                id: FileId("f-2".to_string()),
                path: "beta.md".to_string(),
                kind: FileKind::Markdown,
                content: "# Beta\n\nBeta body.\n".to_string(),
            },
            File {
                id: FileId("f-3".to_string()),
                path: "code.rs".to_string(),
                kind: FileKind::Diff,
                content: "diff --git a/code.rs b/code.rs\nindex 111..222 100644\n--- a/code.rs\n+++ b/code.rs\n@@ -1 +1 @@\n-fn old() {}\n+fn new_name() {}\n".to_string(),
            },
        ],
    }
}

#[tokio::test]
async fn get_root_seeds_rendered_files_and_file_metadata_for_multi_file_sessions() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_source(multi_file_source());
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let response = get_root(addr).await;
    assert!(response.starts_with("HTTP/1.1 200"));
    let body = response_body(&response);

    // First file is injected into #doc-content; every file is seeded into
    // the rendered-files map for client-side switching.
    assert!(doc_content(body).contains("<h1>Alpha</h1>"));
    assert!(body.contains("window.__DISCUSS_RENDERED_FILES__ = "));
    assert!(body.contains(r#"{"id":"f-1","html":"#));
    assert!(body.contains(r#"{"id":"f-2","html":"#));
    assert!(body.contains(r#"{"id":"f-3","html":"#));
    // Diff files render as per-hunk diff-<lang> fenced blocks.
    assert!(body.contains("language-diff-rust"));
    // Initial state carries file metadata (no content) for the sidebar.
    assert!(body.contains(r#"\"files\":"#) || body.contains(r#""files":"#));
    assert!(body.contains(r#""path":"beta.md"#));
    assert!(body.contains(r#""kind":"diff"#));
    assert!(body.contains(r#"id="file-sidebar""#));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_threads_validates_file_id_against_loaded_files() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_source(multi_file_source());
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let missing = post_json_path(
        addr,
        "/api/threads",
        r#"{"anchorStart":1,"anchorEnd":1,"snippet":"s","text":"t"}"#,
    )
    .await;
    assert!(missing.starts_with("HTTP/1.1 400"), "response: {missing}");
    assert_eq!(response_json(&missing)["error"]["code"], "missing_file_id");

    let unknown = post_json_path(
        addr,
        "/api/threads",
        r#"{"fileId":"f-9","anchorStart":1,"anchorEnd":1,"snippet":"s","text":"t"}"#,
    )
    .await;
    assert!(unknown.starts_with("HTTP/1.1 404"), "response: {unknown}");
    assert_eq!(response_json(&unknown)["error"]["code"], "unknown_file");

    let created = post_json_path(
        addr,
        "/api/threads",
        r#"{"fileId":"f-2","anchorStart":2,"anchorEnd":2,"snippet":"Beta body.","text":"note"}"#,
    )
    .await;
    assert!(created.starts_with("HTTP/1.1 200"), "response: {created}");
    let created = response_json(&created);
    assert_eq!(created["fileId"], "f-2");

    let state = response_json(&get_path(addr, "/api/state").await);
    assert_eq!(state["threads"][0]["fileId"], "f-2");

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_threads_defaults_file_id_in_single_file_sessions() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_markdown_source("# Solo\n\nBody.\n");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let created = post_json_path(
        addr,
        "/api/threads",
        r#"{"anchorStart":1,"anchorEnd":1,"snippet":"Solo","text":"no fileId needed"}"#,
    )
    .await;
    assert!(created.starts_with("HTTP/1.1 200"), "response: {created}");
    assert_eq!(response_json(&created)["fileId"], "f-1");

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn post_api_source_scopes_coverage_and_payload_to_the_updated_file() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_source(multi_file_source());
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    let created = post_json_path(
        addr,
        "/api/threads",
        r#"{"fileId":"f-2","anchorStart":2,"anchorEnd":2,"snippet":"Beta body.","text":"beta note"}"#,
    )
    .await;
    assert!(created.starts_with("HTTP/1.1 200"), "response: {created}");
    let thread_id = response_json(&created)["id"].as_str().unwrap().to_string();

    // Updating f-1 must not require (or accept) f-2's threads.
    let foreign = post_json_path(
        addr,
        "/api/source",
        &json!({
            "fileId": "f-1",
            "markdown": "# Alpha v2\n",
            "threadAnchors": [{ "threadId": thread_id, "anchorStart": 1, "anchorEnd": 1 }]
        })
        .to_string(),
    )
    .await;
    assert!(foreign.starts_with("HTTP/1.1 400"), "response: {foreign}");

    let updated = post_json_path(
        addr,
        "/api/source",
        r##"{"fileId":"f-1","markdown":"# Alpha v2\n","threadAnchors":[]}"##,
    )
    .await;
    assert!(updated.starts_with("HTTP/1.1 200"), "response: {updated}");
    let payload = response_json(&updated);
    assert_eq!(payload["fileId"], "f-1");
    assert!(
        payload["renderedHtml"]
            .as_str()
            .unwrap()
            .contains("<h1>Alpha v2</h1>")
    );
    assert_eq!(payload["threadAnchors"], json!([]));

    // Updating f-2 requires covering its active thread.
    let uncovered = post_json_path(
        addr,
        "/api/source",
        r##"{"fileId":"f-2","markdown":"# Beta v2\n","threadAnchors":[]}"##,
    )
    .await;
    assert!(
        uncovered.starts_with("HTTP/1.1 400"),
        "response: {uncovered}"
    );
    assert!(uncovered.contains("must cover every active thread on file f-2"));

    shutdown_tx.send(()).expect("send shutdown signal");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");
}

#[tokio::test]
async fn drafts_with_same_anchors_on_different_files_coexist() {
    let addr = free_loopback_addr();
    let app_state = AppState::for_process().with_source(multi_file_source());
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));

    wait_for_server(addr).await;

    for (file_id, text) in [("f-1", "alpha draft"), ("f-2", "beta draft")] {
        let response = post_json_path(
            addr,
            "/api/drafts/new-thread",
            &json!({ "fileId": file_id, "anchorStart": 1, "anchorEnd": 2, "text": text })
                .to_string(),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 200"), "response: {response}");
        assert_eq!(response_json(&response)["fileId"], *file_id);
    }

    let state = response_json(&get_path(addr, "/api/state").await);
    assert_eq!(
        state["drafts"]["newThread"]["f-1|1-2"]["text"],
        "alpha draft"
    );
    assert_eq!(
        state["drafts"]["newThread"]["f-2|1-2"]["text"],
        "beta draft"
    );

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

fn test_verdict_config() -> VerdictConfig {
    VerdictConfig {
        prompt: Some("Final verdict?".to_string()),
        options: vec![
            VerdictOption {
                id: "approved".to_string(),
                label: "Approve".to_string(),
                style: VerdictStyle::Positive,
                feedback_required: false,
            },
            VerdictOption {
                id: "declined".to_string(),
                label: "Decline".to_string(),
                style: VerdictStyle::Negative,
                feedback_required: true,
            },
        ],
    }
}

async fn assert_verdict_rejection_is_noop_then_valid(invalid_body: &str, expected_code: &str) {
    let addr = free_loopback_addr();
    let stdout = Arc::new(Mutex::new(Vec::new()));
    let app_state = AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(16)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_no_save(true)
    .with_idle_timeout_secs(0)
    .with_verdict_config(Some(test_verdict_config()));
    let server = tokio::spawn(serve(addr, app_state, pending()));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/done", invalid_body).await;

    assert!(
        response.starts_with("HTTP/1.1 400 Bad Request"),
        "expected 400 response, got {response}"
    );
    assert_eq!(response_json(&response)["error"]["code"], expected_code);
    assert!(
        stdout_string(&stdout).is_empty(),
        "rejected verdict must not emit session.done"
    );

    let state_response = get_path(addr, "/api/state").await;
    assert!(
        state_response.starts_with("HTTP/1.1 200 OK"),
        "server should still be alive after rejection: {state_response}"
    );

    let response = post_json_path(
        addr,
        "/api/done",
        r#"{"verdict":{"optionId":"declined","feedback":"needs work"}}"#,
    )
    .await;

    assert!(response.starts_with("HTTP/1.1 200 OK"), "{response}");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");

    let stdout = stdout_string(&stdout);
    assert!(
        stdout.contains("session.done"),
        "valid retry should emit session.done: {stdout}"
    );
    assert!(
        stdout.contains("needs work"),
        "valid retry should carry verdict feedback: {stdout}"
    );
}

async fn post_json_path(addr: SocketAddr, path: &str, body: &str) -> String {
    try_post_json_path(addr, path, body)
        .await
        .expect("POST request should succeed")
}

async fn post_path_no_body(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect to server");
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .expect("write request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .await
        .expect("read response");
    String::from_utf8(response).expect("response should be utf-8")
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
        orphaned: false,
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
