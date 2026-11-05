//! `opencli completion <shell>`: a completion script covering built-ins and
//! every registered API's commands as they are now.

use clap::CommandFactory;
use clap_complete::{Shell, generate};
use opencli_core::ApiName;
use opencli_store::Store;

use crate::cache;
use crate::cli::Cli;
use crate::dynamic;
use crate::error::CliError;
use crate::paths::Paths;

const BINARY: &str = "opencli";

pub async fn run(shell: Shell, paths: &Paths) -> Result<(), CliError> {
    let mut root = Cli::command();
    let store = Store::open(&paths.database).await?;
    for summary in store.list_apis().await? {
        let Ok(api) = ApiName::parse(&summary.name) else {
            continue;
        };
        if let Some(cached) = cache::load_or_build(paths, &api).await? {
            root = root.subcommand(dynamic::api_command(&cached));
        }
    }
    generate(shell, &mut root, BINARY, &mut std::io::stdout());
    Ok(())
}
