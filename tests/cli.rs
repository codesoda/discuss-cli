use std::fs;
use std::io::{BufRead, BufReader};
use std::net::{Ipv4Addr, TcpListener};
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

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
    let (line_tx, line_rx) = mpsc::channel();

    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        let result = reader.read_line(&mut line).map(|_| line);
        let _ = line_tx.send(result);
    });

    let line = line_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("listening line should be written")
        .expect("stderr line should be readable");
    assert_eq!(line, format!("listening on http://127.0.0.1:{port}\n"));

    let _ = child.kill();
    let _ = child.wait();
}

fn free_port() -> u16 {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind free listener");

    listener.local_addr().expect("free listener addr").port()
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
