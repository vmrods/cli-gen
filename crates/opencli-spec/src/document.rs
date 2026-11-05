use miette::NamedSource;
use serde_json::Value;

use crate::{SpecError, SyntaxError};

pub(crate) enum DetectedFormat {
    OpenApi30,
}

/// Parses JSON or YAML into a generic document, keeping syntax error positions.
pub(crate) fn load(text: &str, source_name: &str) -> Result<Value, SpecError> {
    if text.trim_start().starts_with('{') {
        serde_json::from_str(text).map_err(|error| {
            let offset = offset_of(text, error.line(), error.column());
            syntax_error(text, source_name, offset, error.to_string())
        })
    } else {
        serde_yaml_ng::from_str(text).map_err(|error| {
            let offset = error.location().map_or(0, |location| location.index());
            syntax_error(text, source_name, offset, error.to_string())
        })
    }
}

pub(crate) fn detect_format(
    document: &Value,
    source_name: &str,
) -> Result<DetectedFormat, SpecError> {
    if let Some(version) = document.get("openapi").and_then(Value::as_str) {
        if version.starts_with("3.0.") {
            return Ok(DetectedFormat::OpenApi30);
        }
        return Err(SpecError::UnsupportedVersion {
            source_name: source_name.to_owned(),
            version: format!("OpenAPI {version}"),
        });
    }
    if let Some(version) = document.get("swagger").and_then(Value::as_str) {
        return Err(SpecError::UnsupportedVersion {
            source_name: source_name.to_owned(),
            version: format!("Swagger {version}"),
        });
    }
    Err(SpecError::UnknownFormat {
        source_name: source_name.to_owned(),
    })
}

/// Converts a 1-based line/column into a byte offset, clamped to the text.
fn offset_of(text: &str, line: usize, column: usize) -> usize {
    let line_start: usize = text
        .split_inclusive('\n')
        .take(line.saturating_sub(1))
        .map(str::len)
        .sum();
    (line_start + column.saturating_sub(1)).min(text.len())
}

fn syntax_error(text: &str, source_name: &str, offset: usize, message: String) -> SpecError {
    let length = usize::from(offset < text.len());
    SpecError::Syntax(Box::new(SyntaxError {
        source_name: source_name.to_owned(),
        document: NamedSource::new(source_name, text.to_owned()),
        span: (offset, length).into(),
        message,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offset_of_counts_preceding_lines_and_column() {
        let text = "ab\ncde\nf";
        assert_eq!(offset_of(text, 1, 1), 0);
        assert_eq!(offset_of(text, 2, 2), 4);
        assert_eq!(offset_of(text, 3, 1), 7);
    }

    #[test]
    fn offset_of_clamps_past_end() {
        assert_eq!(offset_of("ab", 9, 9), 2);
    }
}
