use std::fs;
use std::future::{Future, pending};
use std::io::{self, IsTerminal, Read};
use std::net::{Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

use chrono::Utc;
use clap::CommandFactory;

pub mod assets;
pub mod cli;
pub mod config;
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
pub use transcript::{Transcript, TranscriptThread, build_transcript};

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
        file,
        command,
    } = args;

    if command.is_none() && file.is_none() && io::stdin().is_terminal() {
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
        None => {
            let input =
                resolve_input(file)?.expect("no-input case is short-circuited before tracing init");
            let MarkdownInput {
                markdown_source,
                source_path,
                source_file,
            } = input;
            let port = config.port.unwrap_or(DEFAULT_PORT);
            let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            let auto_open = config.auto_open;
            let mut app_state = AppState::for_process()
                .with_markdown_source(markdown_source)
                .with_no_save(config.no_save)
                .with_idle_timeout_secs(config.idle_timeout_secs);
            if let Some(source_path) = source_path {
                app_state = app_state.with_source_path(source_path);
            }
            if let Some(history_dir) = config.history_dir.clone() {
                app_state = app_state.with_history_dir(history_dir);
            }
            let emitter = app_state.emitter.clone();

            server::serve_with_ready(addr, app_state, shutdown, move |listening_addr| {
                let url = launch::loopback_url(listening_addr);
                let started_at = Utc::now();

                if let Err(error) = emitter.emit(&Event {
                    kind: EventKind::SessionStarted,
                    at: started_at,
                    payload: serde_json::json!({
                        "url": url.clone(),
                        "source_file": source_file,
                        "started_at": started_at.to_rfc3339(),
                    }),
                }) {
                    tracing::warn!(
                        %url,
                        error = %error,
                        "failed to emit session.started event"
                    );
                }

                let launcher = launch::SystemBrowserLauncher;
                let mut stderr = io::stderr();

                if let Err(error) =
                    launch::announce_listening(&mut stderr, &launcher, &url, auto_open)
                {
                    tracing::warn!(
                        %url,
                        error = %error,
                        "failed to write listening URL to stderr"
                    );
                }
            })
            .await
        }
    }
}

struct MarkdownInput {
    markdown_source: String,
    source_path: Option<PathBuf>,
    source_file: String,
}

fn resolve_input(file: Option<PathBuf>) -> Result<Option<MarkdownInput>> {
    match file {
        Some(ref path) if path.as_os_str() == "-" => Ok(Some(read_markdown_stdin()?)),
        Some(path) => {
            let markdown_source = read_markdown_file(&path)?;
            let source_file = source_file_for_event(&path);
            Ok(Some(MarkdownInput {
                markdown_source,
                source_path: Some(path),
                source_file,
            }))
        }
        None if !io::stdin().is_terminal() => Ok(Some(read_markdown_stdin()?)),
        None => Ok(None),
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
    fn resolve_input_with_file_returns_file_metadata() {
        let temp_dir = tempdir().expect("tempdir");
        let path = temp_dir.path().join("plan.md");
        fs::write(&path, "# hello").expect("write fixture");

        let input = resolve_input(Some(path.clone()))
            .expect("file path should resolve")
            .expect("file path should yield input");

        assert_eq!(input.markdown_source, "# hello");
        assert_eq!(input.source_path.as_deref(), Some(path.as_path()));
        assert!(!input.source_file.is_empty());
        assert_ne!(input.source_file, "<stdin>");
    }
}
