//! Chooses the base URL requests go to.
//!
//! Order: `--server`, then the spec's first server. A relative server (or none,
//! which OpenAPI defines as `/`) resolves against the spec's source URL; a spec
//! imported from a file has nothing to resolve against, so `--server` is needed.

use opencli_runtime::is_loopback;
use url::Url;

use crate::error::CliError;

const DEFAULT_SERVER: &str = "/";

pub fn resolve(
    override_url: Option<&str>,
    servers: &[String],
    source: &str,
) -> Result<String, CliError> {
    let url = match override_url {
        Some(raw) => Url::parse(raw).map_err(|_| CliError::InvalidServer(raw.to_owned()))?,
        None => from_spec(
            servers.first().map_or(DEFAULT_SERVER, String::as_str),
            source,
        )?,
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Err(CliError::InvalidServer(url.to_string()));
    }
    if url.scheme() == "http" && !is_loopback(&url) {
        tracing::warn!(server = %url, "sending over plain HTTP; traffic is not encrypted");
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
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

    #[rstest]
    #[case(Some("https://staging.example.com/v2/"), &["https://prod.example.com"], FILE_SOURCE, "https://staging.example.com/v2")]
    #[case(None, &["https://prod.example.com/v1"], FILE_SOURCE, "https://prod.example.com/v1")]
    #[case(None, &["/v1"], URL_SOURCE, "https://api.example.com/v1")]
    #[case(None, &["v1"], URL_SOURCE, "https://api.example.com/docs/v1")]
    #[case(None, &[], URL_SOURCE, "https://api.example.com")]
    #[case(None, &["http://localhost:8080"], FILE_SOURCE, "http://localhost:8080")]
    fn resolve_picks_base_url(
        #[case] override_url: Option<&str>,
        #[case] servers: &[&str],
        #[case] source: &str,
        #[case] expected: &str,
    ) {
        let servers: Vec<String> = servers.iter().map(|&server| server.to_owned()).collect();
        assert_eq!(resolve(override_url, &servers, source).unwrap(), expected);
    }

    #[rstest]
    #[case(&["/v1"])]
    #[case(&[])]
    fn resolve_requires_server_for_relative_urls_from_files(#[case] servers: &[&str]) {
        let servers: Vec<String> = servers.iter().map(|&server| server.to_owned()).collect();
        assert!(matches!(
            resolve(None, &servers, FILE_SOURCE),
            Err(CliError::RelativeServer(_))
        ));
    }

    #[rstest]
    #[case("not a url")]
    #[case("ftp://files.example.com")]
    fn resolve_rejects_invalid_overrides(#[case] raw: &str) {
        assert!(matches!(
            resolve(Some(raw), &[], FILE_SOURCE),
            Err(CliError::InvalidServer(_))
        ));
    }
}
