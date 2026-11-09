// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const PETSTORE: &str = include_str!("../../../fixtures/petstore.yaml");

/// Petstore 1.1.0: adds search and a required --tenant on list, drops
/// DELETE /pets/{petId} and the basic scheme.
fn petstore_v2() -> String {
    PETSTORE
        .replace("version: 1.0.0", "version: 1.1.0")
        .replace(
            "        - $ref: '#/components/parameters/Limit'\n",
            "        - $ref: '#/components/parameters/Limit'\n        - {name: tenant, in: query, required: true, schema: {type: string}}\n",
        )
        .replace(
            "  /pets/{petId}:\n",
            "  /pets/search:\n    get:\n      operationId: searchPets\n      x-cli-name: search\n      x-cli-resource: pets\n      responses:\n        '200':\n          description: ok\n  /pets/{petId}:\n",
        )
        .replace("    delete:\n      operationId: deletePet\n      deprecated: true\n      security: []\n      responses:\n        '204':\n          description: Deleted\n", "")
        .replace("    basic:\n      type: http\n      scheme: basic\n", "")
}

/// Petstore registered from a spec file the test can rewrite.
struct Home {
    directory: TempDir,
}

impl Home {
    fn new() -> Self {
        let home = Self {
            directory: TempDir::new().unwrap(),
        };
        home.write_spec(PETSTORE);
        home.opencli()
            .args(["api", "add", "petstore", "--spec"])
            .arg(home.spec())
            .assert()
            .success();
        home
    }

    fn spec(&self) -> PathBuf {
        self.directory.path().join("petstore.yaml")
    }

    fn write_spec(&self, text: &str) {
        std::fs::write(self.spec(), text).unwrap();
    }

