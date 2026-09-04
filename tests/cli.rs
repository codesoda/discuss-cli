use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use chrono::DateTime;
use serde_json::Value;
use tempfile::tempdir;

/// Ceiling for every "wait for the spawned binary to announce itself" step.
///
/// Each of these tests spawns `CARGO_BIN_EXE_discuss` and blocks on its first
/// stderr/stdout line. Locally that lands in milliseconds; the ceiling exists
/// for loaded machines, where process spawn and first-touch paging of a large
/// debug binary are the variable costs. `recv_timeout` returns as soon as the
/// line arrives, so a generous ceiling costs nothing on the passing path — it
/// only changes how long a genuine hang takes to report. Kept as a single
/// constant so the startup waits cannot drift apart per test.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

#[test]
fn cli_busy_port_exits_three_and_reports_port() {
    let busy_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind busy listener");
    let busy_port = busy_listener
        .local_addr()
        .expect("busy listener addr")
        .port();
    let env_port = free_port();
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");
    let markdown_path = temp_dir.path().join("review.md");
    fs::write(&markdown_path, "# Review\n").expect("markdown file should be written");

    let child = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("--port")
        .arg(busy_port.to_string())
        .arg(&markdown_path)
        .env("HOME", &home_dir)
        .env("DISCUSS_PORT", env_port.to_string())
        .env_remove("DISCUSS_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn discuss binary");
    let output = wait_with_timeout(child, Duration::from_secs(2));

    assert_eq!(output.status.code(), Some(3));
    assert!(
        output.stdout.is_empty(),
        "stdout should be reserved for JSON events"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(stderr.contains(&format!("port {busy_port}")));
    assert!(stderr.contains("choose another explicit port"));
    assert!(stderr.contains("clear the port override"));
    assert!(stderr.contains("stop the other instance"));
}

#[test]
fn cli_no_open_logs_listening_url_to_stderr() {
    let port = free_port();
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");
    let markdown_path = temp_dir.path().join("review.md");
    fs::write(&markdown_path, "# Review\n").expect("markdown file should be written");

    let mut child = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("--no-open")
        .arg("--port")
        .arg(port.to_string())
        .arg(&markdown_path)
        .env("HOME", &home_dir)
        .env_remove("DISCUSS_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn discuss binary");
    let stderr = child.stderr.take().expect("stderr pipe should be present");
    let line_rx = read_first_line(stderr);

    let line = line_rx
        .recv_timeout(STARTUP_TIMEOUT)
        .expect("listening line should be written")
        .expect("stderr line should be readable");
    assert_eq!(line, format!("review UI/API: http://127.0.0.1:{port}\n"));

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cli_update_requires_yes_when_stdin_is_not_tty() {
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");

    let output = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("update")
        .env("HOME", &home_dir)
        .env_remove("DISCUSS_LOG")
        .output()
        .expect("spawn discuss binary");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "stdout should be reserved for JSON events"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(stderr.contains("stdin is not a TTY"));
    assert!(stderr.contains("discuss update -y"));
}

#[test]
fn cli_emits_single_session_started_event_after_listening() {
    let port = free_port();
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");
    let markdown_path = temp_dir.path().join("review.md");
    fs::write(&markdown_path, "# Review\n").expect("markdown file should be written");
    let source_file = fs::canonicalize(&markdown_path)
        .expect("markdown path should canonicalize")
        .to_string_lossy()
        .into_owned();

    let mut child = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("--no-open")
        .arg("--port")
        .arg(port.to_string())
        .arg(&markdown_path)
        .env("HOME", &home_dir)
        .env_remove("DISCUSS_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn discuss binary");
    let stderr = child.stderr.take().expect("stderr pipe should be present");
    let line_rx = read_first_line(stderr);

    let line = line_rx
        .recv_timeout(STARTUP_TIMEOUT)
        .expect("listening line should be written")
        .expect("stderr line should be readable");
    assert_eq!(line, format!("review UI/API: http://127.0.0.1:{port}\n"));

    let output = kill_and_collect(child);
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    let events = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("stdout line should be JSON"))
        .collect::<Vec<_>>();

    assert_eq!(events.len(), 1, "stdout should contain one startup event");
    let event = &events[0];
    assert_eq!(event["kind"], "session.started");
    assert_rfc3339(event["at"].as_str().expect("event at should be a string"));
    let base_url = format!("http://127.0.0.1:{port}");
    assert_eq!(event["payload"]["url"], base_url);
    assert_eq!(event["payload"]["apiBaseUrl"], base_url);
    assert!(event["payload"].get("proxyUrl").is_none());
    assert_endpoint_contract(&event["payload"], &base_url);
    assert_eq!(event["payload"]["source_file"], source_file);
    assert_rfc3339(
        event["payload"]["started_at"]
            .as_str()
            .expect("started_at should be a string"),
    );
}

#[test]
fn cli_pr_without_gh_fails_before_readiness_with_install_instructions() {
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    let empty_bin = temp_dir.path().join("empty-bin");
    fs::create_dir(&home_dir).expect("home dir should be created");
    fs::create_dir(&empty_bin).expect("empty PATH directory should be created");

    let output = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .args([
            "--no-open",
            "--no-save",
            "pr",
            "https://github.com/codesoda/discuss-cli/pull/51",
        ])
        .env("HOME", &home_dir)
        .env("PATH", &empty_bin)
        .env_remove("DISCUSS_LOG")
        .output()
        .expect("run PR command without gh");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "startup must fail before session.started"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    for expected in [
        "GitHub CLI (`gh`) is required",
        "not found in PATH",
        "brew install gh",
        "winget install",
        "install_linux.md",
        "gh auth login --hostname github.com",
    ] {
        assert!(
            stderr.contains(expected),
            "expected stderr to contain {expected:?}: {stderr}"
        );
    }
}

#[cfg(unix)]
#[test]
fn cli_pr_automatically_imports_through_gh_before_review() {
    let port = free_port();
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    let bin_dir = temp_dir.path().join("bin");
    fs::create_dir(&home_dir).expect("home dir should be created");
    fs::create_dir(&bin_dir).expect("fake bin dir should be created");
    write_executable(
        &bin_dir.join("gh"),
        r##"#!/bin/sh
case "$1 $2" in
  "--version ") printf '%s\n' 'gh version test' ;;
  "auth status") exit 0 ;;
  "pr view") printf '%s\n' '{"number":51,"title":"Test PR","body":"Body","state":"OPEN","isDraft":false,"author":{"login":"octocat"},"baseRefName":"main","baseRefOid":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","headRefName":"feature","headRefOid":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","url":"https://github.com/codesoda/discuss-cli/pull/51"}' ;;
  "repo clone") /bin/mkdir -p "$4/.git" ;;
  "api --hostname")
    case "$*" in
      *'/files?per_page=100'*) printf '%s\n' '[[{"filename":"src/lib.rs"}]]' ;;
      *'/issues/'*'/comments?per_page=100'*) printf '%s\n' '[[]]' ;;
      *'/reviews?per_page=100'*) printf '%s\n' '[[]]' ;;
      *'/pulls/'*'/comments?per_page=100'*) printf '%s\n' '[[]]' ;;
      *' graphql '*) printf '%s\n' '[{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[]}}}}}]' ;;
      *) printf '%s\n' "unexpected gh api arguments: $*" >&2; exit 1 ;;
    esac ;;
  *) printf '%s\n' "unexpected gh arguments: $*" >&2; exit 1 ;;
