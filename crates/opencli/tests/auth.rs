// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{body_string_contains, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PASSPHRASE: &str = "test-passphrase";

/// An API with one operation per security scheme, served by a mock that is
/// also its OAuth token endpoint.
struct Secure {
    home: TempDir,
    server: MockServer,
}

impl Secure {
    async fn new() -> Self {
        let secure = Self {
            home: TempDir::new().unwrap(),
            server: MockServer::start().await,
        };
        let spec = format!(
            "openapi: 3.0.3
info: {{title: Secure, version: '1'}}
servers: [{{url: https://unused.example.com}}]
components:
  securitySchemes:
    apiKey: {{type: apiKey, name: X-API-Key, in: header}}
    basic: {{type: http, scheme: basic}}
    bearer: {{type: http, scheme: bearer}}
    oauth:
      type: oauth2
      flows:
        clientCredentials: {{tokenUrl: '{}/token', scopes: {{read: Read things}}}}
paths:
  /key: {{get: {{security: [{{apiKey: []}}], responses: {{'200': {{description: ok}}}}}}}}
  /basic: {{get: {{security: [{{basic: []}}], responses: {{'200': {{description: ok}}}}}}}}
  /me: {{get: {{security: [{{oauth: [read]}}], responses: {{'200': {{description: ok}}}}}}}}
  /optional: {{get: {{security: [{{bearer: []}}, {{}}], responses: {{'200': {{description: ok}}}}}}}}
",
            secure.server.uri()
        );
        let file = secure.home.path().join("secure.yaml");
        std::fs::write(&file, spec).unwrap();
        secure
            .opencli()
            .args(["api", "add", "secure", "--spec"])
            .arg(&file)
            .assert()
            .success();
        secure
    }

    fn opencli(&self) -> Command {
        let mut command = cargo_bin_cmd!("opencli");
        command
            .env("OPENCLI_HOME", self.home.path())
            .env("OPENCLI_CREDENTIAL_STORE", "file")
            .env("OPENCLI_PASSPHRASE", PASSPHRASE)
            .env("NO_COLOR", "1");
        command
    }

    /// `opencli --server <mock> secure …`
    fn call(&self, args: &[&str]) -> Command {
        let mut command = self.opencli();
        command
            .args(["--server", &self.server.uri(), "secure"])
            .args(args);
        command
    }

    fn login(&self, args: &[&str], secret: &str) -> Command {
        let mut command = self.opencli();
        command
            .args(["auth", "login", "secure", "--with-token"])
            .args(args)
            .write_stdin(secret);
        command
    }

    fn status(&self) -> Value {
        let output = self
            .opencli()
            .args(["auth", "status", "secure", "-o", "json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn configured(&self, scheme: &str) -> Value {
        self.status()
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["scheme"] == scheme)
            .unwrap()["configured"]
            .clone()
    }

    async fn expect_ok(&self, route: &str, expected_header: (&str, &str)) {
        Mock::given(method("GET"))
            .and(path(route))
            .and(header(expected_header.0, expected_header.1))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .expect(1)
            .mount(&self.server)
            .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn api_key_login_is_used_for_requests() {
    let secure = Secure::new().await;
    secure
        .login(&["--scheme", "apiKey"], "k-123\n")
        .assert()
        .success()
        .stdout(contains(
            "Logged in to `secure` with `apiKey` (stored in the encrypted file).",
        ));
    secure.expect_ok("/key", ("x-api-key", "k-123")).await;
    secure.call(&["key", "list"]).assert().success();
    assert_eq!(secure.configured("apiKey"), "encrypted file");
    assert_eq!(secure.configured("basic"), Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn stored_secrets_never_appear_in_files_or_status() {
    let secure = Secure::new().await;
    secure
        .login(&["--scheme", "apiKey"], "very-secret-key")
        .assert()
        .success();
    let file = std::fs::read_to_string(secure.home.path().join("credentials.enc")).unwrap();
    assert!(!file.contains("very-secret-key"));
    assert!(!secure.status().to_string().contains("very-secret-key"));
}

#[tokio::test(flavor = "multi_thread")]
async fn basic_login_sends_authorization_header() {
    let secure = Secure::new().await;
    secure
        .login(&["--scheme", "basic", "--username", "me"], "pw")
        .assert()
        .success();
    secure
        .expect_ok("/basic", ("authorization", "Basic bWU6cHc="))
        .await;
    secure.call(&["basic", "list"]).assert().success();
}

#[tokio::test(flavor = "multi_thread")]
async fn client_credentials_login_fetches_and_uses_a_token() {
    let secure = Secure::new().await;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=client_credentials"))
        .and(body_string_contains("client_id=app"))
        .and(body_string_contains("client_secret=app-secret"))
        .and(body_string_contains("scope=read"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "tok-1", "expires_in": 3600})),
        )
        .expect(1)
        .mount(&secure.server)
        .await;
    secure
        .login(&["--scheme", "oauth", "--client-id", "app"], "app-secret")
        .assert()
        .success();
    secure
        .expect_ok("/me", ("authorization", "Bearer tok-1"))
        .await;
    secure.call(&["me", "list"]).assert().success();
    let oauth = secure
        .status()
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["scheme"] == "oauth")
        .unwrap()
        .clone();
    assert!(oauth["expires_at"].as_str().is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_client_credentials_store_nothing() {
    let secure = Secure::new().await;
    Mock::given(path("/token"))
        .respond_with(ResponseTemplate::new(401).set_body_json(
            json!({"error": "invalid_client", "error_description": "Unknown client app"}),
        ))
        .mount(&secure.server)
        .await;
    secure
        .login(&["--scheme", "oauth", "--client-id", "app"], "wrong")
        .assert()
        .code(1)
        .stderr(contains("invalid_client").and(contains("Unknown client app")));
    assert_eq!(secure.configured("oauth"), Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_credentials_explain_both_ways_to_provide_them() {
    let secure = Secure::new().await;
    secure.call(&["key", "list"]).assert().code(1).stderr(
        contains("`secure` needs credentials for apiKey")
            .and(contains("opencli auth login secure"))
            .and(contains("OPENCLI_SECURE_APIKEY")),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn environment_credentials_need_no_login() {
    let secure = Secure::new().await;
    secure.expect_ok("/key", ("x-api-key", "from-env")).await;
    secure
        .call(&["key", "list"])
        .env("OPENCLI_SECURE_APIKEY", "from-env")
        .assert()
        .success();
    let status = secure
        .opencli()
        .args(["auth", "status", "secure", "-o", "json"])
        .env("OPENCLI_SECURE_APIKEY", "x")
        .output()
        .unwrap();
    let rows: Value = serde_json::from_slice(&status.stdout).unwrap();
    assert!(
        rows.as_array()
            .unwrap()
            .iter()
            .any(|row| row["scheme"] == "apiKey" && row["configured"] == "environment")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn optional_security_sends_anonymously_or_with_credentials() {
    let secure = Secure::new().await;
    Mock::given(path("/optional"))
        .and(header("authorization", "Bearer b-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"who": "user"})))
        .mount(&secure.server)
        .await;
    Mock::given(path("/optional"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"who": "anonymous"})))
        .mount(&secure.server)
        .await;
    secure
        .call(&["optional", "list"])
        .assert()
        .success()
        .stdout(contains("anonymous"));
    secure
        .call(&["optional", "list"])
        .env("OPENCLI_SECURE_BEARER", "b-1")
        .assert()
        .success()
        .stdout(contains("user"));
}

#[tokio::test(flavor = "multi_thread")]
async fn logout_removes_credentials() {
    let secure = Secure::new().await;
    secure
        .login(&["--scheme", "apiKey"], "k")
        .assert()
        .success();
    secure
        .opencli()
        .args(["auth", "logout", "secure"])
        .assert()
        .success()
        .stdout("Removed credentials for `secure`: apiKey.\n");
    secure
        .call(&["key", "list"])
        .assert()
        .code(1)
        .stderr(contains("needs credentials"));
    secure
        .opencli()
        .args(["auth", "logout", "secure"])
        .assert()
        .success()
        .stdout(contains("No stored credentials"));
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_the_api_removes_its_credentials() {
    let secure = Secure::new().await;
    secure
        .login(&["--scheme", "apiKey"], "k")
        .assert()
        .success();
    secure
        .opencli()
        .args(["api", "remove", "secure", "--yes"])
        .assert()
        .success();
    let file = secure.home.path().join("secure.yaml");
    secure
        .opencli()
        .args(["api", "add", "secure", "--spec"])
        .arg(&file)
        .assert()
        .success();
    assert_eq!(secure.configured("apiKey"), Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn login_needs_explicit_inputs_when_not_interactive() {
    let secure = Secure::new().await;
    secure
        .opencli()
        .args(["auth", "login", "secure"])
        .assert()
        .code(1)
        .stderr(contains("several security schemes").and(contains("apiKey, basic, bearer, oauth")));
    secure
        .opencli()
        .args(["auth", "login", "secure", "--scheme", "nope"])
        .assert()
        .code(1)
        .stderr(contains("no security scheme named `nope`"));
    secure
        .opencli()
        .args(["auth", "login", "secure", "--scheme", "apiKey"])
        .assert()
        .code(1)
        .stderr(contains("--with-token"));
    secure
        .login(&["--scheme", "apiKey"], "")
        .assert()
        .code(1)
        .stderr(contains("non-empty secret"));
    secure
        .login(&["--scheme", "basic"], "pw")
        .assert()
        .code(1)
        .stderr(contains("a username is required").and(contains("--username")));
}

#[tokio::test(flavor = "multi_thread")]
async fn credentials_are_never_sent_over_plain_http() {
    let secure = Secure::new().await;
    secure
        .opencli()
        .args([
            "--server",
            "http://api.example.invalid",
            "secure",
            "key",
            "list",
        ])
        .env("OPENCLI_SECURE_APIKEY", "k")
        .assert()
        .code(1)
        .stderr(contains("refusing to send credentials").and(contains("plain HTTP")));
}

#[tokio::test(flavor = "multi_thread")]
async fn dry_runs_need_no_credentials() {
    let secure = Secure::new().await;
    secure
        .call(&["key", "list", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("GET "));
}

#[tokio::test(flavor = "multi_thread")]
async fn rejected_credentials_suggest_logging_in_again() {
    let secure = Secure::new().await;
    Mock::given(path("/key"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&secure.server)
        .await;
    secure
        .call(&["key", "list"])
        .env("OPENCLI_SECURE_APIKEY", "stale")
        .assert()
        .code(3)
        // miette wraps help text, so match its parts separately.
        .stderr(
            contains("rejected the credentials")
                .and(contains("auth"))
                .and(contains("login secure")),
        );
}

#[tokio::test(flavor = "multi_thread")]
async fn wrong_passphrase_is_reported() {
    let secure = Secure::new().await;
    secure
        .login(&["--scheme", "apiKey"], "k")
        .assert()
        .success();
    secure
        .call(&["key", "list"])
        .env("OPENCLI_PASSPHRASE", "not-it")
        .assert()
        .code(1)
        .stderr(contains("wrong passphrase").and(contains("OPENCLI_PASSPHRASE")));
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_passphrase_is_reported_when_not_interactive() {
    let secure = Secure::new().await;
    secure
        .login(&["--scheme", "apiKey"], "k")
        .env_remove("OPENCLI_PASSPHRASE")
        .assert()
        .code(1)
        .stderr(contains("set OPENCLI_PASSPHRASE"));
}
