//! Assembles the root command: built-ins plus the requested API's commands,
//! loaded at runtime (ADR 0002). Only the API named on the command line is
//! loaded, so dispatch stays fast however many APIs are registered.

use std::ffi::OsString;

use clap::{CommandFactory, FromArgMatches};
use opencli_core::ApiName;
use opencli_store::Store;

use crate::cache;
use crate::cli::{Cli, OutputFormat};
use crate::commands;
use crate::dynamic;
use crate::error::CliError;
use crate::paths::Paths;

const OUTPUT_LONG: &str = "--output";
const OUTPUT_SHORT: &str = "-o";
const OUTPUT_ID: &str = "output";

pub async fn run(args: Vec<OsString>) -> Result<(), CliError> {
    let paths = Paths::resolve().await?;
    let mut root = Cli::command();
    let mut loaded = None;
    if let Some(api) = requested_api(&args) {
        loaded = cache::load_or_build(&paths, &api).await?;
        if let Some(cached) = &loaded {
            root = root.subcommand(dynamic::api_command(cached));
        }
    }
    if loaded.is_none() {
        root = with_registered_apis(root, &paths).await;
    }

    let matches = root
        .try_get_matches_from(&args)
        .unwrap_or_else(|error| error.exit());
    let format = matches
        .get_one::<OutputFormat>(OUTPUT_ID)
        .copied()
        .unwrap_or(OutputFormat::Table);
    if let (Some(cached), Some((name, api_matches))) = (&loaded, matches.subcommand())
        && name == cached.api
    {
        return dynamic::run(api_matches, cached, format).await;
    }
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    commands::run(cli, &paths).await
}

/// The first positional argument, if it names an API rather than a built-in.
/// `opencli help <api> …` asks for that API's help, so `help` is skipped.
fn requested_api(args: &[OsString]) -> Option<ApiName> {
    let mut rest = args.iter().skip(1).filter_map(|arg| arg.to_str());
    while let Some(arg) = rest.next() {
        match arg {
            "--" => return None,
            OUTPUT_LONG | OUTPUT_SHORT => {
                rest.next();
            }
            "help" => {}
            flag if flag.starts_with('-') => {}
            positional => return ApiName::parse(positional).ok(),
        }
    }
    None
}

/// Lists registered APIs in `opencli --help` (titles only; commands load on use).
/// A nicety: if the store can't be read, built-ins like `--version` still work.
async fn with_registered_apis(root: clap::Command, paths: &Paths) -> clap::Command {
    let apis = match Store::open(&paths.database).await {
        Ok(store) => store.list_apis().await,
        Err(error) => Err(error),
    };
    match apis {
        Ok(apis) => apis.into_iter().fold(root, |root, api| {
            root.subcommand(
                clap::Command::new(api.name).about(format!("{} {}", api.title, api.version)),
            )
        }),
        Err(error) => {
            tracing::warn!(%error, "could not list registered APIs");
            root
        }
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn args(raw: &[&str]) -> Vec<OsString> {
        std::iter::once("opencli")
            .chain(raw.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[rstest]
    #[case(&["petstore", "pets", "list"], Some("petstore"))]
    #[case(&["--output", "json", "petstore", "pets", "list"], Some("petstore"))]
    #[case(&["-o", "json", "petstore"], Some("petstore"))]
    #[case(&["--output=json", "petstore"], Some("petstore"))]
    #[case(&["help", "petstore", "pets"], Some("petstore"))]
    #[case(&["api", "list"], None)]
    #[case(&["--help"], None)]
    #[case(&[], None)]
    #[case(&["--", "petstore"], None)]
    #[case(&["Not_An_Api"], None)]
    fn requested_api_finds_first_positional_api_name(
        #[case] raw: &[&str],
        #[case] expected: Option<&str>,
    ) {
        assert_eq!(
            requested_api(&args(raw)).as_ref().map(ApiName::as_str),
            expected
        );
    }
}
