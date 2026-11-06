//! Chooses the base URL requests go to.
//!
//! Order: `--server` on the call, then the base URL stored for the API (asked
//! for at onboarding, or set with `api set-server`), then the spec's first
//! server. A relative spec server (or none, which OpenAPI defines as `/`)
//! resolves against the spec's source URL; a spec imported from a file has
//! nothing to resolve against, so it needs a stored base URL or `--server`.

use opencli_runtime::is_loopback;
use url::Url;

use crate::error::CliError;

const DEFAULT_SERVER: &str = "/";

pub fn resolve(
    override_url: Option<&str>,
    stored: Option<&str>,
    servers: &[String],
    source: &str,
) -> Result<String, CliError> {
    if let Some(raw) = override_url.or(stored) {
        return normalize(raw);
    }
    let url = from_spec(
        servers.first().map_or(DEFAULT_SERVER, String::as_str),
        source,
    )?;
    checked(&url)
}

/// Validates a base URL typed by the user: absolute `http(s)`, no trailing slash.
pub fn normalize(raw: &str) -> Result<String, CliError> {
    let url = Url::parse(raw.trim()).map_err(|_| CliError::InvalidServer(raw.to_owned()))?;
    checked(&url)
}

fn checked(url: &Url) -> Result<String, CliError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(CliError::InvalidServer(url.to_string()));
    }
    if url.scheme() == "http" && !is_loopback(url) {
        tracing::warn!(server = %url, "sending over plain HTTP; traffic is not encrypted");
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// The spec's servers as usable absolute URLs, first one first, for the
/// onboarding prompt. Relative servers that can't be resolved are left out.
pub fn candidates(servers: &[String], source: &str) -> Vec<String> {
    let declared: Vec<&str> = if servers.is_empty() {
        vec![DEFAULT_SERVER]
    } else {
        servers.iter().map(String::as_str).collect()
    };
    let mut found: Vec<String> = Vec::new();
    for server in declared {
        if let Ok(url) = from_spec(server, source).and_then(|url| checked(&url))
            && !found.contains(&url)
        {
            found.push(url);
        }
    }
    found
}

/// What an onboarding answer means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseUrlChoice {
    /// Keep following the spec's first server (nothing is stored).
    FollowSpec,
    Store(String),
}

/// Enter accepts the spec's first server; a number picks a listed server;
/// anything else must be a URL.
pub fn interpret_answer(answer: &str, candidates: &[String]) -> Result<BaseUrlChoice, String> {
    let answer = answer.trim();
    if answer.is_empty() {
        return if candidates.is_empty() {
            Err("the spec has no usable server, so a base URL is required".to_owned())
        } else {
            Ok(BaseUrlChoice::FollowSpec)
        };
    }
    if let Ok(number) = answer.parse::<usize>() {
        return match number {
            1 if !candidates.is_empty() => Ok(BaseUrlChoice::FollowSpec),
            n if (2..=candidates.len()).contains(&n) => {
                Ok(BaseUrlChoice::Store(candidates[n - 1].clone()))
            }
            _ => Err(format!(
                "enter a number from 1 to {}, or a URL",
                candidates.len().max(1)
            )),
        };
    }
    normalize(answer)
        .map(BaseUrlChoice::Store)
        .map_err(|_| format!("`{answer}` is not an absolute http:// or https:// URL"))
}

