use miette::{Diagnostic, NamedSource, SourceSpan};
use thiserror::Error;

#[derive(Debug, Error, Diagnostic)]
pub enum SpecError {
    #[error(transparent)]
    #[diagnostic(transparent)]
    Syntax(Box<SyntaxError>),

    #[error("{source_name} is not an OpenAPI or Swagger document")]
    #[diagnostic(
        code(opencli::spec::unknown_format),
        help("The document needs a top-level `openapi: 3.0.x` field.")
    )]
    UnknownFormat { source_name: String },

    #[error("{source_name} uses {version}, which is not supported yet")]
    #[diagnostic(
        code(opencli::spec::unsupported_version),
        help("OpenCLI currently supports OpenAPI 3.0.x documents.")
    )]
    UnsupportedVersion {
        source_name: String,
        version: String,
    },

    #[error("invalid OpenAPI document at `{path}`: {message}")]
    #[diagnostic(code(opencli::spec::structure))]
    Structure { path: String, message: String },

    #[error("{source_name} failed validation with {} issue(s)", issues.len())]
    #[diagnostic(code(opencli::spec::invalid))]
    Invalid {
        source_name: String,
        #[related]
        issues: Vec<ValidationIssue>,
    },
}

/// One semantic problem, located by operation or document section.
#[derive(Debug, Clone, PartialEq, Eq, Error, Diagnostic)]
#[error("{location}: {message}")]
pub struct ValidationIssue {
    pub location: String,
    pub message: String,
}

impl ValidationIssue {
    pub(crate) fn new(location: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            location: location.into(),
            message: message.into(),
        }
    }
}

/// Boxed inside [`SpecError`] because it carries the whole document for display.
#[derive(Debug, Error, Diagnostic)]
#[error("{source_name} is not valid JSON or YAML")]
#[diagnostic(code(opencli::spec::syntax))]
pub struct SyntaxError {
    pub source_name: String,
    #[source_code]
    pub document: NamedSource<String>,
    #[label("{message}")]
    pub span: SourceSpan,
    pub message: String,
}
