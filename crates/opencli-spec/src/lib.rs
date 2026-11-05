//! Parses OpenAPI documents into the OpenCLI domain model and validates them.
//!
//! Parser crates stay inside this crate (ADR 0001): callers only see
//! [`opencli_core::ApiSpec`] and [`SpecError`].

mod document;
mod error;
mod openapi30;
mod validate;

pub use error::{SpecError, SyntaxError, ValidationIssue};
use opencli_core::ApiSpec;

/// Parses `text` (JSON or YAML) and validates it. `source_name` labels diagnostics.
///
/// # Errors
/// Returns [`SpecError`] when the text isn't JSON/YAML, isn't a supported
/// specification version, doesn't match the OpenAPI structure, or fails
/// semantic validation (all validation issues are reported together).
pub fn parse(text: &str, source_name: &str) -> Result<ApiSpec, SpecError> {
    let document = document::load(text, source_name)?;
    let (spec, mut issues) = match document::detect_format(&document, source_name)? {
        document::DetectedFormat::OpenApi30 => openapi30::convert(document)?,
    };
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
