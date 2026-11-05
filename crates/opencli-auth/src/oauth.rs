//! OAuth 2.0 token requests (RFC 6749), PKCE (RFC 7636) and the loopback
//! redirect for native apps (RFC 8252). Implemented over our own HTTP client
//! rather than the `oauth2` crate (ADR 0007).

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use opencli_runtime::is_secure_transport;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use url::Url;

use crate::AuthError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PKCE_VERIFIER_BYTES: usize = 32;
const STATE_BYTES: usize = 16;
const CALLBACK_PATH: &str = "/callback";
const LOOPBACK_ADDRESS: &str = "127.0.0.1";
/// Bounds what a stray local client can make us read.
const MAX_REQUEST_LINE_BYTES: u64 = 8 * 1024;
const BEARER: &str = "bearer";
const CALLBACK_PAGE: &str = "<!doctype html><title>OpenCLI</title><p>Signed in. You can close this window and return to the terminal.</p>";
const CALLBACK_ERROR_PAGE: &str = "<!doctype html><title>OpenCLI</title><p>Sign-in failed. Return to the terminal for details.</p>";

/// The token endpoint's answer, with secrets wrapped.
#[derive(Debug)]
pub struct TokenResponse {
    pub access_token: SecretString,
    pub refresh_token: Option<SecretString>,
    pub expires_in: Option<u64>,
    pub scope: Option<String>,
}

impl TokenResponse {
    #[must_use]
    pub fn expires_at(&self, now: OffsetDateTime) -> Option<OffsetDateTime> {
        self.expires_in
            .and_then(|seconds| i64::try_from(seconds).ok())
            .map(|seconds| now + time::Duration::seconds(seconds))
    }
}

#[derive(Deserialize)]
struct TokenBody {
    access_token: String,
    token_type: Option<String>,
    expires_in: Option<NumberOrString>,
    refresh_token: Option<String>,
    scope: Option<String>,
}

/// Some providers send `expires_in` as a string.
#[derive(Deserialize)]
#[serde(untagged)]
enum NumberOrString {
    Number(u64),
    Text(String),
}

#[derive(Deserialize)]
struct ErrorBody {
    error: Option<String>,
    error_description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TokenClient {
    http: reqwest::Client,
}

impl TokenClient {
    /// # Errors
    /// [`AuthError::TokenClient`] when the TLS backend can't be initialized.
    pub fn new(timeout: Duration) -> Result<Self, AuthError> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("opencli/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(timeout)
            // A token endpoint must answer directly; following redirects would
            // replay client secrets to wherever it points.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(AuthError::TokenClient)?;
        Ok(Self { http })
    }

    /// Client credentials grant, authenticating with `client_secret_post`.
    ///
    /// # Errors
    /// [`AuthError`] for insecure URLs, transport failures and endpoint errors.
    pub async fn client_credentials(
        &self,
        token_url: &str,
        client_id: &str,
        client_secret: &SecretString,
        scopes: &[String],
    ) -> Result<TokenResponse, AuthError> {
        let scope = scopes.join(" ");
        let mut form = vec![
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
            ("client_secret", client_secret.expose_secret()),
        ];
        if !scope.is_empty() {
            form.push(("scope", &scope));
        }
        self.request(token_url, &form).await
    }

    /// Refresh token grant.
    ///
    /// # Errors
    /// [`AuthError`] for insecure URLs, transport failures and endpoint errors.
    pub async fn refresh(
        &self,
        token_url: &str,
        client_id: &str,
        client_secret: Option<&SecretString>,
        refresh_token: &SecretString,
    ) -> Result<TokenResponse, AuthError> {
        let mut form = vec![
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token.expose_secret()),
            ("client_id", client_id),
        ];
        if let Some(secret) = client_secret {
            form.push(("client_secret", secret.expose_secret()));
        }
        self.request(token_url, &form).await
    }

    /// Authorization code grant with the PKCE verifier.
    ///
    /// # Errors
    /// [`AuthError`] for insecure URLs, transport failures and endpoint errors.
    pub async fn exchange_code(
        &self,
        token_url: &str,
        client_id: &str,
        client_secret: Option<&SecretString>,
        code: &str,
        pkce: &Pkce,
        redirect_uri: &str,
    ) -> Result<TokenResponse, AuthError> {
        let mut form = vec![
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("client_id", client_id),
            ("code_verifier", pkce.verifier.expose_secret()),
        ];
        if let Some(secret) = client_secret {
            form.push(("client_secret", secret.expose_secret()));
        }
        self.request(token_url, &form).await
    }