fn from_spec(server: &str, source: &str) -> Result<Url, CliError> {
    match Url::parse(server) {
        Ok(url) => Ok(url),
        Err(url::ParseError::RelativeUrlWithoutBase) => Url::parse(source)
            .ok()
            .filter(|base| matches!(base.scheme(), "http" | "https"))
            .and_then(|base| base.join(server).ok())
            .ok_or_else(|| CliError::RelativeServer(server.to_owned())),
        Err(_) => Err(CliError::InvalidServer(server.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const FILE_SOURCE: &str = "/home/me/specs/crm.yaml";
    const URL_SOURCE: &str = "https://api.example.com/docs/openapi.json";

    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|&value| value.to_owned()).collect()
    }

    #[rstest]
    #[case(Some("https://override.example.com/"), Some("https://stored.example.com"), &["https://spec.example.com"], FILE_SOURCE, "https://override.example.com")]
    #[case(None, Some("https://stored.example.com/v2/"), &["https://spec.example.com"], FILE_SOURCE, "https://stored.example.com/v2")]
    #[case(None, None, &["https://prod.example.com/v1"], FILE_SOURCE, "https://prod.example.com/v1")]
    #[case(None, None, &["/v1"], URL_SOURCE, "https://api.example.com/v1")]
    #[case(None, None, &["v1"], URL_SOURCE, "https://api.example.com/docs/v1")]
    #[case(None, None, &[], URL_SOURCE, "https://api.example.com")]
    #[case(None, None, &["http://localhost:8080"], FILE_SOURCE, "http://localhost:8080")]
    fn resolve_prefers_call_then_stored_then_spec(
        #[case] override_url: Option<&str>,
        #[case] stored: Option<&str>,
        #[case] servers: &[&str],
        #[case] source: &str,
        #[case] expected: &str,
    ) {
        assert_eq!(
            resolve(override_url, stored, &owned(servers), source).unwrap(),
            expected
        );
    }

    #[rstest]
    #[case(&["/v1"])]
    #[case(&[])]
    fn resolve_requires_a_base_url_for_relative_servers_from_files(#[case] servers: &[&str]) {
        assert!(matches!(
            resolve(None, None, &owned(servers), FILE_SOURCE),
            Err(CliError::RelativeServer(_))
        ));
    }

    #[rstest]
    #[case("not a url")]
    #[case("ftp://files.example.com")]
    fn normalize_rejects_unusable_urls(#[case] raw: &str) {
        assert!(matches!(normalize(raw), Err(CliError::InvalidServer(_))));
    }

    #[test]
    fn candidates_lists_usable_spec_servers_once() {
        let servers = owned(&[
            "https://prod.example.com/",
            "https://prod.example.com",
            "/v1",
            "https://staging.example.com",
        ]);
        assert_eq!(
            candidates(&servers, FILE_SOURCE),
            owned(&["https://prod.example.com", "https://staging.example.com"])
        );
        assert_eq!(
            candidates(&owned(&["/v1"]), URL_SOURCE),
            owned(&["https://api.example.com/v1"])
        );
        assert_eq!(candidates(&[], FILE_SOURCE), Vec::<String>::new());
    }

    #[rstest]
    #[case("", &["https://a", "https://b"], Ok(BaseUrlChoice::FollowSpec))]
    #[case("1", &["https://a", "https://b"], Ok(BaseUrlChoice::FollowSpec))]
    #[case("2", &["https://a", "https://b"], Ok(BaseUrlChoice::Store("https://b".into())))]
    #[case(" https://custom.example.com/ ", &["https://a"], Ok(BaseUrlChoice::Store("https://custom.example.com".into())))]
    #[case("https://custom.example.com", &[], Ok(BaseUrlChoice::Store("https://custom.example.com".into())))]
    fn interpret_answer_accepts_enter_numbers_and_urls(
        #[case] answer: &str,
        #[case] listed: &[&str],
        #[case] expected: Result<BaseUrlChoice, String>,
    ) {
        assert_eq!(interpret_answer(answer, &owned(listed)), expected);
    }

    #[rstest]
    #[case("", &[])]
    #[case("3", &["https://a", "https://b"])]
    #[case("0", &["https://a"])]
    #[case("crm.example.com", &["https://a"])]
    fn interpret_answer_rejects_unusable_answers(#[case] answer: &str, #[case] listed: &[&str]) {
        assert!(interpret_answer(answer, &owned(listed)).is_err());
    }
}
