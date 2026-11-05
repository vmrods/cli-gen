//! Sends a [`RequestPlan`]. TLS verification is always on (OWASP API8); every
//! request has connect and total timeouts.

use std::time::{Duration, Instant};

use opencli_core::{HttpMethod, is_json_media_type};
use reqwest::header::{ACCEPT, CONTENT_TYPE, COOKIE};
use serde_json::Value;
use thiserror::Error;
use url::Url;

use crate::RequestPlan;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_REDIRECTS: usize = 5;
/// Protects the terminal and memory from runaway responses.
const MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const USER_AGENT: &str = concat!("opencli/", env!("CARGO_PKG_VERSION"));
const ACCEPT_JSON_FIRST: &str = "application/json, */*;q=0.8";

#[derive(Debug, Clone)]
pub struct HttpClient {
    client: reqwest::Client,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    pub reason: String,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

#[derive(Debug, Error)]
pub enum SendError {
    #[error("`{url}` is not an absolute URL")]
    InvalidUrl {
        url: String,
        #[source]
        source: url::ParseError,
    },
    #[error("request to {url} timed out")]
    Timeout { url: String },
    #[error("could not connect to {url}")]
    Connect {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("request to {url} failed")]
    Request {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("response from {url} is larger than {limit} MiB", limit = MAX_RESPONSE_BYTES / 1024 / 1024)]
    TooLarge { url: String },
    #[error("could not create the HTTP client")]
    Client(#[source] reqwest::Error),
    #[error("could not encode the request body")]
    Encode(#[from] serde_json::Error),
}

impl HttpClient {
    /// # Errors
    /// [`SendError::Client`] when the TLS backend can't be initialized.
    pub fn new(timeout: Duration) -> Result<Self, SendError> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::limited(MAX_REDIRECTS))
            .build()
            .map_err(SendError::Client)?;
        Ok(Self { client })
    }

    /// # Errors
    /// [`SendError`] for invalid URLs, timeouts, connection failures and
    /// oversized responses. HTTP error statuses are not errors here.
    pub async fn send(&self, plan: &RequestPlan) -> Result<Response, SendError> {
        let mut url = Url::parse(&plan.url).map_err(|source| SendError::InvalidUrl {
            url: plan.url.clone(),
            source,
        })?;
        if !plan.query.is_empty() {
            url.query_pairs_mut().extend_pairs(&plan.query);
        }
        let label = without_query(&url);

        let mut request = self
            .client
            .request(method(plan.method), url)
            .header(ACCEPT, ACCEPT_JSON_FIRST);
        for (name, value) in &plan.headers {
            request = request.header(name, value);
        }
        if !plan.cookies.is_empty() {
            let cookies: Vec<String> = plan
                .cookies
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect();
            request = request.header(COOKIE, cookies.join("; "));
        }
        if let Some(body) = &plan.body {
            let bytes = match &body.value {
                Value::String(text) if !is_json_media_type(&body.content_type) => {
                    text.clone().into_bytes()
                }
                value => serde_json::to_vec(value)?,
            };
            request = request.header(CONTENT_TYPE, &body.content_type).body(bytes);
        }

        let started = Instant::now();
        let mut response = request
            .send()
            .await
            .map_err(|error| classify(error, &label))?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err(SendError::TooLarge { url: label });
        }
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|error| classify(error, &label))?
        {
            body.extend_from_slice(&chunk);
            if body.len() > MAX_RESPONSE_BYTES {
                return Err(SendError::TooLarge { url: label });
            }
        }
        // Query strings and headers may carry credentials, so they are never logged.
        tracing::debug!(
            method = %plan.method,
            url = %label,
            status = status.as_u16(),
            elapsed_ms = started.elapsed().as_millis(),
            bytes = body.len(),
            "request completed"
        );
        Ok(Response {
            status: status.as_u16(),
            reason: status.canonical_reason().unwrap_or_default().to_owned(),
            content_type,
            body,
        })
    }
}

impl Response {
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The body as JSON, when it is declared or sniffed as JSON.
    #[must_use]
    pub fn json(&self) -> Option<Value> {
        if self.body.is_empty() {
            return None;
        }
        let declared_json = self.content_type.as_deref().is_some_and(is_json_media_type);
        let undeclared = self.content_type.is_none();
        (declared_json || undeclared)
            .then(|| serde_json::from_slice(&self.body).ok())
            .flatten()
    }

    #[must_use]
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.body).ok()
    }
}

fn method(method: HttpMethod) -> reqwest::Method {
    match method {
        HttpMethod::Get => reqwest::Method::GET,
        HttpMethod::Post => reqwest::Method::POST,
        HttpMethod::Put => reqwest::Method::PUT,
        HttpMethod::Patch => reqwest::Method::PATCH,
        HttpMethod::Delete => reqwest::Method::DELETE,
    }
}

fn without_query(url: &Url) -> String {
    let mut label = url.clone();
    label.set_query(None);
    label.set_fragment(None);
    label.to_string()
}

fn classify(error: reqwest::Error, url: &str) -> SendError {
    let url = url.to_owned();
    if error.is_timeout() {
        SendError::Timeout { url }
    } else if error.is_connect() {
        SendError::Connect { url, source: error }
    } else {
        SendError::Request { url, source: error }
    }
}
