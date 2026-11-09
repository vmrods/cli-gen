use clap::{Parser, Subcommand, ValueEnum};
use opencli_core::ApiName;

use crate::output::ResponseQuery;

pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

const ROOT_EXAMPLES: &str = "\
Examples:
  Add an API (asks for the spec and base URL when you leave them out):
    opencli api add crm --spec ./crm.yaml
    opencli --server https://api.example.com api add crm --spec ./crm.yaml

  Sign in (the secret is asked for, hidden):
    opencli auth login crm
    opencli auth status crm

  Explore what an API offers:
    opencli api list
    opencli api show crm
    opencli api commands crm login                  # find commands by name, path or summary
    opencli crm --help
    opencli crm users post --describe

  Call it (the URL path is the command; GET has no method word):
    opencli crm users --limit 10                    # GET /users
    opencli crm users --id 123 -o json              # GET /users/{id}
    opencli crm users --query data.0.email
    opencli crm users post --name Ada --dry-run     # POST /users
    opencli crm users post --body @user.json
    opencli crm users post -i
    opencli crm users delete --id 123 --yes         # DELETE /users/{id}

  Pass inputs (in a terminal, missing required values are asked for):
    --body @file.json | --body '{...}' | --body -   the whole request body
    -i, --interactive                               ask for every flag and body field
    --no-validate                                   send a body that doesn't match the spec

  Keep it up to date:
    opencli api refresh crm --dry-run
    opencli api refresh crm
    opencli api set-server crm https://staging.example.com

  Extend and integrate:
    opencli plugin install ./crm-helpers.toml
    opencli completion zsh > ~/.zfunc/_opencli

More examples: opencli api --help, opencli auth --help, opencli plugin --help";

const API_EXAMPLES: &str = "\
Examples:
  opencli api add crm --spec ./crm.yaml                   # JSON or YAML; Swagger 2.0 or OpenAPI 3.x
  opencli api add crm --spec https://example.com/api.json
  opencli --server https://api.example.com api add crm --spec ./crm.yaml
  opencli api list
  opencli api show crm
  opencli api commands crm                                # every command, with its endpoint
  opencli api commands crm login                          # only those matching every word
  opencli api set-server crm https://staging.example.com
  opencli api set-server crm --clear                      # follow the spec's servers again
  opencli api refresh crm --dry-run                       # what would change
  opencli api refresh crm --spec ./crm-v2.yaml            # import from a new source
  opencli api versions crm
  opencli api remove crm --yes";

const API_ADD_EXAMPLES: &str = "\
Examples:
  opencli api add crm                                     # asks for the spec and base URL
  opencli api add crm --spec ./crm.yaml                   # asks for the base URL
  opencli api add crm --spec https://example.com/api.json
  opencli --server https://api.example.com api add crm --spec ./crm.yaml   # no questions";

const AUTH_EXAMPLES: &str = "\
Examples:
  opencli auth login crm                                  # asks which scheme, then for the secret
                                                          # or signs in with the API's own login
                                                          # (running that login command saves the token too)
  printf %s \"$TOKEN\" | opencli auth login crm --scheme bearer --with-token
  opencli auth login crm --scheme basic --username ada
  opencli auth login crm --scheme oauth --client-id my-app --flow authorization-code
  opencli auth status crm                                 # never shows secrets
  opencli auth logout crm";

const PLUGIN_EXAMPLES: &str = "\
Examples:
  opencli plugin install ./crm-helpers.toml               # shows what it adds and runs, then asks
  opencli plugin list
  opencli plugin remove crm-helpers --yes";

const COMPLETION_EXAMPLES: &str = "\
Examples:
  opencli completion zsh > ~/.zfunc/_opencli
  opencli completion bash > ~/.local/share/bash-completion/completions/opencli";

/// Turn imported OpenAPI specifications into CLI commands.
#[derive(Debug, Parser)]
// `bin_name` keeps usage lines saying `opencli` on every platform, not
// `opencli.exe` or however the binary happened to be launched.
#[command(
    name = "opencli",
    bin_name = "opencli",
    version,
    about,
    arg_required_else_help = true,
    after_help = ROOT_EXAMPLES
)]
pub struct Cli {
    /// Output format.
    #[arg(long, short, global = true, value_enum, default_value_t = OutputFormat::Table)]
    pub output: OutputFormat,

    /// Select a JSON response field before formatting (e.g. data or data.items.0).
    #[arg(long, global = true, value_name = "PATH")]
    pub query: Option<ResponseQuery>,

    /// Base URL to send requests to, overriding the spec's servers.
    #[arg(long, global = true, value_name = "URL")]
    pub server: Option<String>,

    /// Show debug logs, including per-phase timings (RUST_LOG overrides).
    #[arg(long, short, global = true)]
    pub verbose: bool,