esac
"##,
    );
    write_executable(
        &bin_dir.join("git"),
        r##"#!/bin/sh
case "$3" in
  fetch) exit 0 ;;
  rev-parse)
    case "$4" in
      refs/discuss/pr-head*) printf '%s\n' 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb' ;;
      *) printf '%s\n' 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa' ;;
    esac ;;
  diff)
    case "$*" in *'--unified=4'*) ;; *) printf '%s\n' "missing --unified=4: $*" >&2; exit 1 ;; esac
    printf '%s\n' 'diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1 +1 @@
-old
+new' ;;
  *) printf '%s\n' "unexpected git arguments: $*" >&2; exit 1 ;;
esac
"##,
    );

    let mut child = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .args([
            "--no-open",
            "--no-save",
            "--port",
            &port.to_string(),
            "pr",
            "https://github.com/codesoda/discuss-cli/pull/51",
            "--unified=4",
        ])
        .env("HOME", &home_dir)
        .env("PATH", &bin_dir)
        .env_remove("DISCUSS_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn automatic PR session");
    let stdout = read_all_lines(child.stdout.take().expect("stdout pipe"));
    let started_line = stdout
        .recv_timeout(STARTUP_TIMEOUT)
        .expect("session.started event")
        .expect("read session.started");
    let started: Value = serde_json::from_str(&started_line)
        .unwrap_or_else(|error| panic!("startup JSON {started_line:?}: {error}"));
    assert_eq!(started["kind"], "session.started");
    assert_eq!(started["payload"]["mode"], "pr");
    assert_eq!(started["payload"]["prImportMode"], "automatic");
    assert_eq!(started["payload"]["unified"], 4);
    assert_eq!(started["payload"]["files_count"], 2);
    let instructions = started["payload"]["agentInstructions"]
        .as_str()
        .expect("PR instructions");
    assert!(instructions.contains("Discuss itself loads the PR and publishes"));
    assert!(instructions.contains("Do not fetch, clone, publish"));
    assert!(!instructions.contains("gh repo clone"));
    assert!(!instructions.contains("pr.publish.requested"));

    let imported: Value = serde_json::from_str(
        &stdout
            .recv_timeout(STARTUP_TIMEOUT)
            .expect("automatic pr.imported event")
            .expect("read pr.imported"),
    )
    .expect("import JSON");
    assert_eq!(imported["kind"], "pr.imported");
    assert_eq!(imported["payload"]["files"][0]["path"], "src/lib.rs");

    let state_response =
        http_request_url(&format!("http://127.0.0.1:{port}/api/state"), "GET", None);
    let state = response_json(&state_response);
    assert_eq!(state["prSession"]["phase"], "reviewing");
    assert_eq!(state["files"].as_array().map(Vec::len), Some(2));

    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn cli_pr_rejects_invalid_url_files_and_verdict_flags() {
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");
    let file = temp_dir.path().join("review.md");
    fs::write(&file, "# Review\n").expect("write fixture");
    let cases: Vec<Vec<String>> = vec![
        vec!["pr".into(), "http://github.com/acme/repo/pull/1".into()],
        vec![
            file.display().to_string(),
            "pr".into(),
            "https://github.com/acme/repo/pull/1".into(),
        ],
        vec![
            "--verdict-options".into(),
            "yes,no".into(),
            "pr".into(),
            "https://github.com/acme/repo/pull/1".into(),
        ],
        vec![
            "--verdict-prompt".into(),
            "Choose".into(),
            "pr".into(),
            "https://github.com/acme/repo/pull/1".into(),
        ],
    ];
    for args in cases {
        let output = Command::new(env!("CARGO_BIN_EXE_discuss"))
            .args(&args)
            .env("HOME", &home_dir)
            .env_remove("DISCUSS_LOG")
            .output()
            .expect("run invalid PR command");
        assert_eq!(output.status.code(), Some(2), "args: {args:?}");
        assert!(output.stdout.is_empty(), "args: {args:?}");
    }
}

