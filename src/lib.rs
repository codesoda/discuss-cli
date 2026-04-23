pub mod cli;
pub mod config;
pub mod error;
pub mod exit;
pub mod logging;

pub use config::{Config, ConfigOverrides};
pub use error::{DiscussError, Result};
pub use exit::exit_code_for_error;
pub use logging::init_tracing;

pub fn run(args: cli::Args) -> Result<()> {
    let config = Config::resolve(ConfigOverrides::default())?;
    init_tracing(&config)?;
    tracing::debug!("tracing initialized");

    match args.command {
        Some(cli::Commands::Update) | None => Ok(()),
    }
}
