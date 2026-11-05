use std::fmt::Display;

use miette::Diagnostic;
use opencli_core::NamingError;
use opencli_runtime::{PlanError, SendError};
use opencli_spec::SpecError;
use opencli_store::StoreError;
use thiserror::Error;

use crate::source::SourceError;

#[derive(Debug, Error, Diagnostic)]
pub enum CliError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Spec(#[from] SpecError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Source(#[from] SourceError),

    #[error(transparent)]
    #[diagnostic(transparent)]
    Store(#[from] StoreFailure),

    #[error("the specification's operations don't map to unique command names")]
    #[diagnostic(code(opencli::naming), help("{details}"))]
    Naming { details: String },

    #[error(transparent)]
    #[diagnostic(code(opencli::request))]
    Plan(#[from] PlanError),

    #[error("could not read the request body from {path}")]
    #[diagnostic(code(opencli::body))]
    BodyFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("the request body is not valid JSON")]
    #[diagnostic(
        code(opencli::body),
        help("Pass JSON inline, as @file.json, or - to read stdin.")
    )]
    BodyJson(#[source] serde_json::Error),

    #[error("sending {method} {url} needs confirmation")]
    #[diagnostic(
        code(opencli::confirmation_required),
        help(
            "Pass --yes to send requests that change data when not running in an interactive terminal."
        )
    )]
    SendConfirmationRequired { method: String, url: String },

    #[error("{method} {url} returned HTTP {status} {reason}")]
    #[diagnostic(code(opencli::http_status))]
    HttpStatus {
        method: String,
        url: String,
        status: u16,
        reason: String,
    },

    #[error("request to {url} timed out")]
    #[diagnostic(
        code(opencli::timeout),
        help("Retry, or allow more time with --timeout <SECONDS>.")
    )]
    Timeout { url: String },

    #[error(transparent)]
    #[diagnostic(code(opencli::network))]
    Network(SendError),

    #[error("`{0}` is not a usable server URL")]
    #[diagnostic(
        code(opencli::server),
        help("Pass an absolute http:// or https:// URL, e.g. --server https://api.example.com.")
    )]
    InvalidServer(String),

    #[error("the spec's server `{0}` is relative and the spec was imported from a file")]
    #[diagnostic(
        code(opencli::server),
        help("Pass the API's base URL with --server <URL>.")
    )]
    RelativeServer(String),

    #[error("no command `{0}`")]
    #[diagnostic(code(opencli::unknown_command))]
    UnknownCommand(String),

    #[error("no specification given")]
    #[diagnostic(
        code(opencli::missing_spec),
        help("Pass --spec <file-or-url>; prompts only appear in an interactive terminal.")
    )]
    MissingSpec,

    #[error("removing `{0}` needs confirmation")]
    #[diagnostic(
        code(opencli::confirmation_required),
        help("Pass --yes to confirm when not running in an interactive terminal.")
    )]
    ConfirmationRequired(String),

    #[error("could not determine the OpenCLI data directory")]
    #[diagnostic(
        code(opencli::data_directory),
        help("Set OPENCLI_HOME to a writable directory.")
    )]
    NoDataDirectory,

    #[error("could not create the OpenCLI data directory {path}")]
    #[diagnostic(code(opencli::data_directory))]
    CreateDataDirectory {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not read from or write to the terminal")]
    #[diagnostic(code(opencli::terminal))]
    Terminal(#[source] std::io::Error),

    #[error("could not encode output as JSON")]
    #[diagnostic(code(opencli::output))]
    Output(#[from] serde_json::Error),
}

/// Exit codes are part of the CLI contract for scripts and agents (ADR 0002).
/// Usage errors exit with 2, set by clap.
pub const EXIT_FAILURE: u8 = 1;
pub const EXIT_HTTP_CLIENT_ERROR: u8 = 3;
pub const EXIT_HTTP_SERVER_ERROR: u8 = 4;
pub const EXIT_NETWORK_ERROR: u8 = 5;
const SERVER_ERROR_STATUSES: u16 = 500;

impl CliError {
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::HttpStatus { status, .. } if *status >= SERVER_ERROR_STATUSES => {
                EXIT_HTTP_SERVER_ERROR
            }
            Self::HttpStatus { .. } => EXIT_HTTP_CLIENT_ERROR,
            Self::Timeout { .. } | Self::Network(_) => EXIT_NETWORK_ERROR,
            _ => EXIT_FAILURE,
        }
    }
}

impl From<SendError> for CliError {
    fn from(error: SendError) -> Self {
        match error {
            SendError::Timeout { url } => Self::Timeout { url },
            SendError::InvalidUrl { url, .. } => Self::InvalidServer(url),
            other => Self::Network(other),
        }
    }
}

impl From<NamingError> for CliError {
    fn from(error: NamingError) -> Self {
        let conflicts: Vec<String> = error
            .conflicts
            .iter()
            .map(|conflict| format!("`{}`: {}", conflict.command, conflict.operations.join(", ")))
            .collect();
        Self::Naming {
            details: format!(
                "{}\nAdd `x-cli-name` (and optionally `x-cli-resource`) to these operations to name them explicitly.",
                conflicts.join("\n")
            ),
        }
    }
}

impl From<StoreError> for CliError {
    fn from(error: StoreError) -> Self {
        Self::Store(StoreFailure(error))
    }
}

/// Adds next-step help to store errors without making the store depend on miette.
#[derive(Debug, Error)]
#[error(transparent)]
pub struct StoreFailure(StoreError);

impl Diagnostic for StoreFailure {
    fn code<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        let code = match &self.0 {
            StoreError::AlreadyExists(_) => "opencli::store::already_exists",
            StoreError::NotFound(_) => "opencli::store::not_found",
            _ => "opencli::store::database",
        };
        Some(Box::new(code))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn Display + 'a>> {
        match &self.0 {
            StoreError::AlreadyExists(name) => Some(Box::new(format!(
                "Choose another name, or run `opencli api remove {name}` first."
            ))),
            StoreError::NotFound(_) => {
                Some(Box::new("Run `opencli api list` to see registered APIs."))
            }
            _ => None,
        }
    }
}
