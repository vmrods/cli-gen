// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::json;
use tempfile::TempDir;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// The Accounts fixture registered in a temporary home, with no terminal:
/// the tests run as a script or agent would.
struct Accounts {
    home: TempDir,
}

impl Accounts {
    fn new() -> Self {
        let accounts = Self {
            home: TempDir::new().unwrap(),
        };
        accounts
            .bare()
            .args(["api", "add", "accounts", "--spec"])
            .arg(fixture("accounts.yaml"))
            .assert()
            .success();
        accounts
    }

    fn bare(&self) -> Command {
        let mut command = cargo_bin_cmd!("opencli");
        command
            .env("OPENCLI_HOME", self.home.path())
            .env("OPENCLI_CREDENTIAL_STORE", "file")
            .env("OPENCLI_PASSPHRASE", "test-passphrase")
            .env("NO_COLOR", "1");
        command
    }

    fn create(&self, server: Option<&str>, args: &[&str]) -> Command {
        let mut command = self.bare();
        if let Some(server) = server {
            command.args(["--server", server]);
        }
        command.args(["accounts", "accounts", "create"]).args(args);
        command
    }
}

#[test]
fn secret_fields_are_not_flags() {
    let accounts = Accounts::new();
    accounts
        .create(None, &["--help"])
        .assert()
        .success()
        .stdout(contains("--email").and(contains("--password").not()));
    accounts
        .create(None, &["--password", "hunter2", "--dry-run"])
        .assert()
        .code(2);
}

#[test]
fn invalid_bodies_are_rejected_before_sending_with_every_problem() {
    Accounts::new()
        .create(
            None,
            &[
                "--body",
                r#"{"age": "old", "address": {}}"#,
                "--email",
                "a@b.c",
                "--dry-run",
            ],
        )
        .assert()
        .code(1)
        .stderr(
            contains("body.password: required field is missing")
                .and(contains("body.role: required field is missing"))
                .and(contains("body.age: expected integer, got string"))
                .and(contains("body.address.city: required field is missing"))
                .and(contains("Secret fields (password) are never flags"))
                .and(contains("--no-validate")),
        );
}

#[test]
fn missing_secrets_point_to_body_input_and_valid_bodies_pass() {
    let accounts = Accounts::new();
    accounts
        .create(None, &["--email", "a@b.c", "--role", "admin", "--dry-run"])
        .assert()
        .code(1)
        .stderr(contains("--body @file or --body -"));
    accounts
        .create(
            None,
            &[
                "--body",
                "-",
                "--email",
                "a@b.c",
                "--role",
                "admin",
                "--dry-run",
            ],
        )
        .write_stdin(r#"{"password": "hunter2"}"#)
        .assert()
        .success()
        .stdout(contains(r#""password": "hunter2""#));
}

#[tokio::test]
async fn no_validate_sends_the_body_as_given() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/accounts"))
        .and(body_json(json!({"email": "a@b.c"})))
        .respond_with(ResponseTemplate::new(201))
        .expect(1)
        .mount(&server)
        .await;
    Accounts::new()
        .create(
            Some(&server.uri()),
            &["--email", "a@b.c", "--no-validate", "--yes"],
        )
        .assert()
        .success();
}

#[test]
fn interactive_mode_needs_a_terminal() {
    Accounts::new()
        .create(None, &["--interactive"])
        .assert()
        .code(1)
        .stderr(contains("--interactive needs a terminal"));
}

#[test]
fn operation_help_shows_examples_built_from_the_operation() {
    Accounts::new()
        .create(None, &["--help"])
        .assert()
        .success()
        .stdout(
            contains("Calls POST /accounts")
                .and(contains("Examples:"))
                .and(contains(
                    "opencli accounts accounts create --body @body.json",
                ))
                .and(contains("opencli accounts accounts create -i"))
                .and(contains("opencli accounts accounts create --describe")),
        );
}