#[test]
fn cli_live_url_binds_adjacent_loopback_proxy_reports_contract_and_shuts_down_both() {
    let upstream = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind upstream fixture");
    let upstream_url = format!(
        "http://127.0.0.1:{}/start?from=test#section",
        upstream.local_addr().expect("upstream addr").port()
    );
    let api_port = free_adjacent_ports();
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");

    let mut child = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("--no-open")
        .arg("--no-save")
        .arg("--port")
        .arg(api_port.to_string())
        .arg(&upstream_url)
        .current_dir(temp_dir.path())
        .env("HOME", &home_dir)
        .env_remove("DISCUSS_LOG")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn live discuss session");
    let stdout = read_all_lines(child.stdout.take().expect("stdout pipe"));
    let stderr = read_all_lines(child.stderr.take().expect("stderr pipe"));
    let started: Value = serde_json::from_str(
        &stdout
            .recv_timeout(STARTUP_TIMEOUT)
            .expect("live startup event")
            .expect("read live startup event"),
    )
    .expect("live startup JSON");
    let api_url = format!("http://127.0.0.1:{api_port}");
    let proxy_url = format!("http://127.0.0.1:{}", api_port + 1);
    assert_eq!(started["payload"]["mode"], "live");
    assert_eq!(started["payload"]["upstreamUrl"], upstream_url);
    assert_eq!(started["payload"]["apiBaseUrl"], api_url);
    assert_eq!(started["payload"]["proxyUrl"], proxy_url);
    assert_endpoint_contract(&started["payload"], &api_url);
    assert_eq!(
        stderr.recv_timeout(STARTUP_TIMEOUT).unwrap().unwrap(),
        format!("review UI/API: {api_url}")
    );

    let root = http_request_url(&api_url, "GET", None);
    assert!(root.starts_with("HTTP/1.1 200"), "{root}");
    assert!(
        response_body(&root).contains(&format!("src=\\\"{proxy_url}/start?from=test#section\\\""))
            || response_body(&root)
                .contains(&format!("src=\"{proxy_url}/start?from=test#section\""))
    );

    let done_url = started["payload"]["endpoints"]["done"]
        .as_str()
        .expect("done endpoint");
    let done = http_request_url(done_url, "POST", None);
    assert!(done.starts_with("HTTP/1.1 200"), "{done}");
    let output = wait_with_timeout(child, Duration::from_secs(2));
    assert_eq!(output.status.code(), Some(0));
    assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, api_port)).is_err());
    assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, api_port + 1)).is_err());
}

#[test]
fn cli_live_url_fails_before_readiness_when_adjacent_proxy_port_is_occupied() {
    let (api_port, proxy_listener) = occupied_proxy_pair();
    let proxy_port = api_port + 1;
    let upstream = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind upstream fixture");
    let upstream_url = format!("http://127.0.0.1:{}", upstream.local_addr().unwrap().port());
    let temp_dir = tempdir().expect("tempdir");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir");

    let output = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("--no-open")
        .arg("--port")
        .arg(api_port.to_string())
        .arg(upstream_url)
        .env("HOME", home_dir)
        .env_remove("DISCUSS_LOG")
        .output()
        .expect("run live collision case");
    drop(proxy_listener);
    assert_eq!(output.status.code(), Some(3));
    assert!(
        output.stdout.is_empty(),
        "startup event must not be emitted"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains(&format!("port {proxy_port}")));
    assert!(
        TcpListener::bind((Ipv4Addr::LOCALHOST, api_port)).is_ok(),
        "partial API bind must be released"
    );
}

