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
    /// Remove an API and all its versions.
    Remove {
        name: ApiName,
        /// Skip the confirmation prompt (required when not in a terminal).
        #[arg(long)]
        yes: bool,
    },
}
