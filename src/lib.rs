use std::fs;
use std::future::{pending, Future};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::Path;

use chrono::Utc;

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
pub use launch::{announce_listening, loopback_url, SystemBrowserLauncher};
pub use logging::init_tracing;
pub use render::render;
pub use server::{serve, serve_with_ready, AppState};
pub use sse::{BroadcastEvent, EventBus};
pub use template::render_page;
pub use transcript::{build_transcript, Transcript, TranscriptThread};

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
            }

            Ok(())
        }
        None => {
            let Some(file) = file else {
                return Ok(());
            };
            let markdown_source = read_markdown_file(&file)?;
            let source_file = source_file_for_event(&file);
            let port = config.port.unwrap_or(DEFAULT_PORT);
            let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
            let auto_open = config.auto_open;
            let mut app_state = AppState::for_process()
                .with_markdown_source(markdown_source)
                .with_source_path(file.clone())
                .with_no_save(config.no_save)
                .with_idle_timeout_secs(config.idle_timeout_secs);
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
}
