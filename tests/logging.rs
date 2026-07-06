use std::process::Command;

use tempfile::tempdir;

#[test]
fn cli_tracing_uses_file_logs_without_polluting_stderr() {
    let temp_home = tempdir().expect("temp home should be created");
    let missing_path = temp_home.path().join("missing.md");

    let output = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg(&missing_path)
        .env("HOME", temp_home.path())
        .env("DISCUSS_LOG", "debug")
        .output()
        .expect("discuss should run");

    assert_eq!(output.status.code(), Some(1));
    assert!(
        output.stdout.is_empty(),
        "stdout should be reserved for events"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(stderr.contains("file not found"));
    assert!(
        !stderr.contains("tracing initialized"),
        "stderr should only contain the user-facing error"
    );

    let log_dir = temp_home.path().join(".discuss").join("logs");
    let log_files = std::fs::read_dir(&log_dir)
        .expect("log dir should exist")
        .map(|entry| entry.expect("log entry should be readable").path())
        .collect::<Vec<_>>();

    assert_eq!(log_files.len(), 1);
    assert!(
        std::fs::read_to_string(&log_files[0])
            .expect("log file should be readable")
            .contains("tracing initialized")
    );
}