#[test]
fn cli_serves_server_stamped_markdown_anchors_matching_blocks_api() {
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");
    let markdown_path = temp_dir.path().join("review.md");
    fs::write(
        &markdown_path,
        "---\ntitle: Review\n---\n# Plan\n\nBody.\n\n| a |\n| - |\n| b |\n\n```rust\nfn main() {}\n```\n\nReference.[^note]\n\n[^note]: Detail.\n",
    )
    .expect("markdown file should be written");

    let mut session = spawn_dynamic_session(temp_dir.path(), &home_dir, &markdown_path);
    let started = receive_startup(&session, "anchor HTML");
    let base_url = startup_base_url(&started);
    assert_startup_contract(&session, &started, &base_url);

    let root_response = http_request_url(&base_url, "GET", None);
    assert!(root_response.starts_with("HTTP/1.1 200"), "{root_response}");
    let html = doc_content(response_body(&root_response));
    assert!(html.contains("<h1 data-anchor-idx=\"2\">Plan</h1>"));
    assert!(html.contains("class=\"table-wrap\" data-anchor-idx=\"4\""));
    assert!(html.contains("<tr data-anchor-idx=\"5\">"));
    assert!(html.contains("<tr data-anchor-idx=\"6\">"));
    assert!(html.contains("class=\"pre-wrap\" data-anchor-idx=\"7\""));

    let blocks_url = started["payload"]["endpoints"]["blocksTemplate"]
        .as_str()
        .expect("blocksTemplate endpoint should be a string")
        .replace("{fileId}", "f-1");
    let blocks_response = http_request_url(&blocks_url, "GET", None);
    assert!(
        blocks_response.starts_with("HTTP/1.1 200"),
        "{blocks_response}"
    );
    let blocks = response_json(&blocks_response);
    let block_indices = blocks["blocks"]
        .as_array()
        .expect("blocks array")
        .iter()
        .map(|block| block["index"].as_u64().expect("numeric block index"))
        .collect::<Vec<_>>();
    assert_eq!(stamped_indices(html), block_indices);
    assert_eq!(block_indices, (1..=9).collect::<Vec<_>>());

    let done_url = started["payload"]["endpoints"]["done"]
        .as_str()
        .expect("done endpoint should be a string");
    let done_response = http_request_url(done_url, "POST", None);
    assert!(done_response.starts_with("HTTP/1.1 200"), "{done_response}");
    let output = wait_with_timeout(
        session
            .child
            .take()
            .expect("session child should be present"),
        Duration::from_secs(2),
    );
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn concurrent_sessions_use_distinct_reported_endpoints_without_state_leakage() {
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");
    let markdown_path = temp_dir.path().join("review.md");
    fs::write(&markdown_path, "# Review\n\nBody paragraph.\n")
        .expect("markdown file should be written");

    let mut first = spawn_dynamic_session(temp_dir.path(), &home_dir, &markdown_path);
    let mut second = spawn_dynamic_session(temp_dir.path(), &home_dir, &markdown_path);
    let first_started = receive_startup(&first, "first");
    let second_started = receive_startup(&second, "second");

    let first_base = startup_base_url(&first_started);
    let second_base = startup_base_url(&second_started);
    assert_ne!(first_base, second_base);
    assert_startup_contract(&first, &first_started, &first_base);
    assert_startup_contract(&second, &second_started, &second_base);

    let first_state = create_thread_and_take(&first_started, "first question", "first take");
    let second_state = create_thread_and_take(&second_started, "second question", "second take");

    assert_eq!(first_state["threads"][0]["text"], "first question");
    assert_eq!(first_state["takes"]["u-1"][0]["text"], "first take");
    assert_eq!(second_state["threads"][0]["text"], "second question");
    assert_eq!(second_state["takes"]["u-1"][0]["text"], "second take");

    for (session, started) in [(&mut first, &first_started), (&mut second, &second_started)] {
        let done_url = started["payload"]["endpoints"]["done"]
            .as_str()
            .expect("done endpoint should be a string");
        let response = http_request_url(done_url, "POST", None);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        let output = wait_with_timeout(
            session
                .child
                .take()
                .expect("session child should be present"),
            Duration::from_secs(2),
        );
        assert_eq!(output.status.code(), Some(0));
    }
}

#[test]
fn cli_env_port_collision_exits_three_without_startup_event() {
    assert_explicit_port_collision(PortSource::Environment);
}

#[test]
fn cli_project_config_port_collision_exits_three_without_startup_event() {
    assert_explicit_port_collision(PortSource::ProjectConfig);
}

#[test]
fn cli_history_dir_flag_overrides_config_history_dir_and_writes_archive() {
    let port = free_port();
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    let discuss_dir = home_dir.join(".discuss");
    fs::create_dir_all(&discuss_dir).expect("home config dir should be created");
    let config_history_dir = temp_dir.path().join("config-history");
    fs::write(
        discuss_dir.join("discuss.config.toml"),
        format!("history_dir = \"{}\"\n", config_history_dir.display()),
    )
    .expect("user config should be written");
    let cli_history_dir = temp_dir.path().join("cli-history");
    let markdown_path = temp_dir.path().join("review.md");
    fs::write(&markdown_path, "# Review\n").expect("markdown file should be written");

    let mut child = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("--no-open")
        .arg("--port")
        .arg(port.to_string())
        .arg("--history-dir")
        .arg(&cli_history_dir)
        .arg(&markdown_path)
        .current_dir(temp_dir.path())
        .env("HOME", &home_dir)
        .env_remove("DISCUSS_LOG")
        .env_remove("DISCUSS_HISTORY_DIR")
        .env_remove("DISCUSS_NO_SAVE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn discuss binary");
    let stderr = child.stderr.take().expect("stderr pipe should be present");
    let line_rx = read_first_line(stderr);

    let line = line_rx
        .recv_timeout(STARTUP_TIMEOUT)
        .expect("listening line should be written")
        .expect("stderr line should be readable");
    assert_eq!(line, format!("review UI/API: http://127.0.0.1:{port}\n"));

    let response = post_done(port);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "done response should be 200, got {response:?}"
    );
    let output = wait_with_timeout(child, Duration::from_secs(2));

    assert_eq!(output.status.code(), Some(0));
    assert_session_done_emitted(&output);
    assert_eq!(json_files_in(&cli_history_dir.join("review")).len(), 1);
    assert!(
        json_files_in(&config_history_dir.join("review")).is_empty(),
        "--history-dir should override configured history_dir"
    );
}

