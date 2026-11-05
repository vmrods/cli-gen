mod cli;
mod commands;
mod error;
mod output;
mod paths;
mod prompt;
mod source;

use clap::Parser;
use tracing_subscriber::EnvFilter;

use crate::cli::Cli;

const DEFAULT_LOG_FILTER: &str = "warn";

#[tokio::main(flavor = "current_thread")]
async fn main() -> miette::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    commands::run(cli).await?;
    Ok(())
}

/// Diagnostics go to stderr so stdout stays clean for `--output json`.
fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_LOG_FILTER));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
