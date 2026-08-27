//! Integration tests for demo mode: seeded state, the canned Demo agent
//! responder (initial/follow-up behavior and suppression cases), and
//! normal-mode isolation.

use std::future::pending;
use std::io::{self, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener as StdTcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use discuss::server::demo::{
    DEMO_AGENT_PREFIX, canned_response, demo_source, seed_demo_threads, spawn_demo_responder,
};
use discuss::state::{FileKind, State, ThreadId};
use discuss::{AppState, EventBus, EventEmitter, serve};
use serde_json::Value;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::time::{sleep, timeout};

const SHORT_DELAY: Duration = Duration::from_millis(50);
const SUPPRESSION_DELAY: Duration = Duration::from_millis(300);

#[tokio::test]
async fn demo_state_seeds_four_agent_threads_and_allocates_ids_after_them() {
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    seed_demo_threads(&app_state);
    let addr = free_loopback_addr();
    let (server, shutdown_tx) = spawn_server(addr, app_state);

    wait_for_server(addr).await;

    let state = state_json(addr).await;
    let threads = state["threads"].as_array().expect("threads array");
    assert_eq!(threads.len(), 4);
    for (index, thread) in threads.iter().enumerate() {
        let id = format!("a-{}", index + 1);
        assert_eq!(thread["id"], id);
        assert_eq!(thread["kind"], "agent");
        let takes = state["takes"][&id].as_array().expect("seed takes");
        assert_eq!(takes.len(), 1);
        assert_eq!(takes[0]["id"], format!("t-{}", index + 1));
        assert!(
            takes[0]["text"]
                .as_str()
                .expect("take text")
                .starts_with(DEMO_AGENT_PREFIX)
        );
    }
    // Seeded threads sit on the two revised markdown files.
    assert_eq!(threads[0]["fileId"], "f-2");
    assert_eq!(threads[1]["fileId"], "f-2");
    assert_eq!(threads[2]["fileId"], "f-3");
    assert_eq!(threads[3]["fileId"], "f-3");

    // The initial page snapshot carries the seeds too.
    let page = get_path(addr, "/").await;
    assert!(page.contains("\"a-1\""));

    // A runtime agent thread allocates a-5: no collision with the seeds.
    let response = post_json_path(
        addr,
        "/api/threads",
        r#"{"fileId":"f-2","kind":"agent","anchorStart":3,"anchorEnd":3,"snippet":"failure rate","text":"agent note"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    assert_eq!(response_json(&response)["id"], "a-5");

    finish(server, shutdown_tx).await;
}

#[tokio::test]
async fn responder_answers_user_thread_and_reply_with_sse_only_takes() {
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    let seeded = seed_demo_threads(&app_state);
    spawn_demo_responder(app_state.clone(), seeded, SHORT_DELAY);
    let mut bus_rx = app_state.bus.subscribe();
    let addr = free_loopback_addr();
    let (server, shutdown_tx) = spawn_server(addr, app_state.clone());

    wait_for_server(addr).await;

    let response = post_json_path(
        addr,
        "/api/threads",
        r#"{"fileId":"f-2","anchorStart":3,"anchorEnd":3,"snippet":"failure rate","text":"Is 2.1% still the current figure?"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let thread_id = response_json(&response)["id"]
        .as_str()
        .expect("thread id")
        .to_string();

    let takes = wait_for_takes(addr, &thread_id, 1).await;
    // A reviewer-opened thread is not one of the seeds: no seed index.
    let opener = canned_response(None, FileKind::Markdown, 0);
    assert_eq!(takes[0]["text"], opener);

    // The response was published as a take.added SSE broadcast...
    let event = wait_for_bus_event(&mut bus_rx, "take.added").await;
    assert_eq!(event["threadId"], thread_id.as_str());

    // ...and a user reply gets exactly one generic follow-up.
    let response = post_json_path(
        addr,
        &format!("/api/threads/{thread_id}/replies"),
        r#"{"text":"Yes, it is."}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let takes = wait_for_takes(addr, &thread_id, 2).await;
    let followup = canned_response(None, FileKind::Markdown, 1);
    assert_eq!(takes[1]["text"], followup);

    // Extra settling time must not produce duplicates.
    sleep(SHORT_DELAY * 5).await;
    let takes = takes_json(addr, &thread_id).await;
    assert_eq!(takes.len(), 2);

    // Stdout keeps normal semantics: thread/reply events, never take events.
    let stdout = stdout_string(&stdout);
    assert!(stdout.contains("thread.created"));
    assert!(stdout.contains("reply.added"));
    assert!(!stdout.contains("take.added"));
    assert!(!stdout.contains(DEMO_AGENT_PREFIX));

    finish(server, shutdown_tx).await;
}

#[tokio::test]
async fn responder_never_answers_agent_created_threads() {
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    let seeded = seed_demo_threads(&app_state);
    spawn_demo_responder(app_state.clone(), seeded, SHORT_DELAY);
    let addr = free_loopback_addr();
    let (server, shutdown_tx) = spawn_server(addr, app_state);

    wait_for_server(addr).await;

    let response = post_json_path(
        addr,
        "/api/threads",
        r#"{"fileId":"f-3","kind":"agent","anchorStart":3,"anchorEnd":3,"snippet":"rate limit","text":"Pre-annotating this passage."}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let thread_id = response_json(&response)["id"]
        .as_str()
        .expect("thread id")
        .to_string();

    sleep(SHORT_DELAY * 6).await;
    let takes = takes_json(addr, &thread_id).await;
    // Only the agent thread's own opening take: no canned response ever.
    assert_eq!(takes.len(), 1);
    assert_eq!(takes[0]["text"], "Pre-annotating this passage.");

    finish(server, shutdown_tx).await;
}

#[tokio::test]
async fn replies_on_seeded_threads_get_tailored_follow_up_then_closer() {
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    let seeded = seed_demo_threads(&app_state);
    spawn_demo_responder(app_state.clone(), seeded, SHORT_DELAY);
    let addr = free_loopback_addr();
    let (server, shutdown_tx) = spawn_server(addr, app_state);

    wait_for_server(addr).await;

    let response = post_json_path(
        addr,
        "/api/threads/a-1/replies",
        r#"{"text":"Confirmed, the ceiling fits the budget."}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let takes = wait_for_takes(addr, "a-1", 2).await;
    let tailored = canned_response(Some(0), FileKind::Markdown, 1);
    assert_eq!(takes[1]["text"], tailored);
    assert_ne!(
        takes[1]["text"],
        canned_response(None, FileKind::Markdown, 1),
        "seeded threads get a tailored follow-up, not the generic one"
    );

    let response = post_json_path(
        addr,
        "/api/threads/a-1/replies",
        r#"{"text":"One more thought."}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let takes = wait_for_takes(addr, "a-1", 3).await;
    let closer = canned_response(Some(0), FileKind::Markdown, 2);
    assert_eq!(takes[2]["text"], closer);

    // Exactly one take per reply: no extras after settling.
    sleep(SHORT_DELAY * 5).await;
    assert_eq!(takes_json(addr, "a-1").await.len(), 3);

    finish(server, shutdown_tx).await;
}

#[tokio::test]
async fn resolve_during_the_delay_suppresses_the_pending_response() {
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    let seeded = seed_demo_threads(&app_state);
    spawn_demo_responder(app_state.clone(), seeded, SUPPRESSION_DELAY);
    let addr = free_loopback_addr();
    let (server, shutdown_tx) = spawn_server(addr, app_state);

    wait_for_server(addr).await;

    let thread_id = create_user_thread(addr).await;
    let response = post_json_path(
        addr,
        &format!("/api/threads/{thread_id}/resolve"),
        r#"{"decision":"agree"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    sleep(SUPPRESSION_DELAY * 3).await;
    assert!(
        takes_json(addr, &thread_id).await.is_empty(),
        "no take may land after the thread is resolved"
    );

    finish(server, shutdown_tx).await;
}

#[tokio::test]
async fn delete_during_the_delay_suppresses_the_pending_response() {
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    let seeded = seed_demo_threads(&app_state);
    spawn_demo_responder(app_state.clone(), seeded, SUPPRESSION_DELAY);
    let addr = free_loopback_addr();
    let (server, shutdown_tx) = spawn_server(addr, app_state.clone());

    wait_for_server(addr).await;

    let thread_id = create_user_thread(addr).await;
    let response = delete_path(addr, &format!("/api/threads/{thread_id}")).await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    sleep(SUPPRESSION_DELAY * 3).await;
    {
        let state = app_state.state.read().expect("state lock");
        assert!(
            !state.snapshot().takes.contains_key(&ThreadId(thread_id)),
            "no take may land after the thread is deleted"
        );
    }

    finish(server, shutdown_tx).await;
}

#[tokio::test]
async fn shutdown_during_the_delay_cancels_the_pending_response() {
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    let seeded = seed_demo_threads(&app_state);
    spawn_demo_responder(app_state.clone(), seeded, SUPPRESSION_DELAY);
    let addr = free_loopback_addr();
    let server = tokio::spawn(serve(addr, app_state.clone(), pending()));

    wait_for_server(addr).await;

    let thread_id = create_user_thread(addr).await;
    // Done signals internal shutdown while the responder is still sleeping.
    let response = post_json_path(addr, "/api/done", "").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");

    sleep(SUPPRESSION_DELAY * 3).await;
    let state = app_state.state.read().expect("state lock");
    assert!(
        !state.snapshot().takes.contains_key(&ThreadId(thread_id)),
        "no take may land after session shutdown"
    );
}

#[tokio::test]
async fn responder_dedupes_repeated_trigger_keys() {
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    let seeded = seed_demo_threads(&app_state);
    spawn_demo_responder(app_state.clone(), seeded, SHORT_DELAY);
    let addr = free_loopback_addr();
    let (server, shutdown_tx) = spawn_server(addr, app_state.clone());

    wait_for_server(addr).await;

    // Broadcast lag drops events rather than replaying them, so this guards
    // the responder's own bookkeeping: an identical trigger key must produce
    // exactly one take.
    let duplicate = discuss::BroadcastEvent {
        kind: "reply.added".to_string(),
        payload: serde_json::json!({ "id": "r-dup", "threadId": "a-1" }),
    };
    app_state.bus.publish(duplicate.clone());
    app_state.bus.publish(duplicate);

    let takes = wait_for_takes(addr, "a-1", 2).await;
    assert_eq!(takes.len(), 2, "seed opener plus exactly one response");
    sleep(SHORT_DELAY * 5).await;
    assert_eq!(takes_json(addr, "a-1").await.len(), 2);

    finish(server, shutdown_tx).await;
}

#[tokio::test]
async fn burst_of_user_threads_each_get_exactly_one_response_in_order() {
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    let seeded = seed_demo_threads(&app_state);
    spawn_demo_responder(app_state.clone(), seeded, SHORT_DELAY);
    let addr = free_loopback_addr();
    let (server, shutdown_tx) = spawn_server(addr, app_state);

    wait_for_server(addr).await;

    let mut thread_ids = Vec::new();
    for _ in 0..3 {
        thread_ids.push(create_user_thread(addr).await);
    }

    // One response per trigger, answered sequentially in trigger order:
    // the take ids continue t-5, t-6, t-7 after the four seeds.
    for (index, thread_id) in thread_ids.iter().enumerate() {
        let takes = wait_for_takes(addr, thread_id, 1).await;
        assert_eq!(takes.len(), 1, "{thread_id} gets exactly one response");
        assert_eq!(takes[0]["id"], format!("t-{}", index + 5));
    }
    sleep(SHORT_DELAY * 5).await;
    for thread_id in &thread_ids {
        assert_eq!(takes_json(addr, thread_id).await.len(), 1);
    }

    finish(server, shutdown_tx).await;
}

#[tokio::test]
async fn demo_prototype_asset_route_is_a_deterministic_404() {
    // The test CWD is the crate root, which contains a real `demo/`
    // directory; bare-filename virtual paths must keep the asset route
    // unreachable regardless.
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    let addr = free_loopback_addr();
    let (server, shutdown_tx) = spawn_server(addr, app_state);

    wait_for_server(addr).await;

    let document = get_path(addr, "/files/f-6").await;
    assert!(document.starts_with("HTTP/1.1 200"), "{document}");
    assert!(document.contains("Ledgerly"));

    for asset in ["record-demo.mjs", "prototype.html", "mockup.png"] {
        let response = get_path(addr, &format!("/files/f-6/assets/{asset}")).await;
        assert!(
            response.starts_with("HTTP/1.1 404"),
            "asset route must 404 for {asset}: {response}"
        );
        assert_eq!(response_json(&response)["error"]["code"], "not_found");
    }

    finish(server, shutdown_tx).await;
}

#[tokio::test]
async fn normal_sessions_without_the_responder_produce_no_canned_takes() {
    // Structural isolation: an identical session that simply never calls
    // spawn_demo_responder gets zero takes for the same stimuli, and stdout
    // keeps the normal event stream.
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout);
    let addr = free_loopback_addr();
    let (server, shutdown_tx) = spawn_server(addr, app_state);

    wait_for_server(addr).await;

    let thread_id = create_user_thread(addr).await;
    let response = post_json_path(
        addr,
        &format!("/api/threads/{thread_id}/replies"),
        r#"{"text":"Follow-up question."}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");

    sleep(SHORT_DELAY * 6).await;
    assert!(takes_json(addr, &thread_id).await.is_empty());

    let stdout = stdout_string(&stdout);
    assert!(stdout.contains("thread.created"));
    assert!(stdout.contains("reply.added"));
    assert!(!stdout.contains(DEMO_AGENT_PREFIX));

    finish(server, shutdown_tx).await;
}

#[tokio::test]
async fn demo_done_writes_no_history_archive() {
    let history_dir = tempfile::tempdir().expect("history tempdir");
    let stdout = shared_stdout();
    let app_state = demo_app_state(&stdout).with_history_dir(history_dir.path());
    seed_demo_threads(&app_state);
    let addr = free_loopback_addr();
    let server = tokio::spawn(serve(addr, app_state, pending()));

    wait_for_server(addr).await;

    let response = post_json_path(addr, "/api/done", "").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    timeout(Duration::from_secs(1), server)
        .await
        .expect("server exits within timeout")
        .expect("server task should not panic")
        .expect("server shutdown should succeed");

    let entries: Vec<_> = std::fs::read_dir(history_dir.path())
        .expect("read history dir")
        .collect();
    assert!(entries.is_empty(), "demo sessions must not write archives");
}

// ---------- helpers ----------

fn shared_stdout() -> Arc<Mutex<Vec<u8>>> {
    Arc::new(Mutex::new(Vec::new()))
}

fn demo_app_state(stdout: &Arc<Mutex<Vec<u8>>>) -> AppState {
    let (source, file_bytes) = demo_source();
    AppState::new(
        State::new_shared(),
        Arc::new(EventBus::new(1024)),
        Arc::new(EventEmitter::boxed(SharedWriter(stdout.clone()))),
    )
    .with_source(source)
    .with_file_bytes(file_bytes)
    .with_no_save(true)
    .with_idle_timeout_secs(0)
}

fn spawn_server(
    addr: SocketAddr,
    app_state: AppState,
) -> (
    tokio::task::JoinHandle<discuss::Result<()>>,
    oneshot::Sender<()>,
) {
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let server = tokio::spawn(serve(addr, app_state, async move {
        let _ = shutdown_rx.await;
    }));
    (server, shutdown_tx)
}

async fn finish(
    server: tokio::task::JoinHandle<discuss::Result<()>>,
    shutdown_tx: oneshot::Sender<()>,
) {
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
            Err(_) if tokio::time::Instant::now() < deadline => {
                sleep(Duration::from_millis(10)).await;
            }
            Err(error) => panic!("server did not start at {addr}: {error}"),
        }
    }
}

async fn create_user_thread(addr: SocketAddr) -> String {
    let response = post_json_path(
        addr,
        "/api/threads",
        r#"{"fileId":"f-2","anchorStart":3,"anchorEnd":3,"snippet":"failure rate","text":"What about this?"}"#,
    )
    .await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    response_json(&response)["id"]
        .as_str()
        .expect("thread id")
        .to_string()
}

async fn state_json(addr: SocketAddr) -> Value {
    let response = get_path(addr, "/api/state").await;
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    response_json(&response)
}

async fn takes_json(addr: SocketAddr, thread_id: &str) -> Vec<Value> {
    state_json(addr).await["takes"][thread_id]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

async fn wait_for_takes(addr: SocketAddr, thread_id: &str, minimum: usize) -> Vec<Value> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        let takes = takes_json(addr, thread_id).await;
        if takes.len() >= minimum {
            return takes;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {minimum} takes on {thread_id}; saw {takes:?}"
        );
        sleep(Duration::from_millis(20)).await;
    }
}

async fn wait_for_bus_event(
    rx: &mut tokio::sync::broadcast::Receiver<discuss::BroadcastEvent>,
    kind: &str,
) -> Value {
    loop {
        let event = timeout(Duration::from_secs(3), rx.recv())
            .await
            .expect("bus event within timeout")
            .expect("bus should stay open");
        if event.kind == kind {
            return event.payload;
        }
    }
}

async fn get_path(addr: SocketAddr, path: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.expect("connect to server");
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
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

fn response_json(response: &str) -> Value {
    let body = response
        .split("\r\n\r\n")
        .nth(1)
        .expect("response should have a body");
    // Bodies may be chunked; take the JSON line.
    let json_line = body
        .lines()
        .find(|line| line.starts_with('{') || line.starts_with('['))
        .expect("body should contain JSON");
    serde_json::from_str(json_line).expect("body should be valid JSON")
}

fn stdout_string(stdout: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8(stdout.lock().expect("stdout lock").clone()).expect("stdout is utf-8")
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
