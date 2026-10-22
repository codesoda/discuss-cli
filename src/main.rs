use std::process::ExitCode;

use clap::Parser;
use discuss::cli::Args;

#[tokio::main]
async fn main() -> ExitCode {
    // Boxed so the large `run` future lives on the heap; inline it overflows
    // Windows' 1 MB main-thread stack in debug builds.
    match Box::pin(discuss::run(Args::parse())).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(discuss::exit_code_for_error(&error) as u8)
        }
    }
}
