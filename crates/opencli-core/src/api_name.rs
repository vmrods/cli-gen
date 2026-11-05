use std::fmt;
use std::str::FromStr;

use serde::Serialize;
use thiserror::Error;

/// Top-level command names an imported API may not use, because they would
/// shadow built-in commands. Part of the public CLI contract (ADR 0002).
pub const RESERVED_NAMES: &[&str] = &["api", "auth", "plugin", "completion", "help", "config"];

const MAX_LENGTH: usize = 64;

/// The user-chosen name an API is registered under; also its top-level command.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ApiName(String);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ApiNameError {
    #[error("API name must not be empty")]
    Empty,
    #[error("API name must be at most {MAX_LENGTH} characters")]
    TooLong,
    #[error(
        "API name `{0}` must be kebab-case: lowercase letters and digits in hyphen-separated words, starting with a letter"
    )]
    NotKebabCase(String),
    #[error("API name `{0}` is reserved for a built-in command")]
    Reserved(String),
}

impl ApiName {
    /// # Errors
    /// Returns an error when `raw` is empty, too long, not kebab-case, or reserved.
    pub fn parse(raw: &str) -> Result<Self, ApiNameError> {
        if raw.is_empty() {
            return Err(ApiNameError::Empty);
        }
        if raw.len() > MAX_LENGTH {
            return Err(ApiNameError::TooLong);
        }
        if !is_kebab_case(raw) {
            return Err(ApiNameError::NotKebabCase(raw.to_owned()));
        }
        if RESERVED_NAMES.contains(&raw) {
            return Err(ApiNameError::Reserved(raw.to_owned()));
        }
        Ok(Self(raw.to_owned()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

fn is_kebab_case(raw: &str) -> bool {
    raw.starts_with(|c: char| c.is_ascii_lowercase())
        && raw.split('-').all(|word| {
            !word.is_empty()
                && word
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

impl fmt::Display for ApiName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ApiName {
    type Err = ApiNameError;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("crm")]
    #[case("crm-api")]
    #[case("petstore-v2")]
    #[case("a1-b2-c3")]
    fn parse_accepts_kebab_case(#[case] raw: &str) {
        assert_eq!(ApiName::parse(raw).unwrap().as_str(), raw);
    }

    #[rstest]
    #[case("", ApiNameError::Empty)]
    #[case("CRM", ApiNameError::NotKebabCase("CRM".into()))]
    #[case("crm_api", ApiNameError::NotKebabCase("crm_api".into()))]
    #[case("-crm", ApiNameError::NotKebabCase("-crm".into()))]
    #[case("crm-", ApiNameError::NotKebabCase("crm-".into()))]
    #[case("crm--api", ApiNameError::NotKebabCase("crm--api".into()))]
    #[case("1crm", ApiNameError::NotKebabCase("1crm".into()))]
    #[case("api", ApiNameError::Reserved("api".into()))]
    #[case("auth", ApiNameError::Reserved("auth".into()))]
    fn parse_rejects_invalid_names(#[case] raw: &str, #[case] expected: ApiNameError) {
        assert_eq!(ApiName::parse(raw), Err(expected));
    }

    #[test]
    fn parse_rejects_names_longer_than_limit() {
        let raw = "a".repeat(MAX_LENGTH + 1);
        assert_eq!(ApiName::parse(&raw), Err(ApiNameError::TooLong));
    }

    #[test]
    fn from_str_matches_parse() {
        let name: ApiName = "crm".parse().unwrap();
        assert_eq!(name.to_string(), "crm");
    }
}
