pub mod assets;
pub mod cli;
pub mod config;
pub mod error;
pub mod events;
pub mod exit;
pub mod logging;
pub mod render;
pub mod server;
pub mod sse;
pub mod state;
pub mod template;

pub use config::{Config, ConfigOverrides};
pub use error::{DiscussError, Result};
pub use events::{Event, EventEmitter, EventKind};
pub use exit::exit_code_for_error;
pub use logging::init_tracing;
pub use render::render;
pub use server::{serve, AppState};
pub use sse::{BroadcastEvent, EventBus};
pub use template::render_page;

pub fn run(args: cli::Args) -> Result<()> {
    let config = Config::resolve(ConfigOverrides::default())?;
    init_tracing(&config)?;
    tracing::debug!("tracing initialized");

    match args.command {
        Some(cli::Commands::Update) | None => Ok(()),
    }
}