    fn opencli(&self) -> Command {
        let mut command = cargo_bin_cmd!("opencli");
        command
            .env("OPENCLI_HOME", self.directory.path())
            .env("OPENCLI_CREDENTIAL_STORE", "file")
            .env("OPENCLI_PASSPHRASE", "test-passphrase")
            .env("NO_COLOR", "1");
        command
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

    fn current_version(&self) -> Value {
        let versions = self.json(&["api", "versions", "petstore"]);
        versions
            .as_array()
            .unwrap()
            .iter()
            .find(|version| version["current"] == true)
            .unwrap()["number"]
            .clone()
    }
}

#[test]
fn refresh_with_unchanged_spec_is_a_no_op() {
    let home = Home::new();
    home.opencli()
        .args(["api", "refresh", "petstore"])
        .assert()
        .success()
        .stdout("`petstore` is already up to date.\n");
    assert_eq!(
        home.json(&["api", "versions", "petstore"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn refresh_records_a_new_version_and_reports_the_diff() {
    let home = Home::new();
    home.write_spec(&petstore_v2());
    home.opencli()
        .args(["api", "refresh", "petstore"])
        .assert()
        .success()
        .stdout(
            contains("Refreshed `petstore` to version 2: Petstore 1.1.0, 4 operations.")
                .and(contains("added     pets search  GET /pets/search"))
                .and(contains("removed   pets delete  DELETE /pets/{petId}"))
                .and(contains(
                    "changed!  pets         GET /pets: new required argument --tenant",
                ))
                .and(contains("Security schemes removed: basic."))
                .and(contains("Breaking:")),
        );
    assert_eq!(home.current_version(), 2);
    home.opencli()
        .args(["petstore", "pets", "search", "--help"])
        .assert()
        .success();
    home.opencli()
        .args(["petstore", "pets", "delete", "--pet-id", "1"])
        .assert()
        .code(2);
}

#[test]
fn refresh_json_output_carries_the_structured_diff() {
    let home = Home::new();
    home.write_spec(&petstore_v2());
    let result = home.json(&["api", "refresh", "petstore"]);
    assert_eq!(result["outcome"], "added");
    assert_eq!(result["number"], 2);
    assert_eq!(result["breaking"], true);
    assert_eq!(
        result["diff"]["added"],
        json!([{"command": "pets search", "operation": "GET /pets/search"}])
    );
    assert_eq!(result["diff"]["schemes_removed"], json!(["basic"]));
}

#[test]
fn refresh_dry_run_saves_nothing() {
    let home = Home::new();
    home.write_spec(&petstore_v2());
    home.opencli()
        .args(["api", "refresh", "petstore", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("Dry run, nothing saved.").and(contains("pets search")));
    assert_eq!(
        home.json(&["api", "versions", "petstore"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
    home.opencli()
        .args(["petstore", "pets", "search", "--help"])
        .assert()
        .code(2);
}

#[test]
fn refresh_back_to_earlier_content_reactivates_that_version() {
    let home = Home::new();
    home.write_spec(&petstore_v2());
    home.opencli()
        .args(["api", "refresh", "petstore"])
        .assert()
        .success();
    home.write_spec(PETSTORE);
    home.opencli()
        .args(["api", "refresh", "petstore"])
        .assert()
        .success()
        .stdout(
            contains("Reactivated version 1 of `petstore`").and(contains("added     pets delete")),
        );
    assert_eq!(home.current_version(), 1);
    assert_eq!(
        home.json(&["api", "versions", "petstore"])
            .as_array()
            .unwrap()
            .len(),
        2
    );
    home.opencli()
        .args(["petstore", "pets", "delete", "--help"])
        .assert()
        .success();
}

#[test]
fn refresh_from_a_new_source_replaces_the_stored_source() {
    let home = Home::new();
    let moved = home.directory.path().join("moved.yaml");
    std::fs::write(&moved, petstore_v2()).unwrap();
    home.opencli()
        .args(["api", "refresh", "petstore", "--spec"])
        .arg(&moved)
        .assert()
        .success();
    let source = home.json(&["api", "show", "petstore"])["source"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(source.ends_with("moved.yaml"), "{source}");
}

#[test]
fn refresh_keeps_credentials_for_schemes_that_remain() {
    let home = Home::new();
    home.opencli()
        .args([
            "auth",
            "login",
            "petstore",
            "--scheme",
            "apiKey",
            "--with-token",
        ])
        .write_stdin("k-1")
        .assert()
        .success();
    home.opencli()
        .args([
            "auth",
            "login",
            "petstore",
            "--scheme",
            "basic",
            "--username",
            "me",
            "--with-token",
        ])
        .write_stdin("pw")
        .assert()
        .success();
    home.write_spec(&petstore_v2());
    home.opencli()
        .args(["api", "refresh", "petstore"])
        .assert()
        .success()
        .stdout(contains("Removed stored credentials for: basic."));

    let status = home.json(&["auth", "status", "petstore"]);
    let api_key = status
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["scheme"] == "apiKey")
        .unwrap();
    assert_eq!(api_key["configured"], "encrypted file");

    // Going back to v1 brings the basic scheme back, but not its deleted credentials.
    home.write_spec(PETSTORE);
    home.opencli()
        .args(["api", "refresh", "petstore"])
        .assert()
        .success();
    let status = home.json(&["auth", "status", "petstore"]);
    let basic = status
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["scheme"] == "basic")
        .unwrap();
    assert_eq!(basic["configured"], Value::Null);
}

#[tokio::test(flavor = "multi_thread")]
async fn refreshed_specs_keep_working_with_stored_credentials() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/pets/search"))
        .and(header("x-api-key", "k-1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": "1"}])))
        .expect(1)
        .mount(&server)
        .await;
    let home = Home::new();
    home.opencli()
        .args([
            "auth",
            "login",
            "petstore",
            "--scheme",
            "apiKey",
            "--with-token",
        ])
        .write_stdin("k-1")
        .assert()
        .success();
    home.write_spec(&petstore_v2());
    home.opencli()
        .args(["api", "refresh", "petstore"])
        .assert()
        .success();
    home.opencli()
        .args(["--server", &server.uri(), "petstore", "pets", "search"])
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread")]
async fn refresh_refetches_url_sources() {
    let server = MockServer::start().await;
    Mock::given(path("/openapi.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(PETSTORE))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/openapi.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(petstore_v2()))
        .mount(&server)
        .await;
    let home = Home::new();
    let url = format!("{}/openapi.yaml", server.uri());
    home.opencli()
        .args(["api", "add", "remote", "--spec", &url])
        .assert()
        .success();
    home.opencli()
        .args(["api", "refresh", "remote"])
        .assert()
        .success()
        .stdout(contains("to version 2"));
}

#[test]
fn refresh_rejects_invalid_or_conflicting_specs_and_keeps_the_current_version() {
    let home = Home::new();
    home.write_spec(&PETSTORE.replace(
        "  /pets/{petId}:\n    parameters:",
        "  /pets/{id}:\n    parameters:",
    ));
    home.opencli()
        .args(["api", "refresh", "petstore"])
        .assert()
        .failure()
        .stderr(contains("failed validation"));

    // Two hinted operations with the same name can't be resolved automatically.
    let hint = "\n      x-cli-name: same\n      x-cli-resource: pets\n";
    home.write_spec(
        &PETSTORE
            .replace(
                "operationId: showPetById\n",
                &format!("operationId: showPetById{hint}"),
            )
            .replace(
                "operationId: createPet\n",
                &format!("operationId: createPet{hint}"),
            ),
    );
    home.opencli()
        .args(["api", "refresh", "petstore"])
        .assert()
        .failure()
        .stderr(contains("don't map to unique command names"));

    assert_eq!(
        home.json(&["api", "versions", "petstore"])
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn refresh_reports_unknown_api() {
    Home::new()
        .opencli()
        .args(["api", "refresh", "nope"])
        .assert()
        .failure()
        .stderr(contains("no API named `nope`"));
}

#[test]
fn refresh_reports_a_missing_source_file() {
    let home = Home::new();
    std::fs::remove_file(home.spec()).unwrap();
    home.opencli()
        .args(["api", "refresh", "petstore"])
        .assert()
        .failure()
        .stderr(contains("could not read"));
}
