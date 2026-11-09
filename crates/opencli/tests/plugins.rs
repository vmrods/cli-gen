// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::path;
// Only the Unix-only provider tests match on headers and query parameters.
#[cfg(unix)]
use wiremock::matchers::{header, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// Petstore registered, plus a directory for scripts that is put on PATH.
struct Home {
    directory: TempDir,
    bin: TempDir,
}

impl Home {
    fn new() -> Self {
        let home = Self {
            directory: TempDir::new().unwrap(),
            bin: TempDir::new().unwrap(),
        };
        home.opencli()
            .args(["api", "add", "petstore", "--spec"])
            .arg(fixture("petstore.yaml"))
            .assert()
            .success();
        home
    }

    fn opencli(&self) -> Command {
        let mut command = cargo_bin_cmd!("opencli");
        let path: OsString =
            std::env::join_paths(std::iter::once(self.bin.path().to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ))
            .unwrap();
        command
            .env("OPENCLI_HOME", self.directory.path())
            .env("OPENCLI_CREDENTIAL_STORE", "file")
            .env("OPENCLI_PASSPHRASE", "test-passphrase")
            .env("PATH", path)
            .env("NO_COLOR", "1")
            .env_remove("OPENCLI_PETSTORE_APIKEY");
        command
    }

    fn install(&self, manifest: &str) {
        let file = self.directory.path().join("plugin.toml");
        std::fs::write(&file, manifest).unwrap();
        self.opencli()
            .args(["plugin", "install", "--yes"])
            .arg(&file)
            .assert()
            .success();
    }

    #[cfg(unix)]
    // Written by `install`, not std::fs::write: on Linux, a process another test
    // thread forks while this one holds the file open for writing inherits that
    // handle, and running the script then fails with "Text file busy"
    // (ETXTBSY). Only the short-lived `install` process ever writes the file.
    fn script(&self, name: &str, body: &str) -> PathBuf {
        let source = self.directory.path().join(format!("{name}.sh"));
        std::fs::write(&source, format!("#!/bin/sh\n{body}\n")).unwrap();
        let file = self.bin.path().join(name);
        let status = std::process::Command::new("install")
            .args(["-m", "755"])
            .args([&source, &file])
            .status()
            .unwrap();
        assert!(status.success(), "install failed for {}", file.display());
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

const HELPERS: &str = r#"
name = "pet-helpers"
version = "1.2.0"
description = "Shortcuts for the pet store"
api = "petstore"

[aliases.few]
command = "pets"
args = ["--limit", "3"]
description = "First three pets"

[aliases.pets]
command = "pets"

[formats."pets"]
columns = [{ path = "name", header = "PET" }, "owner.name"]
"#;

#[test]
fn install_shows_what_the_plugin_does_and_needs_consent() {
    let home = Home::new();
    let file = home.directory.path().join("plugin.toml");
    std::fs::write(&file, HELPERS).unwrap();
    home.opencli()
        .args(["plugin", "install"])
        .arg(&file)
        .assert()
        .code(1)
        .stderr(
            contains("Plugin `pet-helpers` 1.2.0 for API `petstore`")
                .and(contains("Alias:   few → pets --limit 3"))
                .and(contains("Format:  pets"))
                .and(contains("Runs no programs."))
                .and(contains("--yes")),
        );
    home.opencli()
        .args(["plugin", "install", "--yes"])
        .arg(&file)
        .assert()
        .success()
        .stdout("Installed plugin `pet-helpers` 1.2.0 for `petstore`.\n");
    let listing = home.json(&["plugin", "list"]);
    assert_eq!(listing["manifests"][0]["name"], "pet-helpers");
    assert_eq!(listing["manifests"][0]["aliases"], json!(["few", "pets"]));
}

#[test]
fn install_rejects_invalid_manifests() {
    let home = Home::new();
    let file = home.directory.path().join("bad.toml");
    std::fs::write(
        &file,
        "name = \"Bad Name\"\nversion = \"1\"\napi = \"petstore\"\n",
    )
    .unwrap();
    home.opencli()
        .args(["plugin", "install", "--yes"])
        .arg(&file)
        .assert()
        .failure()
        .stderr(contains("invalid plugin manifest").and(contains("kebab-case")));
}

#[test]
fn aliases_expand_and_appear_in_help_but_never_shadow_commands() {
    let home = Home::new();
    home.install(HELPERS);
    home.opencli()
        .args(["petstore", "few", "--dry-run"])
        .assert()
        .success()
        .stdout("GET https://eu.petstore.example.com/v1/pets?limit=3\n");
    home.opencli()
        .args(["petstore", "--help"])
        .assert()
        .success()
        .stdout(
            contains("Aliases from plugins:")
                .and(contains("few  →  pets --limit 3  (First three pets)")),
        );
    // `pets` is a real command group, so the alias with that name is ignored.
    home.opencli()
        .args(["petstore", "pets", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("/pets\n"));
}

#[rstest::rstest]
#[case(false)]
#[case(true)]
#[tokio::test(flavor = "multi_thread")]
async fn formats_choose_table_columns_for_a_command(#[case] envelope: bool) {
    let server = MockServer::start().await;
    let items = json!([
        {"id": "1", "name": "Rex", "owner": {"name": "Ana"}},
        {"id": "2", "name": "Tom"}
    ]);
    let response = if envelope {
        json!({"data": items})
    } else {
        items
    };
    Mock::given(path("/pets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&server)
        .await;
    let home = Home::new();
    home.install(HELPERS);
    let mut command = home.opencli();
    if envelope {
        command.args(["--query", "data"]);
    }
    command
        .args(["--server", &server.uri(), "petstore", "pets"])
        .env("OPENCLI_PETSTORE_APIKEY", "k")
        .assert()
        .success()
        .stdout("PET  NAME\nRex  Ana\nTom\n");
}

#[test]
fn remove_deletes_the_plugin_and_its_aliases() {
    let home = Home::new();
    home.install(HELPERS);
    home.opencli()
        .args(["plugin", "remove", "pet-helpers"])
        .assert()
        .code(1)
        .stderr(contains("--yes"));
    home.opencli()
        .args(["plugin", "remove", "pet-helpers", "--yes"])
        .assert()
        .success();
    home.opencli()
        .args(["petstore", "few", "--dry-run"])
        .assert()
        .code(2);
    home.opencli()
        .args(["plugin", "remove", "pet-helpers", "--yes"])
        .assert()
        .failure()
        .stderr(contains("no plugin named `pet-helpers`"));
    let events: Vec<String> = std::fs::read_to_string(home.directory.path().join("audit.log"))
        .unwrap()
        .lines()
        .map(|line| {
            serde_json::from_str::<Value>(line).unwrap()["event"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert!(
        events.ends_with(&["plugin.install".to_owned(), "plugin.remove".to_owned()]),
        "{events:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn install_from_url() {
    let server = MockServer::start().await;
    Mock::given(path("/plugin.toml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(HELPERS))
        .mount(&server)
        .await;
    let home = Home::new();
    home.opencli()
        .args([
            "plugin",
            "install",
            "--yes",
            &format!("{}/plugin.toml", server.uri()),
        ])
        .assert()
        .success();
    home.opencli()
        .args(["petstore", "few", "--dry-run"])
        .assert()
        .success();
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn credential_providers_supply_secrets_after_the_environment() {
    let server = MockServer::start().await;
    Mock::given(path("/pets"))
        .and(query_param("limit", "1"))
        .and(header("x-api-key", "from-plugin"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/pets"))
        .and(query_param("limit", "2"))
        .and(header("x-api-key", "from-env"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    let home = Home::new();
    home.script("pet-token", r#"printf '{"value": "from-plugin"}'"#);
    home.install("name = \"pet-auth\"\nversion = \"1\"\napi = \"petstore\"\n[credentials.apiKey]\nexec = \"pet-token\"\n");

    home.opencli()
        .args([
            "--server",
            &server.uri(),
            "petstore",
            "pets",
            "--limit",
            "1",
        ])
        .assert()
        .success();
    home.opencli()
        .args([
            "--server",
            &server.uri(),
            "petstore",
            "pets",
            "--limit",
            "2",
        ])
        .env("OPENCLI_PETSTORE_APIKEY", "from-env")
        .assert()
        .success();
    let status = home.json(&["auth", "status", "petstore"]);
    let api_key = status
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["scheme"] == "apiKey")
        .unwrap();
    assert_eq!(api_key["configured"], "plugin pet-auth");
}

#[cfg(unix)]
#[test]
fn credential_provider_failures_stop_the_request() {
    let home = Home::new();
    home.script("pet-token", "echo 'not signed in to the vault' >&2; exit 4");
    home.install("name = \"pet-auth\"\nversion = \"1\"\napi = \"petstore\"\n[credentials.apiKey]\nexec = \"pet-token\"\n");
    home.opencli()
        .args([
            "--server",
            "https://unused.example.invalid",
            "petstore",
            "pets",
        ])
        .assert()
        .code(1)
        // miette wraps long messages, so match their parts separately.
        .stderr(
            contains("credential provider `pet-token` failed with exit code 4")
                .and(contains("not signed in to")),
        );
}

#[cfg(unix)]
#[test]
fn install_lists_programs_the_plugin_will_run() {
    let home = Home::new();
    let file = home.directory.path().join("plugin.toml");
    std::fs::write(&file, "name = \"pet-auth\"\nversion = \"1\"\napi = \"petstore\"\n[credentials.apiKey]\nexec = \"pet-token\"\nargs = [\"--vault\", \"work\"]\n").unwrap();
    home.opencli()
        .args(["plugin", "install", "--yes"])
        .arg(&file)
        .assert()
        .success()
        .stderr(contains(
            "Runs:    pet-token --vault work  (to supply credentials for `apiKey`)",
        ));
}

#[cfg(unix)]
#[test]
fn executable_plugins_run_with_arguments_and_exit_codes() {
    let home = Home::new();
    home.script(
        "opencli-hello",
        r#"echo "hello $*"; test -x "$OPENCLI_BINARY" && echo "binary ok"; exit 7"#,
    );
    home.opencli()
        .args(["hello", "world", "--loud"])
        .assert()
        .code(7)
        .stdout("hello world --loud\nbinary ok\n")
        .stderr("");
    let listing = home.json(&["plugin", "list"]);
    assert_eq!(listing["executables"][0]["name"], "hello");
}

#[cfg(unix)]
#[test]
fn registered_apis_win_over_executable_plugins() {
    let home = Home::new();
    home.script("opencli-petstore", "echo hijacked");
    home.opencli()
        .args(["petstore", "pets", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("GET https://eu.petstore.example.com/v1/pets"));
}

#[test]
fn unknown_names_are_still_usage_errors() {
    Home::new()
        .opencli()
        .args(["nothing-here"])
        .assert()
        .code(2)
        .stderr(contains("unrecognized subcommand"));
}
