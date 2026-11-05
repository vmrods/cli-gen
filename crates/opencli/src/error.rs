use std::fmt::Display;

use miette::Diagnostic;
use opencli_core::NamingError;
use opencli_runtime::PlanError;
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

    #[error("sending requests is not implemented yet")]
    #[diagnostic(
        code(opencli::not_implemented),
        help("Use --dry-run to see the request, or --describe for the operation's arguments.")
    )]
    SendingNotImplemented,

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
