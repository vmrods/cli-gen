use thiserror::Error;

use crate::CredentialStoreError;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("`{api}` needs credentials for {}", schemes.join(" or "))]
    NotLoggedIn { api: String, schemes: Vec<String> },
    #[error("the stored token for `{api}` ({scheme}) has expired and cannot be refreshed")]
    Expired { api: String, scheme: String },
    #[error("security scheme `{scheme}` is not supported: {description}")]
    UnsupportedScheme { scheme: String, description: String },
    #[error("environment variable {variable} must be `username:password` for basic authentication")]
    InvalidEnvironmentCredential { variable: String },
    #[error("`{0}` is not an HTTPS URL; credentials are only sent over HTTPS (or to localhost)")]
    InsecureEndpoint(String),
    #[error(transparent)]
    Store(#[from] CredentialStoreError),
    #[error("could not create the HTTP client for token requests")]
    TokenClient(#[source] reqwest::Error),
    #[error("token request to {url} failed")]
    TokenRequest {
        url: String,
        #[source]
        source: reqwest::Error,
    },
    #[error("token endpoint {url} returned HTTP {status}{}", error.as_deref().map_or_default(|error| format!(": {error}")))]
    TokenEndpoint {
        url: String,
        status: u16,
        error: Option<String>,
        description: Option<String>,
    },
    #[error("unexpected response from token endpoint {url}: {message}")]
    TokenResponse { url: String, message: String },
    #[error("authorization was denied: {error}")]
    AuthorizationDenied {
        error: String,
        description: Option<String>,
    },
    #[error("the authorization redirect did not match this login attempt")]
    StateMismatch,
    #[error("no authorization redirect arrived in time")]
    CallbackTimeout,
    #[error("the local redirect listener failed")]
    Callback(#[source] std::io::Error),
    #[error("no secure random source: {0}")]
    Random(String),
}
