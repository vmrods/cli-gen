mod api;

use crate::cli::{Cli, Command};
use crate::error::CliError;
use crate::paths::Paths;

pub async fn run(cli: Cli, paths: &Paths) -> Result<(), CliError> {
    match cli.command {
        Command::Api(command) => api::run(command, cli.output, paths).await,
    }
}
