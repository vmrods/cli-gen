//! Loads specification text from a local file or a remote URL.

use std::path::PathBuf;
use std::time::Duration;

use miette::Diagnostic;
use opencli_runtime::is_loopback;
use thiserror::Error;
use url::Url;

/// Larger documents are almost certainly not specs, and would bloat the store.
const MAX_SPEC_BYTES: u64 = 20 * 1024 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_REDIRECTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecSource {
    File(PathBuf),
    Url(Url),
}

/// A loaded document plus how to refer to it later (for refresh).
#[derive(Debug)]
pub struct LoadedSpec {
    pub text: String,
    /// Absolute path or URL, stored as the API's source.
    pub location: String,
    /// Short label for diagnostics.
    pub display_name: String,
}

#[derive(Debug, Error, Diagnostic)]
pub enum SourceError {
    #[error("`{0}` is not a valid URL")]
    #[diagnostic(code(opencli::source::invalid_url))]
    InvalidUrl(String, #[source] url::ParseError),

    #[error("refusing to fetch {0} over plain HTTP")]
    #[diagnostic(
        code(opencli::source::insecure_url),
        help("Use https://. Plain http:// is only allowed for localhost.")
    )]
    InsecureUrl(Url),

    #[error("could not read {path}")]
    #[diagnostic(code(opencli::source::read))]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("could not fetch {url}")]
    #[diagnostic(code(opencli::source::fetch))]
    Fetch {
        url: Url,
        #[source]
        source: reqwest::Error,
    },

    #[error("{url} returned HTTP {status}")]
    #[diagnostic(code(opencli::source::status))]
    Status { url: Url, status: u16 },

    #[error("{0} is larger than the {limit} MiB limit", limit = MAX_SPEC_BYTES / 1024 / 1024)]
    #[diagnostic(code(opencli::source::too_large))]
    TooLarge(String),

    #[error("{0} is not valid UTF-8 text")]
    #[diagnostic(code(opencli::source::encoding))]
    NotUtf8(String),
}

impl SpecSource {
    /// Treats `http://` and `https://` values as URLs and anything else as a path.
    pub fn parse(raw: &str) -> Result<Self, SourceError> {
        let lowered = raw.to_ascii_lowercase();
        if !(lowered.starts_with("http://") || lowered.starts_with("https://")) {
            return Ok(Self::File(PathBuf::from(raw)));
        }
        let url =
            Url::parse(raw).map_err(|error| SourceError::InvalidUrl(raw.to_owned(), error))?;
        if url.scheme() == "http" && !is_loopback(&url) {
            return Err(SourceError::InsecureUrl(url));
        }
        Ok(Self::Url(url))
    }

    pub async fn load(&self) -> Result<LoadedSpec, SourceError> {
        match self {
            Self::File(path) => load_file(path).await,
            Self::Url(url) => load_url(url).await,
        }
    }
}

async fn load_file(path: &PathBuf) -> Result<LoadedSpec, SourceError> {
    let read_error = |source| SourceError::Read {
        path: path.display().to_string(),
        source,
    };
    let absolute = tokio::fs::canonicalize(path).await.map_err(read_error)?;
    let metadata = tokio::fs::metadata(&absolute).await.map_err(read_error)?;
    if metadata.len() > MAX_SPEC_BYTES {
        return Err(SourceError::TooLarge(path.display().to_string()));
    }
    let bytes = tokio::fs::read(&absolute).await.map_err(read_error)?;
    let text =
        String::from_utf8(bytes).map_err(|_| SourceError::NotUtf8(path.display().to_string()))?;
    Ok(LoadedSpec {
        text,
        location: absolute.display().to_string(),
        display_name: file_label(path),
    })
}

fn file_label(path: &std::path::Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

async fn load_url(url: &Url) -> Result<LoadedSpec, SourceError> {
    let fetch_error = |source| SourceError::Fetch {
        url: url.clone(),
        source,
    };
    let client = reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
        // An https:// source must never be downgraded by a redirect.
        .https_only(url.scheme() == "https")
        .build()
        .map_err(fetch_error)?;
    let mut response = client.get(url.clone()).send().await.map_err(fetch_error)?;
    let status = response.status();
    if !status.is_success() {
        return Err(SourceError::Status {
            url: url.clone(),
            status: status.as_u16(),
        });
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SPEC_BYTES)
    {
        return Err(SourceError::TooLarge(url.to_string()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(fetch_error)? {
        bytes.extend_from_slice(&chunk);
        if bytes.len() as u64 > MAX_SPEC_BYTES {
            return Err(SourceError::TooLarge(url.to_string()));
        }
    }
    let text = String::from_utf8(bytes).map_err(|_| SourceError::NotUtf8(url.to_string()))?;
    Ok(LoadedSpec {
        text,
        location: url.to_string(),
        display_name: url.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_treats_plain_values_as_paths() {
        assert_eq!(
            SpecSource::parse("specs/crm.yaml").unwrap(),
            SpecSource::File("specs/crm.yaml".into())
        );
    }

    #[test]
    fn parse_accepts_https_urls() {
        assert!(matches!(
            SpecSource::parse("https://api.example.com/openapi.json"),
            Ok(SpecSource::Url(_))
        ));
    }

    #[test]
    fn parse_accepts_http_only_for_loopback() {
        for raw in [
            "http://localhost:8080/spec.json",
            "http://127.0.0.1/spec.json",
            "http://[::1]/spec.json",
        ] {
            assert!(
                matches!(SpecSource::parse(raw), Ok(SpecSource::Url(_))),
                "{raw}"
            );
        }
        assert!(matches!(
            SpecSource::parse("http://api.example.com/spec.json"),
            Err(SourceError::InsecureUrl(_))
        ));
    }

    #[test]
    fn parse_rejects_malformed_urls() {
        assert!(matches!(
            SpecSource::parse("https://"),
            Err(SourceError::InvalidUrl(..))
        ));
    }
}
