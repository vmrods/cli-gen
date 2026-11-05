mod browser;
mod cache;
mod cli;
mod commands;
mod credentials;
mod dispatch;
mod dynamic;
mod error;
mod output;
mod paths;
mod prompt;
mod server;
mod source;

use std::io::Write as _;
use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

const DEFAULT_LOG_FILTER: &str = "warn";

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    init_tracing();
    match dispatch::run(std::env::args_os().collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let code = error.exit_code();
            // Nothing useful is left to do if stderr itself is gone.
            let _ = writeln!(std::io::stderr(), "{:?}", miette::Report::new(error));
            ExitCode::from(code)
        }
    }
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
