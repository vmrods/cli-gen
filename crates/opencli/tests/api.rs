// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::Value;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// An isolated OpenCLI home so tests never touch the real data directory.
struct Home(TempDir);

impl Home {
    fn new() -> Self {
        Self(TempDir::new().unwrap())
    }

    fn opencli(&self) -> Command {
        let mut command = cargo_bin_cmd!("opencli");
        command
            .env("OPENCLI_HOME", self.0.path())
            .env("OPENCLI_CREDENTIAL_STORE", "file")
            .env("OPENCLI_PASSPHRASE", "test-passphrase")
            .env("NO_COLOR", "1");
        command
    }

    fn add_petstore(&self) {
        self.opencli()
            .args(["api", "add", "petstore", "--spec"])
            .arg(fixture("petstore.yaml"))
            .assert()
            .success();
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self
            .opencli()
            .args(args)
            .args(["--output", "json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn write(&self, file: &str, contents: &str) -> PathBuf {
        let path = self.0.path().join(file);
        std::fs::write(&path, contents).unwrap();
        path
    }
}

#[test]
fn add_registers_api_from_yaml_file() {
    let home = Home::new();
    home.opencli()
        .args(["api", "add", "petstore", "--spec"])
        .arg(fixture("petstore.yaml"))
        .assert()
        .success()
        .stdout(contains("Added `petstore`: Petstore 1.0.0, 4 operations."));
}

#[test]
fn add_registers_api_from_json_file_with_absolute_source() {
    let home = Home::new();
    let added = {
        let output = home
            .opencli()
            .args(["api", "add", "pets", "--output", "json", "--spec"])
            .arg(fixture("petstore.json"))
            .output()
            .unwrap();
        assert!(output.status.success());
        serde_json::from_slice::<Value>(&output.stdout).unwrap()
    };
    assert_eq!(added["name"], "pets");
    assert_eq!(added["operation_count"], 4);
    assert!(Path::new(added["source"].as_str().unwrap()).is_absolute());
}

#[test]
fn add_rejects_duplicate_name_with_help() {
    let home = Home::new();
    home.add_petstore();
    home.opencli()
        .args(["api", "add", "petstore", "--spec"])
        .arg(fixture("petstore.yaml"))
        .assert()
        .failure()
        .stderr(
            contains("an API named `petstore` already exists")
                .and(contains("opencli api remove petstore")),
        );
}

#[test]
fn add_reports_every_validation_issue() {
    let home = Home::new();
    let spec = home.write(
        "broken.yaml",
        "openapi: 3.0.3\ninfo: {title: Broken, version: '1'}\npaths:\n  /a/{id}:\n    get:\n      security: [{missing: []}]\n      responses: {'200': {description: ok}}\n",
    );
    home.opencli()
        .args(["api", "add", "broken", "--spec"])
        .arg(&spec)
        .assert()
        .failure()
        .stderr(
            contains("failed validation with 2 issue(s)")
                .and(contains(
                    "path parameter `id` is used in the path but not declared",
                ))
                .and(contains("security scheme `missing` is not defined")),
        );
    assert_eq!(home.json(&["api", "list"]), Value::Array(vec![]));
}

#[test]
fn add_reports_syntax_error_location() {
    let home = Home::new();
    let spec = home.write("bad.json", "{\n  \"openapi\": \"3.0.3\",\n  oops\n}\n");
    home.opencli()
        .args(["api", "add", "bad", "--spec"])
        .arg(&spec)
        .assert()
        .failure()
        .stderr(contains("bad.json is not valid JSON or YAML").and(contains("oops")));
}

#[test]
fn add_reports_missing_file() {
    let home = Home::new();
    home.opencli()
        .args(["api", "add", "nope", "--spec", "does-not-exist.yaml"])
        .assert()
        .failure()
        .stderr(contains("could not read does-not-exist.yaml"));
}

#[test]
fn add_requires_spec_when_not_interactive() {
    let home = Home::new();
    home.opencli()
        .args(["api", "add", "petstore"])
        .assert()
        .failure()
        .stderr(contains("no specification given").and(contains("--spec")));
}

#[test]
fn add_rejects_reserved_name_as_usage_error() {
    let home = Home::new();
    home.opencli()
        .args(["api", "add", "auth", "--spec"])
        .arg(fixture("petstore.yaml"))
        .assert()
        .code(2)
        .stderr(contains("reserved for a built-in command"));
}

#[test]
fn add_refuses_plain_http_to_remote_host() {
    let home = Home::new();
    home.opencli()
        .args([
            "api",
            "add",
            "remote",
            "--spec",
            "http://api.example.com/openapi.json",
        ])
        .assert()
        .failure()
        .stderr(contains("refusing to fetch").and(contains("plain HTTP")));
}

#[tokio::test(flavor = "multi_thread")]
async fn add_fetches_spec_from_url() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/openapi.yaml"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(include_str!("../../../fixtures/petstore.yaml")),
        )
        .mount(&server)
        .await;
    let url = format!("{}/openapi.yaml", server.uri());
    let home = Home::new();