#[test]
fn cli_no_save_flag_suppresses_history_archive() {
    let port = free_port();
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");
    let history_dir = temp_dir.path().join("history");
    let markdown_path = temp_dir.path().join("review.md");
    fs::write(&markdown_path, "# Review\n").expect("markdown file should be written");

    let mut child = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("--no-open")
        .arg("--no-save")
        .arg("--port")
        .arg(port.to_string())
        .arg("--history-dir")
        .arg(&history_dir)
        .arg(&markdown_path)
        .current_dir(temp_dir.path())
        .env("HOME", &home_dir)
        .env_remove("DISCUSS_LOG")
        .env_remove("DISCUSS_NO_SAVE")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn discuss binary");
    let stderr = child.stderr.take().expect("stderr pipe should be present");
    let line_rx = read_first_line(stderr);

    let line = line_rx
        .recv_timeout(STARTUP_TIMEOUT)
        .expect("listening line should be written")
        .expect("stderr line should be readable");
    assert_eq!(line, format!("review UI/API: http://127.0.0.1:{port}\n"));

    let response = post_done(port);
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "done response should be 200, got {response:?}"
    );
    let output = wait_with_timeout(child, Duration::from_secs(2));

    assert_eq!(output.status.code(), Some(0));
    assert_session_done_emitted(&output);
    assert!(
        json_files_in(&history_dir.join("review")).is_empty(),
        "--no-save should suppress history archive writes"
    );
}

