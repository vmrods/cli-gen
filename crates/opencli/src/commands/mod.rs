mod api;
mod auth;
mod completion;
mod plugin;

use std::time::Duration;

use crate::cli::{Cli, Command};
use crate::error::CliError;
use crate::paths::Paths;

pub async fn run(cli: Cli, paths: &Paths) -> Result<(), CliError> {
    match cli.command {
        Command::Api(command) => api::run(command, cli.output, paths).await,
        Command::Completion { shell } => completion::run(shell, paths).await,
        Command::Plugin(command) => plugin::run(command, cli.output, paths).await,
        Command::Auth(command) => {
            auth::run(command, cli.output, paths, Duration::from_secs(cli.timeout)).await
        }
    }
}
