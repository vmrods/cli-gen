// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const LOGIN_BODY: &str = r#"{"email": "ana@example.com", "password": "s3cret"}"#;

/// An ASP.NET-style API: a bearer token sent as an `Authorization` API key,
/// and a login operation that returns it under `data.token` (ADR 0010).
struct Identity {
    home: TempDir,
    server: MockServer,
}

impl Identity {
    async fn new() -> Self {
        let identity = Self {
            home: TempDir::new().unwrap(),
            server: MockServer::start().await,
        };
        let spec = "openapi: 3.0.3
info: {title: Identity, version: '1'}
servers: [{url: 'https://unused.example.com'}]
components:
  securitySchemes:
    Bearer: {type: apiKey, name: Authorization, in: header}
paths:
  /api/v1/account/login:
    post:
      requestBody:
        content:
          application/json:
            schema:
              type: object
              required: [email, password]
              properties:
                email: {type: string}
                password: {type: string, format: password}
      responses: {'200': {description: ok}}
  /api/v1/account/signup:
    post:
      requestBody:
        content:
          application/json:
            schema: {type: object, properties: {email: {type: string}}}
      responses: {'200': {description: ok}}
  /api/v1/me:
    get:
      security: [{Bearer: []}]
      responses: {'200': {description: ok}}
";
        let file = identity.home.path().join("identity.yaml");
        std::fs::write(&file, spec).unwrap();
        identity
            .opencli()
            .args(["api", "add", "identity", "--spec"])
            .arg(&file)
            .assert()
            .success();
        identity
    }

    fn opencli(&self) -> Command {
        let mut command = cargo_bin_cmd!("opencli");
        command
            .env("OPENCLI_HOME", self.home.path())
            .env("OPENCLI_CREDENTIAL_STORE", "file")
            .env("OPENCLI_PASSPHRASE", "test-passphrase")
            .env("NO_COLOR", "1");
        command
    }

    fn call(&self, args: &[&str]) -> Command {
        let mut command = self.opencli();
        command
            .args(["--server", &self.server.uri(), "identity"])
            .args(args);
        command
    }

    fn login(&self, extra: &[&str]) -> Command {
        let mut command = self.call(&[
            "v1", "account", "login", "post", "--body", LOGIN_BODY, "--yes",
        ]);
        command.args(extra);
        command
    }

    async fn login_responds(&self, status: u16, body: Value) {
        Mock::given(method("POST"))
            .and(path("/api/v1/account/login"))
            .respond_with(ResponseTemplate::new(status).set_body_json(body))
            .mount(&self.server)
            .await;
    }

    fn configured(&self) -> Value {
        let output = self
            .opencli()
            .args(["auth", "status", "identity", "-o", "json"])
            .output()
            .unwrap();
        let rows: Value = serde_json::from_slice(&output.stdout).unwrap();
        rows[0]["configured"].clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_successful_sign_in_saves_its_token_for_later_calls() {
    let identity = Identity::new().await;
    identity
        .login_responds(200, json!({"data": {"token": "abc", "refreshToken": "r"}}))
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v1/me"))
        .and(header("Authorization", "Bearer abc"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": 1})))
        .expect(1)
        .mount(&identity.server)
        .await;

    identity
        .login(&["-o", "json"])
        .assert()
        .success()
        .stdout(contains("\"token\": \"abc\""))
        .stderr(contains(
            "Signed in: saved the token for `identity` (`Bearer`, encrypted file)",
        ));
    identity.call(&["v1", "me"]).assert().success();
    let audit = std::fs::read_to_string(identity.home.path().join("audit.log")).unwrap();
    assert!(audit.contains("\"event\":\"auth.login\""), "{audit}");
    assert!(
        !audit.contains("abc"),
        "the token must not reach the audit log"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn no_save_leaves_credentials_alone() {
    let identity = Identity::new().await;
    identity
        .login_responds(200, json!({"data": {"token": "abc"}}))
        .await;
    identity
        .login(&["--no-save"])
        .assert()
        .success()
        .stderr(contains("Signed in").not());
    assert_eq!(identity.configured(), json!(null));
}

#[rstest::rstest]
#[case(200, json!({"data": {"id": 1}}))]
#[case(401, json!({"errors": ["bad password"]}))]
#[tokio::test(flavor = "multi_thread")]
async fn nothing_is_saved_without_a_token_or_on_failure(#[case] status: u16, #[case] body: Value) {
    let identity = Identity::new().await;
    identity.login_responds(status, body).await;
    let assert = identity.login(&[]).assert();
    let assert = if status == 200 {
        assert.success()
    } else {
        assert.code(3)
    };
    assert.stderr(contains("Signed in").not());
    assert_eq!(identity.configured(), json!(null));
}

#[tokio::test(flavor = "multi_thread")]
async fn only_sign_in_commands_offer_no_save() {
    let identity = Identity::new().await;
    identity
        .call(&["v1", "account", "login", "post", "--help"])
        .assert()
        .success()
        .stdout(contains("--no-save"));
    identity
        .call(&["v1", "account", "signup", "post", "--help"])
        .assert()
        .success()
        .stdout(contains("--no-save").not());
}
