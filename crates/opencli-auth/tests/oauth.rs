// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use opencli_auth::{AuthError, CallbackListener, Pkce, TokenClient};
use secrecy::{ExposeSecret, SecretString};
use serde_json::json;
use time::macros::datetime;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> TokenClient {
    TokenClient::new(Duration::from_secs(5)).unwrap()
}

fn secret(value: &str) -> SecretString {
    SecretString::from(value.to_owned())
}

#[tokio::test]
async fn client_credentials_posts_form_and_parses_tokens() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string_contains("grant_type=client_credentials"))
        .and(body_string_contains("client_id=app"))
        .and(body_string_contains("client_secret=s%26cret"))
        .and(body_string_contains("scope=read+write"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "access_token": "a-1", "token_type": "bearer", "expires_in": "120", "scope": "read write"
        })))
        .expect(1)
        .mount(&server)
        .await;
    let response = client()
        .client_credentials(
            &format!("{}/token", server.uri()),
            "app",
            &secret("s&cret"),
            &["read".into(), "write".into()],
        )
        .await
        .unwrap();
    assert_eq!(response.access_token.expose_secret(), "a-1");
    assert_eq!(response.expires_in, Some(120));
    assert_eq!(response.scope.as_deref(), Some("read write"));
    assert_eq!(
        response.expires_at(datetime!(2026-11-05 12:00 UTC)),
        Some(datetime!(2026-11-05 12:02 UTC))
    );
}

#[tokio::test]
async fn token_endpoint_errors_carry_oauth_error_code() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(401).set_body_json(
                json!({"error": "invalid_client", "error_description": "bad secret"}),
            ),
        )
        .mount(&server)
        .await;
    let error = client()
        .client_credentials(&format!("{}/token", server.uri()), "app", &secret("x"), &[])
        .await
        .unwrap_err();
    assert!(
        matches!(&error, AuthError::TokenEndpoint { status: 401, error: Some(code), description: Some(text), .. } if code == "invalid_client" && text == "bad secret"),
        "{error:?}"
    );
    assert_eq!(
        error.to_string(),
        format!(
            "token endpoint {}/token returned HTTP 401: invalid_client",
            server.uri()
        )
    );
}

#[tokio::test]
async fn token_endpoint_redirects_are_not_followed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "https://evil.example.com/token"),
        )
        .mount(&server)
        .await;
    let error = client()
        .client_credentials(&format!("{}/token", server.uri()), "app", &secret("x"), &[])
        .await
        .unwrap_err();
    assert!(
        matches!(error, AuthError::TokenEndpoint { status: 302, .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn non_bearer_tokens_are_rejected() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "a", "token_type": "mac"})),
        )
        .mount(&server)
        .await;
    let error = client()
        .client_credentials(&format!("{}/token", server.uri()), "app", &secret("x"), &[])
        .await
        .unwrap_err();
    assert!(
        matches!(error, AuthError::TokenResponse { .. }),
        "{error:?}"
    );
}

#[tokio::test]
async fn plain_http_token_endpoints_are_refused() {
    let error = client()
        .client_credentials("http://auth.example.com/token", "app", &secret("x"), &[])
        .await
        .unwrap_err();
    assert!(matches!(error, AuthError::InsecureEndpoint(_)), "{error:?}");
}

#[tokio::test]
async fn exchange_code_sends_verifier_and_redirect_uri() {
    let server = MockServer::start().await;
    let pkce = Pkce::from_verifier("v-123".into());
    Mock::given(body_string_contains("grant_type=authorization_code"))
        .and(body_string_contains("code=c-1"))
        .and(body_string_contains("code_verifier=v-123"))
        .and(body_string_contains(
            "redirect_uri=http%3A%2F%2F127.0.0.1%3A9%2Fcallback",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "a", "refresh_token": "r"})),
        )
        .expect(1)
        .mount(&server)
        .await;
    let response = client()
        .exchange_code(
            &format!("{}/token", server.uri()),
            "app",
            None,
            "c-1",
            &pkce,
            "http://127.0.0.1:9/callback",
        )
        .await
        .unwrap();
    assert_eq!(response.refresh_token.unwrap().expose_secret(), "r");
}

#[tokio::test(flavor = "multi_thread")]
async fn callback_listener_returns_code_and_ignores_other_paths() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let redirect_uri = listener.redirect_uri();
    let waiting = tokio::spawn(listener.wait_for_code("st4te", Duration::from_secs(5)));
    let base = redirect_uri.trim_end_matches("/callback").to_owned();
    let browser = reqwest::Client::new();

    let favicon = browser
        .get(format!("{base}/favicon.ico"))
        .send()
        .await
        .unwrap();
    assert_eq!(favicon.status(), 404);
    let page = browser
        .get(format!("{redirect_uri}?code=c-1&state=st4te"))
        .send()
        .await
        .unwrap();
    assert!(page.text().await.unwrap().contains("Signed in"));

    assert_eq!(waiting.await.unwrap().unwrap(), "c-1");
}

#[tokio::test(flavor = "multi_thread")]
async fn callback_listener_rejects_state_mismatch() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let redirect_uri = listener.redirect_uri();
    let waiting = tokio::spawn(listener.wait_for_code("expected", Duration::from_secs(5)));
    let page = reqwest::get(format!("{redirect_uri}?code=c&state=forged"))
        .await
        .unwrap();
    assert!(page.text().await.unwrap().contains("failed"));
    assert!(matches!(
        waiting.await.unwrap(),
        Err(AuthError::StateMismatch)
    ));
}

#[tokio::test]
async fn callback_listener_times_out() {
    let listener = CallbackListener::bind(0).await.unwrap();
    let result = listener.wait_for_code("s", Duration::from_millis(50)).await;
    assert!(matches!(result, Err(AuthError::CallbackTimeout)));
}
