use std::process::Command;

use tempfile::tempdir;

#[test]
fn cli_tracing_writes_no_stdout_or_stderr_on_success() {
    let temp_home = tempdir().expect("temp home should be created");
    let temp_project = tempdir().expect("temp project should be created");
    let markdown_path = temp_project.path().join("review.md");
    std::fs::write(&markdown_path, "# Review\n").expect("markdown file should be written");

    let output = Command::new(env!("CARGO_BIN_EXE_discuss"))
        .arg(&markdown_path)
        .env("HOME", temp_home.path())
        .env("DISCUSS_LOG", "debug")
        .output()
        .expect("discuss should run");

    assert!(output.status.success());
    assert!(
        output.stdout.is_empty(),
        "stdout should be reserved for events"
    );
    assert!(
        output.stderr.is_empty(),
        "stderr should not receive tracing output"
    );

    let log_dir = temp_home.path().join(".discuss").join("logs");
    let log_files = std::fs::read_dir(&log_dir)
        .expect("log dir should exist")
        .map(|entry| entry.expect("log entry should be readable").path())
        .collect::<Vec<_>>();

    assert_eq!(log_files.len(), 1);
    assert!(std::fs::read_to_string(&log_files[0])
        .expect("log file should be readable")
        .contains("tracing initialized"));
}
