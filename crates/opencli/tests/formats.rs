// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{body_string, body_string_contains, header, method, path, query_param};
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

    fn opencli(&self) -> Command {
        let mut command = cargo_bin_cmd!("opencli");
        command
            .env("OPENCLI_HOME", self.0.path())
            .env("OPENCLI_CREDENTIAL_STORE", "file")
            .env("OPENCLI_PASSPHRASE", "test-passphrase")
            .env("NO_COLOR", "1");
        command
    }

    fn add(&self, name: &str, spec: &Path) {
        self.opencli()
            .args(["api", "add", name, "--spec"])
            .arg(spec)
            .assert()
            .success();
    }

    fn add_text(&self, name: &str, text: &str) {
        let file = self.0.path().join(format!("{name}.yaml"));
        std::fs::write(&file, text).unwrap();
        self.add(name, &file);
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
fn openapi_31_specs_import_and_expose_commands() {
    let home = Home::new();
    home.add("pets31", &fixture("petstore-3.1.yaml"));
    assert_eq!(
        home.json(&["api", "show", "pets31"])["format"],
        "openapi-3.1"
    );
    home.opencli()
        .args(["pets31", "pets", "get", "7", "--dry-run"])
        .assert()
        .success()
        .stdout("GET https://eu.petstore.example.com/v1/pets/7\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn swagger_20_specs_send_authenticated_requests() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/pets"))
        .and(query_param("limit", "5"))
        .and(header("x-api-key", "k"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": "1", "name": "Rex"}])))
        .expect(1)
        .mount(&server)
        .await;
    let home = Home::new();
    home.add("pets20", &fixture("petstore-2.0.json"));
    assert_eq!(
        home.json(&["api", "show", "pets20"])["format"],
        "swagger-2.0"
    );
    home.opencli()
        .args([
            "--server",
            &server.uri(),
            "pets20",
            "pets",
            "list",
            "--limit",
            "5",
        ])
        .env("OPENCLI_PETS20_APIKEY", "k")
        .assert()
        .success()
        .stdout(contains("Rex"));
}

#[tokio::test(flavor = "multi_thread")]
async fn swagger_20_csv_arrays_are_sent_joined() {
    let server = MockServer::start().await;
    Mock::given(query_param("tags", "a,b"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([])))
        .expect(1)
        .mount(&server)
        .await;
    let home = Home::new();
    home.add_text(
        "csv",
        "swagger: '2.0'\ninfo: {title: Csv, version: '1'}\npaths:\n  /items:\n    get:\n      parameters:\n        - {name: tags, in: query, type: array, items: {type: string}}\n      responses: {'200': {description: ok}}\n",
    );
    home.opencli()
        .args([
            "--server",
            &server.uri(),
            "csv",
            "items",
            "list",
            "--tags",
            "a",
            "--tags",
            "b",
        ])
        .assert()
        .success()
        .stdout("No results.\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn form_bodies_are_url_encoded_from_flags() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/login"))
        .and(header("content-type", "application/x-www-form-urlencoded"))
        .and(body_string("user=me&remember=true"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let home = Home::new();
    home.add_text(
        "forms",
        "swagger: '2.0'\ninfo: {title: Forms, version: '1'}\npaths:\n  /login:\n    post:\n      consumes: [application/x-www-form-urlencoded]\n      parameters:\n        - {name: user, in: formData, type: string, required: true}\n        - {name: remember, in: formData, type: boolean}\n      responses: {'204': {description: ok}}\n",
    );
    home.opencli()
        .args([
            "--server",
            &server.uri(),
            "forms",
            "login",
            "create",
            "--user",
            "me",
            "--remember",
            "true",
            "--yes",
        ])
        .assert()
        .success();
}

#[tokio::test(flavor = "multi_thread")]
async fn multipart_uploads_send_files_from_flags() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/files"))
        .and(body_string_contains("filename=\"notes.txt\""))
        .and(body_string_contains("uploaded text"))
        .and(body_string_contains("name=\"label\"\r\n\r\nQ3"))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "f-1"})))
        .expect(1)
        .mount(&server)
        .await;
    let home = Home::new();
    home.add_text(
        "uploads",
        "swagger: '2.0'\ninfo: {title: Uploads, version: '1'}\npaths:\n  /files:\n    post:\n      parameters:\n        - {name: file, in: formData, type: file, required: true}\n        - {name: label, in: formData, type: string}\n      responses: {'201': {description: ok}}\n",
    );
    let upload = home.0.path().join("notes.txt");
    std::fs::write(&upload, "uploaded text").unwrap();
    let upload = upload.display().to_string();

    home.opencli()
        .args([
            "uploads",
            "files",
            "create",
            "--file",
            &upload,
            "--label",
            "Q3",
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(
            contains("Content-Type: multipart/form-data")
                .and(contains(format!("Upload: file ← {upload}"))),
        );
    home.opencli()
        .args([
            "--server",
            &server.uri(),
            "uploads",
            "files",
            "create",
            "--file",
            &upload,
            "--label",
            "Q3",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(contains("f-1"));
    home.opencli()
        .args(["uploads", "files", "create", "--help"])
        .assert()
        .success()
        .stdout(contains("--file <FILE>"));
}

#[test]
fn unsupported_versions_name_the_supported_ones() {
    let home = Home::new();
    let file = home.0.path().join("future.yaml");
    std::fs::write(
        &file,
        "openapi: 3.2.0\ninfo: {title: F, version: '1'}\npaths: {}\n",
    )
    .unwrap();
    home.opencli()
        .args(["api", "add", "future", "--spec"])
        .arg(&file)
        .assert()
        .failure()
        .stderr(contains("OpenAPI 3.2.0").and(contains("3.0.x and 3.1.x, and Swagger 2.0")));
}