#[test]
fn cli_demo_serves_bundled_session_with_normal_stdout_semantics() {
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");

    // Top-level flags must precede the subcommand.
    let mut child = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("--no-open")
        .arg("demo")
        .current_dir(temp_dir.path())
        .env("HOME", &home_dir)
        .env_remove("DISCUSS_LOG")
        .env_remove("DISCUSS_PORT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn discuss binary");
    let stdout_rx = read_all_lines(child.stdout.take().expect("stdout pipe should be present"));
    let stderr = child.stderr.take().expect("stderr pipe should be present");
    let line_rx = read_first_line(stderr);

    let line = line_rx
        .recv_timeout(STARTUP_TIMEOUT)
        .expect("listening line should be written")
        .expect("stderr line should be readable");
    let base_url = line
        .strip_prefix("review UI/API: ")
        .expect("listening line prefix")
        .trim()
        .to_string();
    let port = url::Url::parse(&base_url)
        .expect("listening URL")
        .port()
        .expect("listening port");

    let started_line = stdout_rx
        .recv_timeout(STARTUP_TIMEOUT)
        .expect("session.started should be emitted")
        .expect("stdout line should be readable");
    let started: Value = serde_json::from_str(&started_line).expect("startup line should be JSON");
    assert_eq!(started["kind"], "session.started");
    assert_eq!(started["payload"]["mode"], "demo");
    assert_eq!(started["payload"]["source_file"], "demo");
    assert_eq!(started["payload"]["files_count"], 6);
    assert_endpoint_contract(&started["payload"], &base_url);

    // Bundled page: sidebar shell plus the seeded agent threads.
    let page = http_request_url(&base_url, "GET", None);
    assert!(page.starts_with("HTTP/1.1 200"), "{}", &page[..60]);
    assert!(page.contains("id=\"file-sidebar\""));
    assert!(page.contains("file-sidebar-toggle"));
    for seeded in ["\"a-1\"", "\"a-2\"", "\"a-3\"", "\"a-4\""] {
        assert!(page.contains(seeded), "page should seed thread {seeded}");
    }

    // Embedded GIF served with the image contract (raw bytes, so no utf-8 read).
    let raw_headers = http_request_head_bytes(port, "/api/files/f-1/raw");
    assert!(raw_headers.starts_with("HTTP/1.1 200"), "{raw_headers}");
    assert!(
        raw_headers
            .to_ascii_lowercase()
            .contains("content-type: image/gif")
    );

    // Embedded HTML prototype served from memory.
    let prototype = http_request_url(&format!("{base_url}/files/f-6"), "GET", None);
    assert!(
        prototype.starts_with("HTTP/1.1 200"),
        "{}",
        &prototype[..60]
    );
    assert!(prototype.contains("Ledgerly"));

    // A user thread gets a canned Demo agent response after the delay, and
    // stdout keeps normal wire semantics: no take-shaped events, ever.
    let create_body = serde_json::json!({
        "fileId": "f-2",
        "anchorStart": 3,
        "anchorEnd": 3,
        "snippet": "failure rate",
        "text": "Is 2.1% still current?",
    })
    .to_string();
    let create_response = http_request_url(
        &format!("{base_url}/api/threads"),
        "POST",
        Some(&create_body),
    );
    assert!(
        create_response.starts_with("HTTP/1.1 200"),
        "{create_response}"
    );
    let thread_id = response_json(&create_response)["id"]
        .as_str()
        .expect("created thread should have an id")
        .to_string();

    // Wait past the 1.5 s responder delay for the canned take.
    let deadline = Instant::now() + Duration::from_secs(5);
    let take_text = loop {
        let state_response = http_request_url(&format!("{base_url}/api/state"), "GET", None);
        let state = response_json(&state_response);
        if let Some(takes) = state["takes"][&thread_id].as_array()
            && !takes.is_empty()
        {
            break takes[0]["text"].as_str().expect("take text").to_string();
        }
        assert!(
            Instant::now() < deadline,
            "demo responder should add a take within the deadline"
        );
        thread::sleep(Duration::from_millis(100));
    };
    assert!(take_text.starts_with("Demo agent — "));

    let response = post_done(port);
    assert!(response.starts_with("HTTP/1.1 200"), "{response}");
    let output = wait_with_timeout(child, Duration::from_secs(2));
    assert_eq!(output.status.code(), Some(0));

    // Drain the remaining stdout lines (the pipe was consumed line-by-line
    // above): session.done must be present, take events must not.
    let mut events = vec![started];
    while let Ok(line) = stdout_rx.recv_timeout(Duration::from_millis(200)) {
        let line = line.expect("stdout line should be readable");
        events.push(serde_json::from_str(&line).expect("stdout line should be JSON"));
    }
    assert!(
        events.iter().any(|event| event["kind"] == "session.done"),
        "stdout should contain session.done, got {events:?}"
    );
    assert!(
        events.iter().all(|event| event["kind"] != "take.added"),
        "takes are SSE-only; stdout must stay take-free: {events:?}"
    );
}

#[test]
fn cli_demo_with_file_arguments_exits_two() {
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");

    let output = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("somefile.md")
        .arg("demo")
        .current_dir(temp_dir.path())
        .env("HOME", &home_dir)
        .env_remove("DISCUSS_LOG")
        .output()
        .expect("spawn discuss binary");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "stdout should be reserved for JSON events"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(stderr.contains("`discuss demo` does not accept file arguments"));
}

#[test]
fn cli_bad_verdict_options_exits_two_and_reports_message() {
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");
    let markdown_path = temp_dir.path().join("review.md");
    fs::write(&markdown_path, "# Review\n").expect("markdown file should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("--verdict-options")
        .arg("approved")
        .arg(&markdown_path)
        .env("HOME", &home_dir)
        .env_remove("DISCUSS_LOG")
        .output()
        .expect("spawn discuss binary");

    assert_eq!(output.status.code(), Some(2));
    assert!(
        output.stdout.is_empty(),
        "stdout should be reserved for JSON events"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(stderr.contains("verdict options error"));
    assert!(stderr.contains("at least 2 options"));
}

struct RunningSession {
    child: Option<Child>,
    stdout: mpsc::Receiver<io::Result<String>>,
    stderr: mpsc::Receiver<io::Result<String>>,
}

#[cfg(unix)]
fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents).expect("write executable fixture");
    let mut permissions = fs::metadata(path).expect("fixture metadata").permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions).expect("make fixture executable");
}

fn occupied_proxy_pair() -> (u16, TcpListener) {
    let start = 32_000 + (std::process::id() as u16 % 8_000);
    for api_port in start..42_000 {
        if let Ok(api_probe) = TcpListener::bind((Ipv4Addr::LOCALHOST, api_port))
            && let Ok(proxy_listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, api_port + 1))
        {
            drop(api_probe);
            return (api_port, proxy_listener);
        }
    }
    panic!("could not reserve adjacent proxy collision ports");
}

fn free_adjacent_ports() -> u16 {
    let start = 20_000 + (std::process::id() as u16 % 8_000);
    for first in start..30_000 {
        if let Ok(first_listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, first))
            && let Ok(second_listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, first + 1))
        {
            drop((first_listener, second_listener));
            return first;
        }
    }
    panic!("could not find adjacent loopback ports");
}

