use std::fs;
use std::future::{Future, pending};
use std::io::{self, Read};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use std::collections::BTreeSet;

use chrono::Utc;
use clap::CommandFactory;

use crate::state::{File, FileId, FileKind, Source};

pub mod assets;
pub mod blocks;
pub mod cli;
pub mod config;
pub mod diff;
pub mod error;
pub mod events;
pub mod exit;
pub mod history;
pub mod launch;
pub mod logging;
pub mod render;
pub mod server;
pub mod sse;
pub mod state;
pub mod template;
pub mod transcript;
pub mod update;

pub use config::{Config, ConfigOverrides};
pub use error::{DiscussError, Result};
pub use events::{Event, EventEmitter, EventKind};
pub use exit::exit_code_for_error;
pub use launch::{SystemBrowserLauncher, announce_listening, loopback_url};
pub use logging::init_tracing;
pub use render::render;
pub use server::{AppState, serve, serve_with_ready};
pub use sse::{BroadcastEvent, EventBus};
pub use template::render_page;
pub use transcript::{
    Transcript, TranscriptThread, build_transcript, build_transcript_with_source,
};

pub const DEFAULT_PORT: u16 = 7777;

pub async fn run(args: cli::Args) -> Result<()> {
    run_with_shutdown(args, pending()).await
}

pub async fn run_with_shutdown<F>(args: cli::Args, shutdown: F) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    let cli::Args {
        port,
        no_open,
        no_save,
        history_dir,
        files,
        command,
    } = args;

    if command.is_none() && files.is_empty() && stdin_is_terminal() {
        eprintln!("{}", cli::Args::command().render_long_help());
        std::process::exit(exit::EXIT_CONFIG_ERROR);
    }

    let config = Config::resolve(ConfigOverrides {
        port,
        auto_open: no_open.then_some(false),
        history_dir,
        no_save: no_save.then_some(true),
        ..ConfigOverrides::default()
    })?;
    init_tracing(&config)?;
    tracing::debug!("tracing initialized");

    match command {
        Some(cli::Commands::Update(update_args)) => {
            if update_args.check {
                eprintln!("{}", update::check()?);
            } else {
                eprintln!("{}", update::install(update_args.yes)?);
            }

            Ok(())
        }
        Some(cli::Commands::Diff(diff_args)) => {
            run_review_session(files, Some(diff_args), &config, shutdown).await
        }
        None => run_review_session(files, None, &config, shutdown).await,
    }
}