    async fn request(
        &self,
        token_url: &str,
        form: &[(&str, &str)],
    ) -> Result<TokenResponse, AuthError> {
        let url = secure_url(token_url)?;
        let response = self
            .http
            .post(url)
            .form(form)
            .send()
            .await
            .map_err(|source| AuthError::TokenRequest {
                url: token_url.to_owned(),
                source,
            })?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|source| AuthError::TokenRequest {
                url: token_url.to_owned(),
                source,
            })?;
        if !status.is_success() {
            let body: Option<ErrorBody> = serde_json::from_slice(&bytes).ok();
            return Err(AuthError::TokenEndpoint {
                url: token_url.to_owned(),
                status: status.as_u16(),
                error: body.as_ref().and_then(|body| body.error.clone()),
                description: body.and_then(|body| body.error_description),
            });
        }
        let invalid = |message: &str| AuthError::TokenResponse {
            url: token_url.to_owned(),
            message: message.to_owned(),
        };
        let body: TokenBody =
            serde_json::from_slice(&bytes).map_err(|_| invalid("not a JSON token response"))?;
        if body
            .token_type
            .as_deref()
            .is_some_and(|kind| !kind.eq_ignore_ascii_case(BEARER))
        {
            return Err(invalid("only bearer tokens are supported"));
        }
        let expires_in = match body.expires_in {
            None => None,
            Some(NumberOrString::Number(seconds)) => Some(seconds),
            Some(NumberOrString::Text(text)) => Some(
                text.parse()
                    .map_err(|_| invalid("expires_in is not a number"))?,
            ),
        };
        Ok(TokenResponse {
            access_token: SecretString::from(body.access_token),
            refresh_token: body.refresh_token.map(SecretString::from),
            expires_in,
            scope: body.scope,
        })
    }
}

/// Token and authorization endpoints must use HTTPS (localhost excepted).
fn secure_url(raw: &str) -> Result<Url, AuthError> {
    Url::parse(raw)
        .ok()
        .filter(is_secure_transport)
        .ok_or_else(|| AuthError::InsecureEndpoint(raw.to_owned()))
}

/// PKCE S256 pair. The verifier never leaves this process except to the token endpoint.
#[derive(Debug)]
pub struct Pkce {
    verifier: SecretString,
    pub challenge: String,
}

impl Pkce {
    /// # Errors
    /// [`AuthError::Random`] when the OS has no secure random source.
    pub fn generate() -> Result<Self, AuthError> {
        Ok(Self::from_verifier(
            URL_SAFE_NO_PAD.encode(random_bytes::<PKCE_VERIFIER_BYTES>()?),
        ))
    }

    #[must_use]
    pub fn from_verifier(verifier: String) -> Self {
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier: SecretString::from(verifier),
            challenge,
        }
    }
}

/// Random value tying the browser redirect to this login attempt.
///
/// # Errors
/// [`AuthError::Random`] when the OS has no secure random source.
pub fn new_state() -> Result<String, AuthError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes::<STATE_BYTES>()?))
}

/// The URL to open in the browser for the authorization code flow.
///
/// # Errors
/// [`AuthError::InsecureEndpoint`] for non-HTTPS authorization URLs.
pub fn authorization_url(
    authorization_url: &str,
    client_id: &str,
    redirect_uri: &str,
    scopes: &[String],
    state: &str,
    pkce: &Pkce,
) -> Result<Url, AuthError> {
    let mut url = secure_url(authorization_url)?;
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("state", state)
        .append_pair("code_challenge", &pkce.challenge)
        .append_pair("code_challenge_method", "S256");
    if !scopes.is_empty() {
        url.query_pairs_mut()
            .append_pair("scope", &scopes.join(" "));
    }
    Ok(url)
}

/// Receives the authorization redirect on `127.0.0.1` (RFC 8252 §7.3).
#[derive(Debug)]
pub struct CallbackListener {
    listener: TcpListener,
    port: u16,
}

impl CallbackListener {
    /// Port 0 picks a free port; pass a fixed one when the provider requires
    /// an exact registered redirect URI.
    ///
    /// # Errors
    /// [`AuthError::Callback`] when the port can't be bound.
    pub async fn bind(port: u16) -> Result<Self, AuthError> {
        let listener = TcpListener::bind((LOOPBACK_ADDRESS, port))
            .await
            .map_err(AuthError::Callback)?;
        let port = listener.local_addr().map_err(AuthError::Callback)?.port();
        Ok(Self { listener, port })
    }

    #[must_use]
    pub fn redirect_uri(&self) -> String {
        format!("http://{LOOPBACK_ADDRESS}:{}{CALLBACK_PATH}", self.port)
    }

    /// Waits for the redirect and returns the authorization code.
    ///
    /// # Errors
    /// [`AuthError`] when the provider reports an error, the state doesn't
    /// match, or nothing arrives within `timeout`.
    pub async fn wait_for_code(
        self,
        expected_state: &str,
        timeout: Duration,
    ) -> Result<String, AuthError> {
        tokio::time::timeout(timeout, self.accept_until_callback(expected_state))
            .await
            .map_err(|_| AuthError::CallbackTimeout)?
    }

