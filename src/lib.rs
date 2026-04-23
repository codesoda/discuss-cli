pub mod cli;
pub mod config;
pub mod error;
pub mod exit;

pub use config::Config;
pub use error::{DiscussError, Result};
pub use exit::exit_code_for_error;

pub fn run(args: cli::Args) -> Result<()> {
    match args.command {
        Some(cli::Commands::Update) | None => Ok(()),
    }
}