async fn run_review_session<F>(
    files: Vec<PathBuf>,
    diff_args: Option<cli::DiffArgs>,
    config: &Config,
    shutdown: F,
) -> Result<()>
where
    F: Future<Output = ()> + Send + 'static,
{
    // In diff mode a bare invocation reviews the diff alone; stdin is only
    // read when explicitly requested with `-`.
    let inputs = if diff_args.is_some() && files.is_empty() {
        Vec::new()
    } else {
        resolve_inputs(files)?.expect("no-input case is short-circuited before tracing init")
    };

    let diff_output = match &diff_args {
        Some(diff_args) => {
            let limit_bytes = diff_args
                .max_diff_bytes
                .or(config.max_diff_bytes)
                .unwrap_or(diff::DEFAULT_DIFF_SIZE_LIMIT_BYTES);
            let limit_bytes = if limit_bytes == 0 {
                usize::MAX
            } else {
                limit_bytes
            };
            let output = diff::run_git_diff(diff_args.unstaged, &diff_args.args, limit_bytes)?;
            if output.files.is_empty() {
                return Err(DiscussError::DiffError {
                    message: "no changes to review".to_string(),
                });
            }
            Some(output)
        }
        None => None,
    };

    let mode = match (&diff_output, inputs.is_empty()) {
        (None, _) => "markdown",
        (Some(_), true) => "diff",
        (Some(_), false) => "mixed",
    };
    let git_args = diff_output
        .as_ref()
        .map(|output| output.git_args.clone())
        .unwrap_or_default();

    let primary_source_path = inputs.iter().find_map(|input| input.source_path.clone());
    let mut session_files: Vec<(String, FileKind, String)> = inputs
        .into_iter()
        .map(|input| (input.source_file, input.kind, input.markdown_source))
        .collect();
    if let Some(output) = diff_output {
        session_files.extend(
            output
                .files
                .into_iter()
                .map(|file| (file.path, FileKind::Diff, file.content)),
        );
    }

    let files_count = session_files.len();
    let session_source_label = if mode == "diff" {
        format!("git {}", git_args.join(" "))
    } else if files_count > 1 {
        format!("multi-{files_count}-files")
    } else {
        session_files
            .first()
            .map(|(path, _, _)| path.clone())
            .unwrap_or_else(|| "<stdin>".to_string())
    };

    let source = Source {
        files: session_files
            .into_iter()
            .enumerate()
            .map(|(idx, (path, kind, content))| File {
                id: FileId(format!("f-{}", idx + 1)),
                path,
                kind,
                content,
            })
            .collect(),
    };

    let port = config.port.unwrap_or(DEFAULT_PORT);
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let auto_open = config.auto_open;

    let mut app_state = AppState::for_process()
        .with_source(source)
        .with_no_save(config.no_save)
        .with_idle_timeout_secs(config.idle_timeout_secs);
    if let Some(source_path) = primary_source_path {
        app_state = app_state.with_source_path(source_path);
    }
    if let Some(history_dir) = config.history_dir.clone() {
        app_state = app_state.with_history_dir(history_dir);
    }
    let emitter = app_state.emitter.clone();

    server::serve_with_ready(addr, app_state, shutdown, move |listening_addr| {
        let url = launch::loopback_url(listening_addr);
        let started_at = Utc::now();

        let mut payload = serde_json::json!({
            "url": url.clone(),
            "mode": mode,
            "source_file": session_source_label,
            "files_count": files_count,
            "started_at": started_at.to_rfc3339(),
        });
        if !git_args.is_empty() {
            payload["git_args"] = serde_json::json!(git_args);
        }

        if let Err(error) = emitter.emit(&Event {
            kind: EventKind::SessionStarted,
            at: started_at,
            payload,
        }) {
            tracing::warn!(
                %url,
                error = %error,
                "failed to emit session.started event"
            );
        }

        let launcher = launch::SystemBrowserLauncher;
        let mut stderr = io::stderr();

        if let Err(error) = launch::announce_listening(&mut stderr, &launcher, &url, auto_open) {
            tracing::warn!(
                %url,
                error = %error,
                "failed to write listening URL to stderr"
            );
        }
    })
    .await
}

/// Whether stdin is attached to an interactive terminal.
///
/// Uses the `is-terminal` crate instead of `std::io::IsTerminal` because on
/// Windows the std trait reports MSYS2/mintty/Git Bash pseudo-ttys (named
/// pipes such as `\\msys-*-pty*`) as non-terminals, which made bare `discuss`
/// block on stdin instead of printing help. See issue #5.
pub(crate) fn stdin_is_terminal() -> bool {
    is_terminal::IsTerminal::is_terminal(&io::stdin())
}

#[derive(Debug)]
struct MarkdownInput {
    markdown_source: String,
    source_path: Option<PathBuf>,
    source_file: String,
    kind: FileKind,
}

fn resolve_inputs(files: Vec<PathBuf>) -> Result<Option<Vec<MarkdownInput>>> {
    if files.is_empty() {
        if stdin_is_terminal() {
            return Ok(None);
        }
        return Ok(Some(vec![read_markdown_stdin()?]));
    }

    let mut stdin_used = false;
    let mut seen_paths: BTreeSet<PathBuf> = BTreeSet::new();
    let mut inputs = Vec::with_capacity(files.len());

    for path in files {
        if path.as_os_str() == "-" {
            if stdin_used {
                return Err(DiscussError::DuplicateInputPath {
                    path: PathBuf::from("-"),
                });
            }
            stdin_used = true;
            inputs.push(read_markdown_stdin()?);
            continue;
        }

        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !seen_paths.insert(canonical) {
            return Err(DiscussError::DuplicateInputPath { path });
        }

        let markdown_source = read_markdown_file(&path)?;
        let source_file = source_file_for_event(&path);
        let kind = file_kind_for_path(&path);
        inputs.push(MarkdownInput {
            markdown_source,
            source_path: Some(path),
            source_file,
            kind,
        });
    }

    Ok(Some(inputs))
}

