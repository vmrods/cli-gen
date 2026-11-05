//! Parses OpenAPI documents into the OpenCLI domain model and validates them.
//!
//! OpenAPI 3.1 and Swagger 2.0 are first rewritten to 3.0 form, so one
//! adapter builds the model for every format. Parser crates stay inside this
//! crate (ADR 0001): callers only see
//! [`opencli_core::ApiSpec`] and [`SpecError`].

mod document;
mod error;
mod openapi30;
mod openapi31;
mod swagger20;
mod validate;

use document::DetectedFormat;
pub use error::{SpecError, SyntaxError, ValidationIssue};
use opencli_core::{ApiSpec, SpecFormat};

/// Parses `text` (JSON or YAML) and validates it. `source_name` labels diagnostics.
///
/// # Errors
/// Returns [`SpecError`] when the text isn't JSON/YAML, isn't a supported
/// specification version, doesn't match the OpenAPI structure, or fails
/// semantic validation (all validation issues are reported together).
pub fn parse(text: &str, source_name: &str) -> Result<ApiSpec, SpecError> {
    let document = document::load(text, source_name)?;
    let mut issues = Vec::new();
    let (format, document) = match document::detect_format(&document, source_name)? {
        DetectedFormat::OpenApi30 => (SpecFormat::OpenApi30, document),
        DetectedFormat::OpenApi31 => (SpecFormat::OpenApi31, openapi31::to_openapi30(document)),
        DetectedFormat::Swagger20 => (
            SpecFormat::Swagger20,
            swagger20::to_openapi30(&document, &mut issues),
        ),
    };
    let (spec, adapter_issues) = openapi30::convert(document, format)?;
    issues.extend(adapter_issues);
    issues.extend(validate::validate(&spec));
    if issues.is_empty() {
        Ok(spec)
    } else {
        Err(SpecError::Invalid {
            source_name: source_name.to_owned(),
            issues,
        })
    }
}
