// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// An identity API (token as an `Authorization` API key, with a login) and a
/// trading API (HTTP bearer, no login) on one mock server (ADR 0011).
const IDENTITY: &str = "openapi: 3.0.3
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
            schema: {type: object, properties: {email: {type: string}, password: {type: string, format: password}}}
      responses: {'200': {description: ok}}
";
const TRADING: &str = "openapi: 3.0.3
info: {title: Trading, version: '1'}
servers: [{url: 'https://unused.example.com'}]
components:
  securitySchemes:
    Bearer: {type: http, scheme: bearer, bearerFormat: JWT}
security: [{Bearer: []}]
paths:
  /api/v1/afterhours:
    get: {responses: {'200': {description: ok}}}
";
const OPEN: &str = "openapi: 3.0.3
info: {title: Open, version: '1'}
servers: [{url: 'https://unused.example.com'}]
paths:
  /things:
    get: {responses: {'200': {description: ok}}}
";

struct Apis {
    home: TempDir,
    server: MockServer,
}

impl Apis {
    async fn new() -> Self {
        let apis = Self {
            home: TempDir::new().unwrap(),
            server: MockServer::start().await,
        };
        for (name, spec) in [("identity", IDENTITY), ("trading", TRADING), ("open", OPEN)] {
            let file = apis.home.path().join(format!("{name}.yaml"));
            std::fs::write(&file, spec).unwrap();
            apis.opencli()
                .args(["api", "add", name, "--spec"])
                .arg(&file)
                .assert()
                .success();
        }
        apis
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

    fn link(&self) -> Command {
        let mut command = self.opencli();
        command.args(["api", "set-credentials", "trading", "--from", "identity"]);
        command
    }

    fn trading(&self) -> Command {
        let mut command = self.opencli();
        command.args([
            "--server",
            &self.server.uri(),
            "trading",
            "v1",
            "afterhours",
        ]);
        command
    }

    async fn sign_in(&self, token: &str) {
        Mock::given(method("POST"))
            .and(path("/api/v1/account/login"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"data": {"token": token}})),
            )
            .mount(&self.server)
            .await;
        self.opencli()
            .args(["--server", &self.server.uri(), "identity", "v1", "account"])
            .args([
                "login",
                "post",
                "--body",
                r#"{"email": "a@b.c", "password": "p"}"#,
            ])
            .arg("--yes")
            .assert()
            .success();
    }

    async fn expect_trading_token(&self, token: &str) {
        Mock::given(method("GET"))
            .and(path("/api/v1/afterhours"))
            .and(header("Authorization", format!("Bearer {token}").as_str()))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": []})))
            .expect(1)
            .mount(&self.server)
            .await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_linked_api_calls_with_the_other_apis_token() {
    let apis = Apis::new().await;
    apis.link()
        .assert()
        .success()
        .stdout(contains("`trading` now uses `identity`'s credentials."));
    apis.sign_in("jwt-1").await;
    apis.expect_trading_token("jwt-1").await;

    apis.trading().assert().success();
    apis.opencli()
        .args(["auth", "status", "trading"])
        .assert()
        .success()
        .stdout(contains("from identity"));
    apis.opencli()
        .args(["api", "show", "trading"])
        .assert()
        .success()
        .stdout(contains("Credentials: from `identity`"));
    let audit = std::fs::read_to_string(apis.home.path().join("audit.log")).unwrap();
    assert!(
        audit.contains(
            r#""event":"api.set-credentials","api":"trading","credentials_from":"identity""#
        ),
        "{audit}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_in_to_a_linked_api_signs_in_to_its_source() {
    let apis = Apis::new().await;
    apis.link().assert().success();
    apis.opencli()
        .args(["auth", "login", "trading", "--with-token"])
        .write_stdin("Bearer jwt-2")
        .assert()
        .success()
        .stderr(contains(
            "`trading` uses `identity`'s credentials; signing in to `identity`.",
        ))
        .stdout(contains("Logged in to `identity`"));
    apis.expect_trading_token("jwt-2").await;
    apis.trading().assert().success();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_link_without_a_source_credential_says_where_to_sign_in() {
    let apis = Apis::new().await;
    apis.link().assert().success();
    apis.trading().assert().code(1).stderr(
        contains("`trading` uses `identity`'s credentials, and `identity` has none")
            .and(contains("opencli auth login identity")),
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_environment_still_overrides_a_link() {
    let apis = Apis::new().await;
    apis.link().assert().success();
    apis.sign_in("from-identity").await;
    apis.expect_trading_token("from-env").await;
    apis.trading()
        .env("OPENCLI_TRADING_BEARER", "from-env")
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread")]
async fn clearing_or_removing_the_source_ends_the_link() {
    let apis = Apis::new().await;
    apis.link().assert().success();
    apis.opencli()
        .args(["api", "set-credentials", "trading", "--clear"])
        .assert()
        .success()
        .stdout(contains("`trading` uses its own credentials again."));
    apis.opencli()
        .args(["auth", "status", "trading"])
        .assert()
        .stdout(contains("from identity").not());

    apis.link().assert().success();
    apis.opencli()
        .args(["api", "remove", "identity", "--yes"])
        .assert()
        .success();
    apis.opencli()
        .args(["api", "show", "trading"])
        .assert()
        .success()
        .stdout(contains("Credentials: its own"));
}

#[rstest::rstest]
#[case(&["trading", "--from", "trading"], "`trading` can't use its own credentials")]
#[case(&["trading", "--from", "open"], "`open` needs exactly one token scheme")]
#[case(&["trading", "--from", "nope"], "nope")]
#[tokio::test(flavor = "multi_thread")]
async fn invalid_links_are_refused(#[case] args: &[&str], #[case] message: &str) {
    let apis = Apis::new().await;
    apis.opencli()
        .args(["api", "set-credentials"])
        .args(args)
        .assert()
        .failure()
        .stderr(contains(message));
}

#[tokio::test(flavor = "multi_thread")]
async fn links_are_one_level_deep() {
    let apis = Apis::new().await;
    apis.link().assert().success();
    let identity_spec = apis.home.path().join("identity.yaml");
    apis.opencli()
        .args(["api", "add", "ledger", "--spec"])
        .arg(&identity_spec)
        .assert()
        .success();
    apis.opencli()
        .args(["api", "set-credentials", "ledger", "--from", "trading"])
        .assert()
        .failure()
        .stderr(contains("`trading` already uses `identity`'s credentials"));
    apis.opencli()
        .args(["api", "set-credentials", "identity", "--from", "ledger"])
        .assert()
        .failure()
        .stderr(contains("trading use `identity`'s credentials"));
}
