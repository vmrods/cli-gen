use clap::{Parser, Subcommand, ValueEnum};
use opencli_core::ApiName;

pub const DEFAULT_TIMEOUT_SECONDS: u64 = 30;

/// Turn imported OpenAPI specifications into CLI commands.
#[derive(Debug, Parser)]
#[command(name = "opencli", version, about, arg_required_else_help = true)]
pub struct Cli {
    /// Output format.
    #[arg(long, short, global = true, value_enum, default_value_t = OutputFormat::Table)]
    pub output: OutputFormat,

    /// Base URL to send requests to, overriding the spec's servers.
    #[arg(long, global = true, value_name = "URL")]
    pub server: Option<String>,

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
    #[command(subcommand)]
    Api(ApiCommand),
    /// Store, inspect and remove credentials for an API.
    #[command(subcommand)]
    Auth(AuthCommand),
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
    Add {
        /// Name to register the API under; becomes its top-level command.
        name: ApiName,
        /// Path or URL of the OpenAPI document. Prompted for when omitted in a terminal.
        #[arg(long)]
        spec: Option<String>,
    },
    /// List registered APIs.
    List,
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