    home.opencli()
        .args(["api", "add", "remote", "--spec", &url])
        .assert()
        .success();

    let apis = home.json(&["api", "list"]);
    assert_eq!(apis[0]["source"], url.as_str());
    assert_eq!(apis[0]["operation_count"], 4);
}

#[tokio::test(flavor = "multi_thread")]
async fn add_reports_http_error_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    let home = Home::new();
    home.opencli()
        .args([
            "api",
            "add",
            "remote",
            "--spec",
            &format!("{}/missing.json", server.uri()),
        ])
        .assert()
        .failure()
        .stderr(contains("returned HTTP 404"));
}

#[test]
fn list_prints_hint_when_empty() {
    Home::new()
        .opencli()
        .args(["api", "list"])
        .assert()
        .success()
        .stdout(contains("No APIs registered"));
}

#[test]
fn list_prints_table_of_apis() {
    let home = Home::new();
    home.add_petstore();
    home.opencli()
        .args(["api", "list"])
        .assert()
        .success()
        .stdout(contains("NAME").and(contains("petstore  Petstore  1.0.0")));
}

#[test]
fn show_prints_operations_and_security() {
    let home = Home::new();
    home.add_petstore();
    home.opencli()
        .args(["api", "show", "petstore"])
        .assert()
        .success()
        .stdout(
            contains("petstore: Petstore 1.0.0 (openapi-3.0)")
                .and(contains("Servers:   https://eu.petstore.example.com/v1"))
                .and(contains("apiKey (api-key)"))
                .and(contains("GET     /pets"))
                .and(contains("deletePet     (deprecated)")),
        );
}

#[test]
fn show_outputs_json_details() {
    let home = Home::new();
    home.add_petstore();
    let details = home.json(&["api", "show", "petstore"]);
    assert_eq!(details["title"], "Petstore");
    assert_eq!(details["operations"].as_array().unwrap().len(), 4);
    assert_eq!(details["security_schemes"][0]["name"], "apiKey");
}

#[test]
fn show_reports_unknown_api_with_help() {
    Home::new()
        .opencli()
        .args(["api", "show", "nope"])
        .assert()
        .failure()
        .stderr(contains("no API named `nope`").and(contains("opencli api list")));
}

#[test]
fn versions_lists_current_version() {
    let home = Home::new();
    home.add_petstore();
    let versions = home.json(&["api", "versions", "petstore"]);
    assert_eq!(versions[0]["number"], 1);
    assert_eq!(versions[0]["current"], true);
    home.opencli()
        .args(["api", "versions", "petstore"])
        .assert()
        .success()
        .stdout(contains("1 *"));
}

#[test]
fn remove_requires_yes_when_not_interactive() {
    let home = Home::new();
    home.add_petstore();
    home.opencli()
        .args(["api", "remove", "petstore"])
        .assert()
        .failure()
        .stderr(contains("needs confirmation").and(contains("--yes")));
    assert_eq!(home.json(&["api", "list"]).as_array().unwrap().len(), 1);
}

#[test]
fn remove_with_yes_deletes_api() {
    let home = Home::new();
    home.add_petstore();
    home.opencli()
        .args(["api", "remove", "petstore", "--yes"])
        .assert()
        .success()
        .stdout(contains("Removed `petstore`."));
    assert_eq!(home.json(&["api", "list"]), Value::Array(vec![]));
}

#[test]
fn remove_reports_unknown_api() {
    Home::new()
        .opencli()
        .args(["api", "remove", "nope", "--yes"])
        .assert()
        .failure()
        .stderr(contains("no API named `nope`"));
}
