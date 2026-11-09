// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A spec whose only server is relative, imported from a file: nothing to
/// resolve it against, so onboarding must supply a base URL.
const RELATIVE: &str = "openapi: 3.0.3\ninfo: {title: Internal, version: '1'}\nservers: [{url: /v1}]\npaths:\n  /things:\n    get:\n      responses: {'200': {description: ok}}\n";
/// A spec with absolute servers: onboarding can default to the first.
const ABSOLUTE: &str = "openapi: 3.0.3\ninfo: {title: Public, version: '1'}\nservers: [{url: 'https://prod.example.com/v1'}, {url: 'https://staging.example.com/v1'}]\npaths:\n  /things:\n    get:\n      responses: {'200': {description: ok}}\n";

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

    fn spec(&self, name: &str, text: &str) -> PathBuf {
        let file = self.0.path().join(format!("{name}.yaml"));
        std::fs::write(&file, text).unwrap();
        file
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self
            .opencli()
            .args(args)
            .args(["-o", "json"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

#[test]
fn onboarding_without_a_usable_server_needs_a_base_url() {
    let home = Home::new();
    let file = home.spec("internal", RELATIVE);
    home.opencli()
        .args(["api", "add", "internal", "--spec"])
        .arg(&file)
        .assert()
        .code(1)
        .stderr(contains("`internal` needs a base URL").and(contains("--server")));
    assert_eq!(home.json(&["api", "list"]), json!([]));
}

#[tokio::test(flavor = "multi_thread")]
async fn the_base_url_given_at_onboarding_is_used_for_every_call() {
    let server = MockServer::start().await;
    Mock::given(path("/things"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": 1}])))
        .expect(1)
        .mount(&server)
        .await;
    let home = Home::new();
    let file = home.spec("internal", RELATIVE);
    home.opencli()
        .args([
            "--server",
            &server.uri(),
            "api",
            "add",
            "internal",
            "--spec",
        ])
        .arg(&file)
        .assert()
        .success()
        .stdout(contains(format!("calling {}", server.uri())));
    assert_eq!(
        home.json(&["api", "show", "internal"])["base_url"],
        server.uri()
    );

    home.opencli()
        .args(["internal", "things"])
        .assert()
        .success()
        .stdout("ID\n1\n");
}

#[test]
fn a_per_call_server_overrides_the_stored_base_url() {
    let home = Home::new();
    let file = home.spec("internal", RELATIVE);
    home.opencli()
        .args([
            "--server",
            "https://stored.example.com",
            "api",
            "add",
            "internal",
            "--spec",
        ])
        .arg(&file)
        .assert()
        .success();
    home.opencli()
        .args(["internal", "things", "--dry-run"])
        .assert()
        .stdout("GET https://stored.example.com/things\n");
    home.opencli()
        .args([
            "--server",
            "https://other.example.com",
            "internal",
            "things",
            "--dry-run",
        ])
        .assert()
        .stdout("GET https://other.example.com/things\n");
}

#[test]
fn specs_with_absolute_servers_follow_the_spec_unless_told_otherwise() {
    let home = Home::new();
    let file = home.spec("public", ABSOLUTE);
    home.opencli()
        .args(["api", "add", "public", "--spec"])
        .arg(&file)
        .assert()
        .success()
        .stdout(contains("calling https://prod.example.com/v1"));
    assert_eq!(
        home.json(&["api", "show", "public"])["base_url"],
        Value::Null
    );
    home.opencli()
        .args(["api", "show", "public"])
        .assert()
        .stdout(contains("Base URL:  follows the spec's servers"));
}

#[test]
fn set_server_changes_and_clears_the_base_url() {
    let home = Home::new();
    let file = home.spec("public", ABSOLUTE);
    home.opencli()
        .args(["api", "add", "public", "--spec"])
        .arg(&file)
        .assert()
        .success();

    home.opencli()
        .args([
            "api",
            "set-server",
            "public",
            "https://staging.example.com/v1/",
        ])
        .assert()
        .success()
        .stdout("`public` now calls https://staging.example.com/v1.\n");
    home.opencli()
        .args(["public", "things", "--dry-run"])
        .assert()
        .stdout("GET https://staging.example.com/v1/things\n");

    home.opencli()
        .args(["api", "set-server", "public", "--clear"])
        .assert()
        .success()
        .stdout(contains(
            "follows the spec's servers again: https://prod.example.com/v1",
        ));
    home.opencli()
        .args(["public", "things", "--dry-run"])
        .assert()
        .stdout("GET https://prod.example.com/v1/things\n");

    let events: Vec<String> = std::fs::read_to_string(home.0.path().join("audit.log"))
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(
        events
            .iter()
            .filter(|event| *event == "api.set-server")
            .count(),
        2
    );
}

#[test]
fn set_server_validates_its_input() {
    let home = Home::new();
    let file = home.spec("public", ABSOLUTE);
    home.opencli()
        .args(["api", "add", "public", "--spec"])
        .arg(&file)
        .assert()
        .success();
    home.opencli()
        .args(["api", "set-server", "public"])
        .assert()
        .code(2);
    home.opencli()
        .args([
            "api",
            "set-server",
            "public",
            "https://a.example.com",
            "--clear",
        ])
        .assert()
        .code(2);
    home.opencli()
        .args(["api", "set-server", "public", "ftp://files.example.com"])
        .assert()
        .code(1)
        .stderr(contains("not a usable server URL"));
    home.opencli()
        .args(["api", "set-server", "nope", "https://a.example.com"])
        .assert()
        .failure()
        .stderr(contains("no API named `nope`"));
}

#[test]
fn refresh_keeps_the_base_url() {
    let home = Home::new();
    let file = home.spec("internal", RELATIVE);
    home.opencli()
        .args([
            "--server",
            "https://internal.example.com",
            "api",
            "add",
            "internal",
            "--spec",
        ])
        .arg(&file)
        .assert()
        .success();
    std::fs::write(&file, RELATIVE.replace("version: '1'", "version: '2'")).unwrap();
    home.opencli()
        .args(["api", "refresh", "internal"])
        .assert()
        .success();
    assert_eq!(
        home.json(&["api", "show", "internal"])["base_url"],
        "https://internal.example.com"
    );
    home.opencli()
        .args(["internal", "things", "--dry-run"])
        .assert()
        .stdout("GET https://internal.example.com/things\n");
}
