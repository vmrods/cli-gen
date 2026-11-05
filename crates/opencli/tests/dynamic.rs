// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::{contains, is_match};
use serde_json::{Value, json};
use tempfile::TempDir;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// An OpenCLI home with the Petstore API registered.
struct Petstore(TempDir);

impl Petstore {
    fn new() -> Self {
        let home = Self(TempDir::new().unwrap());
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
            .env("NO_COLOR", "1");
        command
    }

    fn json(&self, args: &[&str]) -> Value {
        let output = self.opencli().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn cache_file(&self) -> PathBuf {
        self.0.path().join("cache/petstore.json")
    }
}

#[test]
fn root_help_lists_registered_apis() {
    Petstore::new()
        .opencli()
        .arg("--help")
        .assert()
        .success()
        .stdout(is_match(r"petstore\s+Petstore 1\.0\.0").unwrap());
}

#[test]
fn resource_help_lists_its_commands() {
    Petstore::new()
        .opencli()
        .args(["petstore", "pets", "--help"])
        .assert()
        .success()
        .stdout(
            contains("create  Create a pet")
                .and(contains("delete  [deprecated]"))
                .and(contains("get     Info for a specific pet"))
                .and(contains("list    List all pets")),
        );
}

#[test]
fn operation_help_shows_arguments_and_endpoint() {
    Petstore::new()
        .opencli()
        .args(["petstore", "pets", "get", "--help"])
        .assert()
        .success()
        .stdout(
            contains("Usage: opencli petstore pets get [OPTIONS] <PET_ID>")
                .and(contains("Path parameter `petId`"))
                .and(contains("Calls GET /pets/{petId}")),
        );
}

#[test]
fn help_subcommand_reaches_operation_help() {
    Petstore::new()
        .opencli()
        .args(["help", "petstore", "pets", "list"])
        .assert()
        .success()
        .stdout(contains("Maximum number of pets to return"));
}

#[test]
fn dry_run_substitutes_and_encodes_path_parameters() {
    Petstore::new()
        .opencli()
        .args(["petstore", "pets", "get", "a b/c", "--dry-run"])
        .assert()
        .success()
        .stdout("GET https://eu.petstore.example.com/v1/pets/a%20b%2Fc\n");
}

#[test]
fn dry_run_outputs_json_plan_with_query() {
    let plan = Petstore::new().json(&[
        "-o",
        "json",
        "petstore",
        "pets",
        "list",
        "--limit",
        "5",
        "--dry-run",
    ]);
    assert_eq!(plan["method"], "GET");
    assert_eq!(plan["url"], "https://eu.petstore.example.com/v1/pets");
    assert_eq!(plan["query"], json!([["limit", "5"]]));
}

#[test]
fn dry_run_builds_body_from_field_flags() {
    let plan = Petstore::new().json(&[
        "petstore",
        "pets",
        "create",
        "--name",
        "Rex",
        "--tag",
        "dog",
        "--dry-run",
        "-o",
        "json",
    ]);
    assert_eq!(plan["body"]["content_type"], "application/json");
    assert_eq!(plan["body"]["value"], json!({"name": "Rex", "tag": "dog"}));
}

#[test]
fn dry_run_reads_body_inline_from_file_and_stdin() {
    let home = Petstore::new();
    let file = home.0.path().join("pet.json");
    std::fs::write(&file, r#"{"name": "FromFile"}"#).unwrap();

    let inline = home.json(&[
        "petstore",
        "pets",
        "create",
        "--body",
        r#"{"name":"Inline"}"#,
        "--dry-run",
        "-o",
        "json",
    ]);
    assert_eq!(inline["body"]["value"], json!({"name": "Inline"}));

    let from_file = home.json(&[
        "petstore",
        "pets",
        "create",
        "--body",
        &format!("@{}", file.display()),
        "--dry-run",
        "-o",
        "json",
    ]);
    assert_eq!(from_file["body"]["value"], json!({"name": "FromFile"}));

    let output = home
        .opencli()
        .args([
            "petstore",
            "pets",
            "create",
            "--body",
            "-",
            "--tag",
            "cat",
            "--dry-run",
            "-o",
            "json",
        ])
        .write_stdin(r#"{"name": "FromStdin", "tag": "dog"}"#)
        .output()
        .unwrap();
    let from_stdin: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        from_stdin["body"]["value"],
        json!({"name": "FromStdin", "tag": "cat"})
    );
}

#[test]
fn missing_required_body_is_reported() {
    Petstore::new()
        .opencli()
        .args(["petstore", "pets", "create", "--dry-run"])
        .assert()
        .failure()
        .stderr(contains("requires a request body"));
}

#[test]
fn invalid_body_json_is_reported_with_help() {
    Petstore::new()
        .opencli()
        .args(["petstore", "pets", "create", "--body", "{nope", "--dry-run"])
        .assert()
        .failure()
        .stderr(contains("not valid JSON").and(contains("@file.json")));
}

#[test]
fn typed_flags_reject_invalid_values_as_usage_errors() {
    Petstore::new()
        .opencli()
        .args(["petstore", "pets", "list", "--limit", "many"])
        .assert()
        .code(2)
        .stderr(contains("expected integer"));
}

#[test]
fn missing_positional_is_a_usage_error() {
    Petstore::new()
        .opencli()
        .args(["petstore", "pets", "get"])
        .assert()
        .code(2)
        .stderr(contains("<PET_ID>"));
}

#[test]
fn describe_prints_machine_readable_contract_without_required_arguments() {
    let description = Petstore::new().json(&["petstore", "pets", "create", "--describe"]);
    assert_eq!(description["command"], "opencli petstore pets create");
    assert_eq!(description["method"], "POST");
    assert_eq!(description["operation_id"], "createPet");
    assert_eq!(description["arguments"][0]["usage"], "--name");
    assert_eq!(
        description["arguments"][0]["target"],
        json!({"in": "body-field", "name": "name"})
    );
    assert_eq!(
        description["body"]["schema"]["properties"]["name"],
        json!({"type": "string"}),
        "body schema should have its $ref inlined"
    );
    let get = Petstore::new().json(&["petstore", "pets", "get", "--describe"]);
    assert_eq!(get["arguments"][0]["usage"], "<PET_ID>");
}

#[test]
fn unknown_api_is_a_usage_error() {
    Petstore::new()
        .opencli()
        .args(["nope", "list"])
        .assert()
        .code(2)
        .stderr(contains("unrecognized subcommand 'nope'"));
}

#[test]
fn add_writes_cache_and_remove_deletes_it() {
    let home = Petstore::new();
    assert!(home.cache_file().exists());
    home.opencli()
        .args(["api", "remove", "petstore", "--yes"])
        .assert()
        .success();
    assert!(!home.cache_file().exists());
    home.opencli()
        .args(["petstore", "pets", "list"])
        .assert()
        .code(2);
}

#[test]
fn missing_or_corrupt_cache_is_rebuilt_from_store() {
    let home = Petstore::new();
    std::fs::remove_file(home.cache_file()).unwrap();
    home.opencli()
        .args(["petstore", "pets", "list", "--dry-run"])
        .assert()
        .success();
    assert!(home.cache_file().exists());

    std::fs::write(home.cache_file(), "not json").unwrap();
    home.opencli()
        .args(["petstore", "pets", "list", "--dry-run"])
        .assert()
        .success();
    let rebuilt: Value =
        serde_json::from_slice(&std::fs::read(home.cache_file()).unwrap()).unwrap();
    assert_eq!(rebuilt["api"], "petstore");
}

#[test]
fn add_rejects_specs_whose_commands_collide() {
    let home = TempDir::new().unwrap();
    let spec = home.path().join("clash.yaml");
    std::fs::write(
        &spec,
        "openapi: 3.0.3\ninfo: {title: Clash, version: '1'}\npaths:\n  /users:\n    delete:\n      responses: {'204': {description: ok}}\n  /users/{id}:\n    delete:\n      parameters: [{name: id, in: path, required: true, schema: {type: string}}]\n      responses: {'204': {description: ok}}\n",
    )
    .unwrap();
    let opencli = || {
        let mut command = cargo_bin_cmd!("opencli");
        command
            .env("OPENCLI_HOME", home.path())
            .env("OPENCLI_CREDENTIAL_STORE", "file")
            .env("OPENCLI_PASSPHRASE", "test-passphrase")
            .env("NO_COLOR", "1");
        command
    };
    opencli()
        .args(["api", "add", "clash", "--spec"])
        .arg(&spec)
        .assert()
        .failure()
        .stderr(
            contains("don't map to unique command names")
                .and(contains(
                    "`users delete`: DELETE /users, DELETE /users/{id}",
                ))
                .and(contains("x-cli-name")),
        );
    let output = opencli()
        .args(["api", "list", "-o", "json"])
        .output()
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!([])
    );
}
