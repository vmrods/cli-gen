// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

struct Home(TempDir);

impl Home {
    fn new() -> Self {
        Self(TempDir::new().unwrap())
    }

    fn with_petstore() -> Self {
        let home = Self::new();
        home.opencli()
            .args(["api", "add", "petstore", "--spec"])
            .arg(fixture("petstore.yaml"))
            .assert()
            .success();
        home
    }

    fn opencli(&self) -> Command {
        let mut command = cargo_bin_cmd!("opencli");
        command
            .env("OPENCLI_HOME", self.0.path())
            .env("OPENCLI_CREDENTIAL_STORE", "file")
            .env("OPENCLI_PASSPHRASE", "test-passphrase")
            .env_remove("RUST_LOG")
            .env_remove("OPENCLI_AUDIT");
        command
    }

    fn audit_path(&self) -> PathBuf {
        self.0.path().join("audit.log")
    }

    fn audit(&self) -> Vec<Value> {
        std::fs::read_to_string(self.audit_path())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

#[test]
fn completion_scripts_include_registered_api_commands() {
    let home = Home::with_petstore();
    let output = home
        .opencli()
        .args(["completion", "bash"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let script = String::from_utf8(output.stdout).unwrap();
    for word in [
        "opencli",
        "petstore",
        "pets",
        "create",
        "--limit",
        "auth",
        "completion",
    ] {
        assert!(script.contains(word), "bash completion is missing `{word}`");
    }
    for shell in ["zsh", "fish", "powershell", "elvish"] {
        home.opencli()
            .args(["completion", shell])
            .assert()
            .success()
            .stdout(contains("petstore"));
    }
}

#[test]
fn completion_works_with_no_apis_and_rejects_unknown_shells() {
    let home = Home::new();
    home.opencli()
        .args(["completion", "bash"])
        .assert()
        .success()
        .stdout(contains("opencli"));
    home.opencli().args(["completion", "tcsh"]).assert().code(2);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_audit_log_records_calls_and_changes_without_secrets() {
    let server = MockServer::start().await;
    Mock::given(path("/pets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .mount(&server)
        .await;
    let home = Home::with_petstore();
    home.opencli()
        .args([
            "auth",
            "login",
            "petstore",
            "--scheme",
            "apiKey",
            "--with-token",
        ])
        .write_stdin("super-secret-key")
        .assert()
        .success();
    home.opencli()
        .args([
            "--server",
            &server.uri(),
            "petstore",
            "pets",
            "list",
            "--limit",
            "5",
        ])
        .assert()
        .success();
    home.opencli()
        .args(["auth", "logout", "petstore"])
        .assert()
        .success();
    home.opencli()
        .args(["api", "remove", "petstore", "--yes"])
        .assert()
        .success();

    let events: Vec<String> = home
        .audit()
        .iter()
        .map(|record| record["event"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        events,
        [
            "api.add",
            "auth.login",
            "request",
            "auth.logout",
            "api.remove"
        ]
    );
    let request = home
        .audit()
        .into_iter()
        .find(|record| record["event"] == "request")
        .unwrap();
    assert_eq!(request["api"], "petstore");
    assert_eq!(request["command"], "pets list");
    assert_eq!(request["method"], "GET");
    assert_eq!(request["url"], format!("{}/pets", server.uri()));
    assert_eq!(request["status"], 200);
    assert_eq!(request["outcome"], "ok");
    assert_eq!(request["schemes"], json!(["apiKey"]));
    assert!(request["timestamp"].as_str().is_some() && request["duration_ms"].is_u64());

    let raw = std::fs::read_to_string(home.audit_path()).unwrap();
    assert!(
        !raw.contains("super-secret-key"),
        "secrets must never be audited"
    );
    assert!(
        !raw.contains("limit=5"),
        "query strings must never be audited"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn network_failures_and_http_errors_are_audited() {
    let server = MockServer::start().await;
    Mock::given(path("/pets"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;
    let home = Home::with_petstore();
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    for (base, code) in [(server.uri(), 4), (format!("http://127.0.0.1:{port}"), 5)] {
        home.opencli()
            .args(["--server", &base, "petstore", "pets", "list"])
            .env("OPENCLI_PETSTORE_APIKEY", "k")
            .assert()
            .code(code);
    }
    let outcomes: Vec<(Value, Value)> = home
        .audit()
        .into_iter()
        .filter(|record| record["event"] == "request")
        .map(|record| (record["outcome"].clone(), record["status"].clone()))
        .collect();
    assert_eq!(
        outcomes,
        [
            (json!("http-error"), json!(503)),
            (json!("network-error"), Value::Null)
        ]
    );
}

#[test]
fn the_audit_log_can_be_turned_off() {
    let home = Home::new();
    home.opencli()
        .args(["api", "add", "petstore", "--spec"])
        .arg(fixture("petstore.yaml"))
        .env("OPENCLI_AUDIT", "off")
        .assert()
        .success();
    assert!(!home.audit_path().exists());
}

#[cfg(unix)]
#[test]
fn the_audit_log_is_readable_only_by_its_owner() {
    use std::os::unix::fs::PermissionsExt as _;
    let home = Home::with_petstore();
    let mode = std::fs::metadata(home.audit_path())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn verbose_shows_phase_timings_on_stderr_only() {
    let home = Home::with_petstore();
    home.opencli()
        .args(["-v", "petstore", "pets", "list", "--dry-run"])
        .assert()
        .success()
        .stdout("GET https://eu.petstore.example.com/v1/pets\n")
        .stderr(
            contains("loaded commands")
                .and(contains("elapsed_ms"))
                .and(contains("request planned")),
        );
    home.opencli()
        .args(["petstore", "pets", "list", "--dry-run"])
        .assert()
        .success()
        .stderr("");
}

#[tokio::test(flavor = "multi_thread")]
async fn output_has_no_spinner_or_colors_when_not_a_terminal() {
    let server = MockServer::start().await;
    Mock::given(path("/pets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": "1"}])))
        .mount(&server)
        .await;
    let home = Home::with_petstore();
    let output = home
        .opencli()
        .args(["--server", &server.uri(), "petstore", "pets", "list"])
        .env("OPENCLI_PETSTORE_APIKEY", "k")
        .env("CLICOLOR_FORCE", "0")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "ID\n1\n");
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
}