fn spawn_dynamic_session(cwd: &Path, home: &Path, markdown: &Path) -> RunningSession {
    let mut child = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg("--no-open")
        .arg("--no-save")
        .arg(markdown)
        .current_dir(cwd)
        .env("HOME", home)
        .env_remove("DISCUSS_LOG")
        .env_remove("DISCUSS_PORT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn discuss binary");

    let stdout = read_all_lines(child.stdout.take().expect("stdout pipe should be present"));
    let stderr = read_all_lines(child.stderr.take().expect("stderr pipe should be present"));

    RunningSession {
        child: Some(child),
        stdout,
        stderr,
    }
}

fn receive_startup(session: &RunningSession, label: &str) -> Value {
    let line = session
        .stdout
        .recv_timeout(STARTUP_TIMEOUT)
        .unwrap_or_else(|error| panic!("{label} session should emit session.started: {error}"))
        .expect("stdout line should be readable");
    serde_json::from_str(&line).expect("startup line should be JSON")
}

fn startup_base_url(started: &Value) -> String {
    started["payload"]["apiBaseUrl"]
        .as_str()
        .expect("apiBaseUrl should be a string")
        .to_string()
}

fn assert_startup_contract(session: &RunningSession, started: &Value, base_url: &str) {
    assert_eq!(started["kind"], "session.started");
    assert_eq!(started["payload"]["url"], base_url);
    assert!(started["payload"].get("proxyUrl").is_none());
    assert_endpoint_contract(&started["payload"], base_url);

    let stderr = session
        .stderr
        .recv_timeout(STARTUP_TIMEOUT)
        .expect("stderr should report the bound URL")
        .expect("stderr line should be readable");
    assert_eq!(stderr, format!("review UI/API: {base_url}"));
}

fn assert_endpoint_contract(payload: &Value, base_url: &str) {
    assert_eq!(
        payload["endpoints"],
        serde_json::json!({
            "state": format!("{base_url}/api/state"),
            "events": format!("{base_url}/api/events"),
            "createThread": format!("{base_url}/api/threads"),
            "addTakeTemplate": format!("{base_url}/api/threads/{{threadId}}/takes"),
            "blocksTemplate": format!("{base_url}/api/files/{{fileId}}/blocks"),
            "done": format!("{base_url}/api/done"),
        })
    );
    assert_eq!(
        payload["agentInstructions"],
        serde_json::json!([
            "Use payload.endpoints; do not assume port 7777.",
            "On thread.created, POST a take to addTakeTemplate with {threadId} replaced.",
            "If you edited the reviewed document, you may pre-annotate it: POST createThread with kind=\"agent\" to leave leading takes; use blocksTemplate to compute anchors.",
            "Stop when session.done is received."
        ])
    );
}

fn create_thread_and_take(started: &Value, question: &str, take: &str) -> Value {
    let endpoints = &started["payload"]["endpoints"];
    let create_url = endpoints["createThread"]
        .as_str()
        .expect("createThread endpoint should be a string");
    let create_body = serde_json::json!({
        "anchorStart": 1,
        "anchorEnd": 1,
        "snippet": "Review",
        "text": question,
    })
    .to_string();
    let create_response = http_request_url(create_url, "POST", Some(&create_body));
    assert!(
        create_response.starts_with("HTTP/1.1 200"),
        "{create_response}"
    );
    let thread_id = response_json(&create_response)["id"]
        .as_str()
        .expect("created thread should have an id")
        .to_string();

    let take_url = endpoints["addTakeTemplate"]
        .as_str()
        .expect("addTakeTemplate endpoint should be a string")
        .replace("{threadId}", &thread_id);
    let take_body = serde_json::json!({ "text": take }).to_string();
    let take_response = http_request_url(&take_url, "POST", Some(&take_body));
    assert!(take_response.starts_with("HTTP/1.1 200"), "{take_response}");

    let state_url = endpoints["state"]
        .as_str()
        .expect("state endpoint should be a string");
    let state_response = http_request_url(state_url, "GET", None);
    assert!(
        state_response.starts_with("HTTP/1.1 200"),
        "{state_response}"
    );
    response_json(&state_response)
}

fn http_request_url(url: &str, method: &str, body: Option<&str>) -> String {
    let remainder = url
        .strip_prefix("http://")
        .unwrap_or_else(|| panic!("expected HTTP endpoint, got {url:?}"));
    let (authority, path) = remainder
        .split_once('/')
        .map(|(authority, path)| (authority, format!("/{path}")))
        .unwrap_or((remainder, "/".to_string()));
    assert!(authority.starts_with("127.0.0.1:"), "{authority}");

    let mut stream = TcpStream::connect(authority).expect("connect to reported endpoint");
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .expect("write endpoint request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read endpoint response");
    response
}