    /// Request timeout in seconds.
    #[arg(long, global = true, value_name = "SECONDS", default_value_t = DEFAULT_TIMEOUT_SECONDS,
          value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout: u64,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    Table,
    Json,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Register, inspect and remove APIs.
    #[command(subcommand, after_help = API_EXAMPLES)]
    Api(ApiCommand),
    /// Store, inspect and remove credentials for an API.
    #[command(subcommand, after_help = AUTH_EXAMPLES)]
    Auth(AuthCommand),
    /// Install, list and remove plugins.
    #[command(subcommand, after_help = PLUGIN_EXAMPLES)]
    Plugin(PluginCommand),
    /// Print a shell completion script.
    ///
    /// The script includes the commands of every API registered now; generate
    /// it again after `api add`, `api refresh` or `api remove`.
    #[command(after_help = COMPLETION_EXAMPLES)]
    Completion {
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    /// Install a plugin manifest (plugin.toml) from a file or URL.
    ///
    /// Shows what the plugin adds, and every program it would run, before asking.
    Install {
        /// Path or URL of the plugin.toml.
        source: String,
        /// Install without asking (required when not in a terminal).
        #[arg(long)]
        yes: bool,
    },
    /// List installed manifest plugins and `opencli-*` executables on PATH.
    List,
    /// Remove an installed manifest plugin.
    Remove {
        name: ApiName,
        /// Remove without asking (required when not in a terminal).
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Store credentials for one of the API's security schemes.
    ///
    /// Secrets are read from a hidden prompt, or from stdin with --with-token;
    /// they are never accepted as command-line arguments.
    Login(LoginArgs),
    /// Remove stored credentials.
    Logout {
        name: ApiName,
        /// Only this scheme (default: all of the API's schemes).
        #[arg(long)]
        scheme: Option<String>,
    },
    /// Show which security schemes have credentials, without revealing them.
    Status { name: ApiName },
}

#[derive(Debug, clap::Args)]
pub struct LoginArgs {
    pub name: ApiName,
    /// Security scheme to log in to (required when the API declares several).
    #[arg(long)]
    pub scheme: Option<String>,
    /// OAuth 2.0 flow (default: the first one the spec declares).
    #[arg(long, value_enum)]
    pub flow: Option<FlowArg>,
    /// OAuth 2.0 client ID.
    #[arg(long)]
    pub client_id: Option<String>,
    /// OAuth 2.0 scope to request; repeatable (default: all the flow declares).
    #[arg(long = "scope", value_name = "SCOPE")]
    pub scopes: Vec<String>,
    /// Username for HTTP basic authentication.
    #[arg(long)]
    pub username: Option<String>,
    /// Read the secret (API key, token, password or client secret) from stdin.
    #[arg(long)]
    pub with_token: bool,
    /// Local port for the OAuth redirect, when the provider requires a fixed one.
    #[arg(long, default_value_t = 0)]
    pub redirect_port: u16,
    /// Print the sign-in URL instead of opening a browser.
    #[arg(long)]
    pub no_browser: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum FlowArg {
    ClientCredentials,
    AuthorizationCode,
}

#[derive(Debug, Subcommand)]
pub enum ApiCommand {
    /// Register an API from an OpenAPI document.
    ///
    /// Asks for the base URL requests should go to, offering the spec's
    /// servers; pass the global --server to answer up front.
    #[command(after_help = API_ADD_EXAMPLES)]
    Add {
        /// Name to register the API under; becomes its top-level command.
        name: ApiName,
        /// Path or URL of the OpenAPI document. Prompted for when omitted in a terminal.
        #[arg(long)]
        spec: Option<String>,
    },
    /// List registered APIs.
    List,
    /// List an API's commands, or find them by words in their name, path or summary.
    Commands {
        name: ApiName,
        /// Words a command must all contain (case-insensitive), e.g. `login` or `countries post`.
        words: Vec<String>,
    },
    /// Set the base URL an API's requests go to, or --clear it to follow the spec.
    #[command(group = clap::ArgGroup::new("target").required(true).args(["url", "clear"]))]
    SetServer {
        name: ApiName,
        /// Absolute http:// or https:// URL.
        url: Option<String>,
        /// Forget the stored base URL and use the spec's servers again.
        #[arg(long)]
        clear: bool,
    },
    /// Show an API's servers, security schemes and operations.
    Show { name: ApiName },
    /// List the imported versions of an API's specification.
    Versions { name: ApiName },
    /// Re-import an API's specification from its source and show what changed.
    ///
    /// Stored credentials are kept, except for security schemes the new
    /// version no longer declares.
    Refresh {
        name: ApiName,
        /// Import from this file or URL instead, and make it the new source.
        #[arg(long)]
        spec: Option<String>,
        /// Show the changes without saving them.
        #[arg(long)]
        dry_run: bool,
    },
    /// Remove an API and all its versions.
    Remove {
        name: ApiName,
        /// Skip the confirmation prompt (required when not in a terminal).
        #[arg(long)]
        yes: bool,
    },
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use rstest::rstest;

    use super::*;

    /// The `opencli …` part of an example line, without its shell comment and
    /// redirect, split into arguments.
    fn example_arguments(line: &str) -> Option<Vec<&str>> {
        let command = &line[line.find("opencli ")?..];
        let command = command.split(" #").next()?;
        let command = command.split(" > ").next()?;
        Some(command.split_whitespace().collect())
    }

    #[rstest]
    #[case(API_EXAMPLES)]
    #[case(API_ADD_EXAMPLES)]
    #[case(AUTH_EXAMPLES)]
    #[case(PLUGIN_EXAMPLES)]
    #[case(COMPLETION_EXAMPLES)]
    fn built_in_examples_parse(#[case] examples: &str) {
        let arguments: Vec<Vec<&str>> = examples.lines().filter_map(example_arguments).collect();
        assert_ne!(arguments.len(), 0, "no examples found");
        for example in arguments {
            Cli::try_parse_from(&example)
                .unwrap_or_else(|error| panic!("{example:?} does not parse: {error}"));
        }
    }
}
