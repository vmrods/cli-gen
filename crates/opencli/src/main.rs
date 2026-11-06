mod audit;
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
mod plugins;
mod progress;
mod prompt;
mod server;
mod source;

use std::io::Write as _;
use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

use crate::error::CliError;

const DEFAULT_LOG_FILTER: &str = "warn";
const VERBOSE_LOG_FILTER: &str = "warn,opencli=debug";
const VERBOSE_FLAGS: &[&str] = &["-v", "--verbose"];

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    init_tracing(is_verbose(&args));
    match dispatch::run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        // The plugin already reported its own failure.
        Err(CliError::PluginExited(code)) => ExitCode::from(code),
        Err(error) => {
            let code = error.exit_code();
            // Nothing useful is left to do if stderr itself is gone.
            let _ = writeln!(std::io::stderr(), "{:?}", miette::Report::new(error));
            ExitCode::from(code)
        }
    }
}

/// Logging starts before clap parses, so `-v` is spotted directly.
fn is_verbose(args: &[std::ffi::OsString]) -> bool {
    args.iter()
        .skip(1)
        .map_while(|arg| arg.to_str().filter(|arg| *arg != "--"))
        .any(|arg| VERBOSE_FLAGS.contains(&arg))
}

/// Diagnostics go to stderr so stdout stays clean for `--output json`.
fn init_tracing(verbose: bool) {
    let fallback = if verbose {
        VERBOSE_LOG_FILTER
    } else {
        DEFAULT_LOG_FILTER
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(fallback));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