/// Reads only the response head for endpoints whose bodies are not utf-8
/// (e.g. the raw image route).
fn http_request_head_bytes(port: u16, path: &str) -> String {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect to discuss");
    let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .expect("write raw request");

    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("read raw response");
    let head_end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .unwrap_or(response.len());
    String::from_utf8_lossy(&response[..head_end]).into_owned()
}

fn response_body(response: &str) -> &str {
    response
        .split_once("\r\n\r\n")
        .expect("HTTP response should contain a body")
        .1
}

fn response_json(response: &str) -> Value {
    serde_json::from_str(response_body(response)).expect("HTTP response body should be JSON")
}

fn doc_content(body: &str) -> &str {
    let open = "<section id=\"doc-content\">";
    let start = body.find(open).expect("doc-content open") + open.len();
    let end = body[start..]
        .find("</section>")
        .map(|offset| start + offset)
        .expect("doc-content close");
    &body[start..end]
}

fn stamped_indices(html: &str) -> Vec<u64> {
    html.split("data-anchor-idx=\"")
        .skip(1)
        .map(|suffix| {
            suffix
                .split_once('"')
                .expect("anchor stamp should have a closing quote")
                .0
                .parse()
                .expect("anchor stamp should be numeric")
        })
        .collect()
}

#[derive(Clone, Copy)]
enum PortSource {
    Environment,
    ProjectConfig,
}

fn assert_explicit_port_collision(source: PortSource) {
    let busy_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind busy listener");
    let busy_port = busy_listener
        .local_addr()
        .expect("busy listener addr")
        .port();
    let temp_dir = tempdir().expect("tempdir should be created");
    let home_dir = temp_dir.path().join("home");
    fs::create_dir(&home_dir).expect("home dir should be created");
    let markdown_path = temp_dir.path().join("review.md");
    fs::write(&markdown_path, "# Review\n").expect("markdown file should be written");

    if matches!(source, PortSource::ProjectConfig) {
        fs::write(
            temp_dir.path().join("discuss.config.toml"),
            format!("port = {busy_port}\n"),
        )
        .expect("project config should be written");
    }

    let mut command = Command::new(env!("CARGO_BIN_EXE_discuss"));
    command
        .arg("--no-open")
        .arg(&markdown_path)
        .current_dir(temp_dir.path())
        .env("HOME", &home_dir)
        .env_remove("DISCUSS_LOG")
        .env_remove("DISCUSS_PORT")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if matches!(source, PortSource::Environment) {
        command.env("DISCUSS_PORT", busy_port.to_string());
    }

    let child = command.spawn().expect("spawn discuss binary");
    let output = wait_with_timeout(child, Duration::from_secs(2));
    assert_eq!(output.status.code(), Some(3));
    assert!(
        output.stdout.is_empty(),
        "collision must emit no stdout event"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(stderr.contains(&format!("port {busy_port}")), "{stderr}");
    assert!(stderr.contains("clear the port override"), "{stderr}");
}

fn free_port() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind free listener");

    listener.local_addr().expect("free listener addr").port()
}

fn read_first_line<R>(reader: R) -> mpsc::Receiver<io::Result<String>>
where
    R: Read + Send + 'static,
{
    let (line_tx, line_rx) = mpsc::channel();

    thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut line = String::new();
        let result = reader.read_line(&mut line).map(|_| line);
        let _ = line_tx.send(result);
    });

    line_rx
}

fn read_all_lines<R>(reader: R) -> mpsc::Receiver<io::Result<String>>
where
    R: Read + Send + 'static,
{
    let (line_tx, line_rx) = mpsc::channel();

    thread::spawn(move || {
        for line in BufReader::new(reader).lines() {
            if line_tx.send(line).is_err() {
                break;
            }
        }
    });

    line_rx
}

fn kill_and_collect(mut child: Child) -> Output {
    let _ = child.kill();
    child.wait_with_output().expect("collect child output")
}

fn assert_rfc3339(value: &str) {
    DateTime::parse_from_rfc3339(value).expect("timestamp should be RFC3339");
}

fn post_done(port: u16) -> String {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).expect("connect to discuss");
    let request = "POST /api/done HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    stream
        .write_all(request.as_bytes())
        .expect("write done request");

    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read done response");
    response
}

fn assert_session_done_emitted(output: &Output) {
    let stdout = std::str::from_utf8(&output.stdout).expect("stdout should be utf-8");
    let events = stdout
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("stdout line should be JSON"))
        .collect::<Vec<_>>();

    assert!(
        events.iter().any(|event| event["kind"] == "session.done"),
        "stdout should contain session.done event, got {stdout}"
    );
}

fn json_files_in(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    entries
        .map(|entry| entry.expect("history entry should be readable").path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("json"))
        .collect()
}

fn wait_with_timeout(mut child: Child, duration: Duration) -> Output {
    let deadline = Instant::now() + duration;

    loop {
        if child.try_wait().expect("poll child").is_some() {
            return child.wait_with_output().expect("collect child output");
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child.wait_with_output().expect("collect timed out output");
            panic!(
                "discuss did not exit within {:?}; stdout: {}; stderr: {}",
                duration,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        thread::sleep(Duration::from_millis(10));
    }
}
