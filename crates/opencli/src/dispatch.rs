//! Assembles the root command: built-ins plus the requested API's commands,
//! loaded at runtime (ADR 0002). Only the API named on the command line is
//! loaded, so dispatch stays fast however many APIs are registered.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::path::Path;
use std::time::{Duration, Instant};

use clap::{CommandFactory, FromArgMatches};
use opencli_core::ApiName;
use opencli_plugin::{Alias, Plugin, expand_alias, find_executable};
use opencli_store::Store;

use crate::cache::{self, CachedCommands};
use crate::cli::{Cli, DEFAULT_TIMEOUT_SECONDS, OutputFormat};
use crate::commands;
use crate::dynamic::{self, RunOptions};
use crate::error::CliError;
use crate::output::ResponseQuery;
use crate::paths::Paths;
use crate::plugins;

const OUTPUT_LONG: &str = "--output";
const OUTPUT_SHORT: &str = "-o";
const OUTPUT_ID: &str = "output";
const QUERY_LONG: &str = "--query";
const QUERY_ID: &str = "query";
const SERVER_LONG: &str = "--server";
const SERVER_ID: &str = "server";
const TIMEOUT_LONG: &str = "--timeout";
const TIMEOUT_ID: &str = "timeout";
/// Lets executable plugins call back into the same `opencli` binary.
const BINARY_VARIABLE: &str = "OPENCLI_BINARY";

pub async fn run(mut args: Vec<OsString>) -> Result<(), CliError> {
    let paths = Paths::resolve().await?;
    let mut root = Cli::command();
    let mut loaded = None;
    let mut extensions = Vec::new();
    if let Some((index, api)) = requested_api(&args) {
        let started = Instant::now();
        loaded = cache::load_or_build(&paths, &api).await?;
        tracing::debug!(%api, elapsed_ms = started.elapsed().as_millis(), "loaded commands");
        match &loaded {
            Some(cached) => {
                extensions = plugins::for_api(&paths, &api).await;
                let aliases = usable_aliases(cached, &extensions);
                if let Some(expanded) = expand_alias(&args, index, &aliases) {
                    tracing::debug!(alias = ?args.get(index + 1), "expanded plugin alias");
                    args = expanded;
                }
                root = root.subcommand(with_alias_help(dynamic::api_command(cached), &aliases));
            }
            // Not an API: maybe an `opencli-<name>` executable plugin. APIs win on a clash.
            None => {
                if let Some(executable) =
                    find_executable(api.as_str(), std::env::var_os("PATH").as_deref())
                {
                    return run_executable(&executable, &args[index + 1..]).await;
                }
            }
        }
    }
    if loaded.is_none() {
        root = with_registered_apis(root, &paths).await;
    }

    let matches = root
        .try_get_matches_from(&args)
        .unwrap_or_else(|error| error.exit());
    if let (Some(cached), Some((name, api_matches))) = (&loaded, matches.subcommand())
        && name == cached.api
    {
        let options = RunOptions {
            query: matches.get_one::<ResponseQuery>(QUERY_ID).cloned(),
            format: matches
                .get_one::<OutputFormat>(OUTPUT_ID)
                .copied()
                .unwrap_or(OutputFormat::Table),
            server: matches.get_one::<String>(SERVER_ID).cloned(),
            timeout: Duration::from_secs(
                matches
                    .get_one::<u64>(TIMEOUT_ID)
                    .copied()
                    .unwrap_or(DEFAULT_TIMEOUT_SECONDS),
            ),
        };
        return dynamic::run(api_matches, cached, &options, &paths, &extensions).await;
    }
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    commands::run(cli, &paths).await
}

/// Aliases from the API's plugins, first plugin (by name) winning. An alias
/// never shadows a real command: the spec is the source of truth.
fn usable_aliases<'a>(
    cached: &CachedCommands,
    extensions: &'a [Plugin],
) -> BTreeMap<String, &'a Alias> {
    let roots: BTreeSet<&str> = cached
        .commands
        .commands
        .iter()
        .filter_map(|command| command.path.first().map(String::as_str))
        .collect();
    let mut aliases = BTreeMap::new();
    for plugin in extensions {
        for (name, alias) in &plugin.aliases {
            if roots.contains(name.as_str()) {
                tracing::warn!(plugin = %plugin.name, alias = %name, "alias shadows a command of the API; ignoring it");
                continue;
            }
            aliases.entry(name.clone()).or_insert(alias);
        }
    }
    aliases
}

fn with_alias_help(command: clap::Command, aliases: &BTreeMap<String, &Alias>) -> clap::Command {
    if aliases.is_empty() {
        return command;
    }
    let lines: Vec<String> = aliases
        .iter()
        .map(|(name, alias)| {
            let target = std::iter::once(alias.command.as_str())
                .chain(alias.args.iter().map(String::as_str));
            let summary = alias
                .description
                .as_deref()
                .map(|text| format!("  ({text})"))
                .unwrap_or_default();
            format!(
                "  {name}  →  {}{summary}",
                target.collect::<Vec<_>>().join(" ")
            )
        })
        .collect();
    command.after_help(format!("Aliases from plugins:\n{}", lines.join("\n")))
}

/// Runs `opencli-<name>` with the remaining arguments and the terminal
/// attached, passing its exit code through.
async fn run_executable(executable: &Path, args: &[OsString]) -> Result<(), CliError> {
    let mut command = tokio::process::Command::new(executable);
    command.args(args);
    if let Ok(binary) = std::env::current_exe() {
        command.env(BINARY_VARIABLE, binary);
    }
    let status = command
        .status()
        .await
        .map_err(|source| CliError::PluginExec {
            path: executable.display().to_string(),
            source,
        })?;
    if status.success() {
        return Ok(());
    }
    let code = status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .filter(|code| *code != 0);
    Err(CliError::PluginExited(code.unwrap_or(1)))
}

/// The first positional argument, if it names an API rather than a built-in.
/// `opencli help <api> …` asks for that API's help, so `help` is skipped.
/// Returns its position too, for alias expansion and executable plugins.
fn requested_api(args: &[OsString]) -> Option<(usize, ApiName)> {
    let mut rest = args
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(index, arg)| Some((index, arg.to_str()?)));
    while let Some((index, arg)) = rest.next() {
        match arg {
            "--" => return None,
            OUTPUT_LONG | OUTPUT_SHORT | SERVER_LONG | TIMEOUT_LONG | QUERY_LONG => {
                rest.next();
            }
            "help" => {}
            flag if flag.starts_with('-') => {}
            positional => return ApiName::parse(positional).ok().map(|api| (index, api)),
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
            if ApiName::parse(&api.name).is_err() {
                tracing::warn!(api = %api.name, "registered API now has a reserved name; use the previous CLI to migrate it");
                return root;
            }
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
    #[case(&["--query", "data", "petstore"], Some("petstore"))]
    #[case(&["--query=data.items", "petstore"], Some("petstore"))]
    #[case(&["--output=json", "petstore"], Some("petstore"))]
    #[case(&["--server", "https://x.example", "--timeout", "5", "petstore"], Some("petstore"))]
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
            requested_api(&args(raw))
                .as_ref()
                .map(|(_, api)| api.as_str()),
            expected
        );
    }
}
