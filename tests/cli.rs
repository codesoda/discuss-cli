use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use chrono::DateTime;
use serde_json::Value;
use tempfile::tempdir;

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
    assert!(stderr.contains("pass --port <N>"));
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
        .recv_timeout(Duration::from_secs(2))
        .expect("listening line should be written")
        .expect("stderr line should be readable");
    assert_eq!(line, format!("listening on http://127.0.0.1:{port}\n"));

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
        .recv_timeout(Duration::from_secs(2))
        .expect("listening line should be written")
        .expect("stderr line should be readable");
    assert_eq!(line, format!("listening on http://127.0.0.1:{port}\n"));

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
    assert_eq!(event["payload"]["url"], format!("http://127.0.0.1:{port}"));
    assert_eq!(event["payload"]["source_file"], source_file);
    assert_rfc3339(
        event["payload"]["started_at"]
            .as_str()
            .expect("started_at should be a string"),
    );
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
        .recv_timeout(Duration::from_secs(2))
        .expect("listening line should be written")
        .expect("stderr line should be readable");
    assert_eq!(line, format!("listening on http://127.0.0.1:{port}\n"));

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
        .recv_timeout(Duration::from_secs(2))
        .expect("listening line should be written")
        .expect("stderr line should be readable");
    assert_eq!(line, format!("listening on http://127.0.0.1:{port}\n"));

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