fn file_kind_for_path(path: &Path) -> FileKind {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .as_deref()
    {
        Some("diff" | "patch") => FileKind::Diff,
        _ => FileKind::Markdown,
    }
}

fn read_markdown_stdin() -> Result<MarkdownInput> {
    let mut markdown_source = String::new();
    io::stdin()
        .read_to_string(&mut markdown_source)
        .map_err(|source| DiscussError::FileNotReadable {
            path: PathBuf::from("<stdin>"),
            source,
        })?;
    Ok(MarkdownInput {
        markdown_source,
        source_path: None,
        source_file: "<stdin>".to_string(),
        kind: FileKind::Markdown,
    })
}

fn source_file_for_event(path: &Path) -> String {
    if let Ok(path) = path.canonicalize() {
        return path.to_string_lossy().into_owned();
    }

    path.file_name()
        .and_then(|file_name| file_name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

fn read_markdown_file(path: &Path) -> Result<String> {
    fs::read_to_string(path).map_err(|source| match source.kind() {
        io::ErrorKind::NotFound => DiscussError::FileNotFound {
            path: path.to_path_buf(),
        },
        _ => DiscussError::FileNotReadable {
            path: path.to_path_buf(),
            source,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::tempdir;

    #[test]
    fn missing_markdown_file_maps_to_file_not_found() {
        let temp_dir = tempdir().expect("tempdir should be created");
        let missing_path = temp_dir.path().join("missing.md");
        let error = read_markdown_file(&missing_path).expect_err("missing file should fail");

        assert!(matches!(error, DiscussError::FileNotFound { .. }));
    }

    #[test]
    fn resolve_inputs_with_single_file_returns_file_metadata() {
        let temp_dir = tempdir().expect("tempdir");
        let path = temp_dir.path().join("plan.md");
        fs::write(&path, "# hello").expect("write fixture");

        let inputs = resolve_inputs(vec![path.clone()])
            .expect("file path should resolve")
            .expect("file path should yield input");

        assert_eq!(inputs.len(), 1);
        let input = &inputs[0];
        assert_eq!(input.markdown_source, "# hello");
        assert_eq!(input.source_path.as_deref(), Some(path.as_path()));
        assert!(!input.source_file.is_empty());
        assert_ne!(input.source_file, "<stdin>");
        assert_eq!(input.kind, FileKind::Markdown);
    }

    #[test]
    fn resolve_inputs_returns_each_file_in_order_with_kinds() {
        let temp_dir = tempdir().expect("tempdir");
        let plan = temp_dir.path().join("plan.md");
        let design = temp_dir.path().join("design.md");
        let patch = temp_dir.path().join("change.patch");
        fs::write(&plan, "plan").expect("write plan");
        fs::write(&design, "design").expect("write design");
        fs::write(&patch, "diff").expect("write patch");

        let inputs = resolve_inputs(vec![plan.clone(), design.clone(), patch.clone()])
            .expect("multi files should resolve")
            .expect("multi files should yield inputs");

        assert_eq!(inputs.len(), 3);
        assert_eq!(inputs[0].source_path.as_deref(), Some(plan.as_path()));
        assert_eq!(inputs[0].kind, FileKind::Markdown);
        assert_eq!(inputs[1].source_path.as_deref(), Some(design.as_path()));
        assert_eq!(inputs[1].kind, FileKind::Markdown);
        assert_eq!(inputs[2].source_path.as_deref(), Some(patch.as_path()));
        assert_eq!(inputs[2].kind, FileKind::Diff);
    }

    #[test]
    fn resolve_inputs_rejects_duplicate_paths() {
        let temp_dir = tempdir().expect("tempdir");
        let path = temp_dir.path().join("plan.md");
        fs::write(&path, "# hello").expect("write fixture");

        let error = resolve_inputs(vec![path.clone(), path.clone()])
            .expect_err("duplicate paths should fail");

        assert!(matches!(error, DiscussError::DuplicateInputPath { .. }));
    }
}
