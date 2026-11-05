mod api;

use crate::cli::{Cli, Command};
use crate::error::CliError;

pub async fn run(cli: Cli) -> Result<(), CliError> {
    match cli.command {
        Command::Api(command) => api::run(command, cli.output).await,
    }
}