    async fn accept_until_callback(&self, expected_state: &str) -> Result<String, AuthError> {
        loop {
            let (stream, _) = self.listener.accept().await.map_err(AuthError::Callback)?;
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            (&mut reader)
                .take(MAX_REQUEST_LINE_BYTES)
                .read_line(&mut request_line)
                .await
                .map_err(AuthError::Callback)?;
            let mut stream = reader.into_inner();
            let Some(parameters) = callback_parameters(&request_line) else {
                // Browsers also ask for /favicon.ico and the like; keep waiting.
                respond(&mut stream, "404 Not Found", "").await;
                continue;
            };
            let outcome = callback_outcome(&parameters, expected_state);
            let page = if outcome.is_ok() {
                CALLBACK_PAGE
            } else {
                CALLBACK_ERROR_PAGE
            };
            respond(&mut stream, "200 OK", page).await;
            return outcome;
        }
    }
}

fn callback_parameters(request_line: &str) -> Option<HashMap<String, String>> {
    let mut parts = request_line.split_whitespace();
    let (method, target) = (parts.next()?, parts.next()?);
    if method != "GET" {
        return None;
    }
    let url = Url::parse(&format!("http://{LOOPBACK_ADDRESS}{target}")).ok()?;
    (url.path() == CALLBACK_PATH).then(|| url.query_pairs().into_owned().collect())
}

fn callback_outcome(
    parameters: &HashMap<String, String>,
    expected_state: &str,
) -> Result<String, AuthError> {
    if let Some(error) = parameters.get("error") {
        return Err(AuthError::AuthorizationDenied {
            error: error.clone(),
            description: parameters.get("error_description").cloned(),
        });
    }
    if parameters.get("state").map(String::as_str) != Some(expected_state) {
        return Err(AuthError::StateMismatch);
    }
    parameters
        .get("code")
        .cloned()
        .ok_or(AuthError::StateMismatch)
}

async fn respond(stream: &mut tokio::net::TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    // The browser page is a courtesy; the code has already been received.
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn random_bytes<const N: usize>() -> Result<[u8; N], AuthError> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|error| AuthError::Random(error.to_string()))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc_7636_example() {
        let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(
            pkce.challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn generated_pkce_and_state_are_unique_and_url_safe() {
        let (first, second) = (Pkce::generate().unwrap(), Pkce::generate().unwrap());
        assert_ne!(first.challenge, second.challenge);
        assert_eq!(first.verifier.expose_secret().len(), 43);
        let state = new_state().unwrap();
        assert!(
            state
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "{state}"
        );
    }

    #[test]
    fn authorization_url_carries_pkce_state_and_scopes() {
        let pkce = Pkce::from_verifier("verifier".into());
        let url = authorization_url(
            "https://login.example.com/authorize?tenant=x",
            "app",
            "http://127.0.0.1:8400/callback",
            &["read".into(), "write".into()],
            "st4te",
            &pkce,
        )
        .unwrap();
        let query: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(query["tenant"], "x");
        assert_eq!(query["response_type"], "code");
        assert_eq!(query["client_id"], "app");
        assert_eq!(query["redirect_uri"], "http://127.0.0.1:8400/callback");
        assert_eq!(query["state"], "st4te");
        assert_eq!(query["code_challenge"], pkce.challenge);
        assert_eq!(query["code_challenge_method"], "S256");
        assert_eq!(query["scope"], "read write");
    }

    #[test]
    fn authorization_url_requires_https() {
        let pkce = Pkce::from_verifier("v".into());
        let error = authorization_url(
            "http://login.example.com/authorize",
            "app",
            "r",
            &[],
            "s",
            &pkce,
        )
        .unwrap_err();
        assert!(matches!(error, AuthError::InsecureEndpoint(_)));
    }

    #[test]
    fn callback_parameters_accept_only_get_callback() {
        assert!(callback_parameters("GET /callback?code=c&state=s HTTP/1.1\r\n").is_some());
        assert!(callback_parameters("GET /favicon.ico HTTP/1.1\r\n").is_none());
        assert!(callback_parameters("POST /callback HTTP/1.1\r\n").is_none());
        assert!(callback_parameters("").is_none());
    }

    #[test]
    fn callback_outcome_checks_state_and_errors() {
        let parameters = |pairs: &[(&str, &str)]| -> HashMap<String, String> {
            pairs
                .iter()
                .map(|&(k, v)| (k.to_owned(), v.to_owned()))
                .collect()
        };
        assert_eq!(
            callback_outcome(&parameters(&[("code", "c"), ("state", "s")]), "s").unwrap(),
            "c"
        );
        assert!(matches!(
            callback_outcome(&parameters(&[("code", "c"), ("state", "x")]), "s"),
            Err(AuthError::StateMismatch)
        ));
        assert!(matches!(
            callback_outcome(&parameters(&[("state", "s")]), "s"),
            Err(AuthError::StateMismatch)
        ));
        assert!(matches!(
            callback_outcome(&parameters(&[("error", "access_denied")]), "s"),
            Err(AuthError::AuthorizationDenied { .. })
        ));
    }
}
