mod api;
mod auth;
mod completion;
mod plugin;

use std::time::Duration;

use crate::cli::{Cli, Command};
use crate::error::CliError;
use crate::paths::Paths;

pub async fn run(cli: Cli, paths: &Paths) -> Result<(), CliError> {
    if cli.query.is_some() {
        return Err(CliError::ResponseQuery(
            "only API operation responses support --query".to_owned(),
        ));
    }
    match cli.command {
        Command::Mcp(command) => {
            if cli.server.is_some() {
                return Err(CliError::Mcp("use api set-server before launching MCP; per-call server overrides are not supported".into()));
            }
            crate::mcp::serve(command, cli.timeout, paths).await
        }
        Command::Api(command) => api::run(command, cli.output, cli.server.as_deref(), paths).await,
        Command::Completion { shell } => completion::run(shell, paths).await,
        Command::Plugin(command) => plugin::run(command, cli.output, paths).await,
        Command::Auth(command) => {
            auth::run(command, cli.output, paths, Duration::from_secs(cli.timeout)).await
        }
    }
}
