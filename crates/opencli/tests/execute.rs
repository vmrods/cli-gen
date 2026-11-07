// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use serde_json::{Value, json};
use tempfile::TempDir;
use wiremock::matchers::{body_json, header, header_regex, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures")
        .join(name)
}

/// Petstore registered from a file, with requests pointed at a mock server.
struct Harness {
    home: TempDir,
    server: MockServer,
}

impl Harness {
    async fn new() -> Self {
        let harness = Self {
            home: TempDir::new().unwrap(),
            server: MockServer::start().await,
        };
        harness
            .bare()
            .args(["api", "add", "petstore", "--spec"])
            .arg(fixture("petstore.yaml"))
            .assert()
            .success();
        harness
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

    /// `opencli --server <mock> petstore …`, with Petstore's credentials in the environment.
    fn petstore(&self, args: &[&str]) -> Command {
        let mut command = self.bare();
        command
            .env("OPENCLI_PETSTORE_APIKEY", "test-key")
            .env("OPENCLI_PETSTORE_OAUTH", "test-token");
        command
            .args(["--server", &self.server.uri(), "petstore"])
            .args(args);
        command
    }
}

fn pets() -> Value {
    json!([{"id": "1", "name": "Rex", "tag": "dog"}, {"id": "2", "name": "Tom"}])
}

#[tokio::test(flavor = "multi_thread")]
async fn get_list_renders_table_from_json_array() {
    let harness = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/pets"))
        .and(query_param("limit", "2"))
        .and(header_regex("user-agent", "^opencli/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pets()))
        .expect(1)
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "list", "--limit", "2"])
        .assert()
        .success()
        .stdout("ID  NAME  TAG\n1   Rex   dog\n2   Tom\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn get_outputs_raw_json_with_output_json() {
    let harness = Harness::new().await;
    Mock::given(path("/pets/1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "1", "name": "Rex"})))
        .mount(&harness.server)
        .await;
    let output = harness
        .petstore(&["pets", "get", "1", "-o", "json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!({"id": "1", "name": "Rex"})
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn post_with_yes_sends_json_body() {
    let harness = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/pets"))
        .and(header("content-type", "application/json"))
        .and(body_json(json!({"name": "Rex", "tag": "dog"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "3", "name": "Rex"})))
        .expect(1)
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "create", "--name", "Rex", "--tag", "dog", "--yes"])
        .assert()
        .success()
        .stdout(contains("id     3"));
}

#[tokio::test(flavor = "multi_thread")]
async fn changing_requests_need_yes_when_not_interactive() {
    let harness = Harness::new().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "create", "--name", "Rex"])
        .assert()
        .code(1)
        .stderr(contains("needs confirmation").and(contains("--yes")));
}

#[tokio::test(flavor = "multi_thread")]
async fn get_commands_have_no_yes_flag() {
    let harness = Harness::new().await;
    harness
        .petstore(&["pets", "list", "--yes"])
        .assert()
        .code(2);
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_with_empty_response_reports_status() {
    let harness = Harness::new().await;
    Mock::given(method("DELETE"))
        .and(path("/pets/1"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "delete", "1", "--yes"])
        .assert()
        .success()
        .stdout("HTTP 204 No Content\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn client_errors_print_body_and_exit_3() {
    let harness = Harness::new().await;
    Mock::given(path("/pets/404"))
        .respond_with(ResponseTemplate::new(404).set_body_raw(
            r#"{"title":"Not Found","status":404}"#,
            "application/problem+json",
        ))
        .mount(&harness.server)
        .await;
    let output = harness
        .petstore(&["pets", "get", "404", "-o", "json"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["title"],
        "Not Found"
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("returned HTTP 404 Not Found"));
}

#[tokio::test(flavor = "multi_thread")]
async fn server_errors_exit_4() {
    let harness = Harness::new().await;
    Mock::given(path("/pets"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "list"])
        .assert()
        .code(4)
        .stderr(contains("HTTP 503"));
}

#[tokio::test(flavor = "multi_thread")]
async fn slow_responses_time_out_with_exit_5() {
    let harness = Harness::new().await;
    Mock::given(path("/pets"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "list", "--timeout", "1"])
        .assert()
        .code(5)
        .stderr(contains("timed out").and(contains("--timeout")));
}

#[tokio::test(flavor = "multi_thread")]
async fn unreachable_servers_exit_5() {
    let harness = Harness::new().await;
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    harness
        .bare()
        .env("OPENCLI_PETSTORE_APIKEY", "test-key")
        .args([
            "--server",
            &format!("http://127.0.0.1:{port}"),
            "petstore",
            "pets",
            "list",
        ])
        .assert()
        .code(5)
        .stderr(contains("could not connect"));
}

#[tokio::test(flavor = "multi_thread")]
async fn text_responses_are_printed_as_is() {
    let harness = Harness::new().await;
    Mock::given(path("/pets"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("plain listing", "text/plain"))
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "list"])
        .assert()
        .success()
        .stdout("plain listing\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_server_override_is_rejected() {
    let harness = Harness::new().await;
    harness
        .bare()
        .args([
            "--server",
            "ftp://files.example.com",
            "petstore",
            "pets",
            "list",
        ])
        .assert()
        .code(1)
        .stderr(contains("not a usable server URL"));
}

#[tokio::test(flavor = "multi_thread")]
async fn relative_servers_resolve_against_the_spec_url() {
    let server = MockServer::start().await;
    let spec = "openapi: 3.0.3\ninfo: {title: Rel, version: '1'}\nservers: [{url: /v1}]\npaths:\n  /things:\n    get:\n      responses: {'200': {description: ok}}\n";
    Mock::given(path("/specs/openapi.yaml"))
        .respond_with(ResponseTemplate::new(200).set_body_string(spec))
        .mount(&server)
        .await;
    Mock::given(path("/v1/things"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([{"id": 1}])))
        .expect(1)
        .mount(&server)
        .await;
    let home = TempDir::new().unwrap();
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
        .args([
            "api",
            "add",
            "rel",
            "--spec",
            &format!("{}/specs/openapi.yaml", server.uri()),
        ])
        .assert()
        .success();
    opencli()
        .args(["rel", "things", "list"])
        .assert()
        .success()
        .stdout("ID\n1\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn query_data_renders_enveloped_items_as_table() {
    let harness = Harness::new().await;
    Mock::given(path("/pets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": pets(), "message": "Items found successfully"
        })))
        .expect(1)
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "list", "--query", "data", "-o", "table"])
        .assert()
        .success()
        .stdout("ID  NAME  TAG\n1   Rex   dog\n2   Tom\n");
}

#[tokio::test(flavor = "multi_thread")]
async fn query_before_api_selects_nested_json_value() {
    let harness = Harness::new().await;
    Mock::given(path("/pets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"items": pets()}})))
        .mount(&harness.server)
        .await;
    let output = harness
        .bare()
        .env("OPENCLI_PETSTORE_APIKEY", "test-key")
        .args([
            "--query",
            "data.items.0.name",
            "--server",
            &harness.server.uri(),
            "petstore",
            "pets",
            "list",
            "--output",
            "json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap(),
        json!("Rex")
    );
}

#[rstest::rstest]
#[case(ResponseTemplate::new(200).set_body_json(json!({"other": []})), "was not found")]
#[case(ResponseTemplate::new(200).set_body_string("plain text"), "not JSON")]
#[tokio::test(flavor = "multi_thread")]
async fn query_reports_unselectable_responses(
    #[case] response: ResponseTemplate,
    #[case] error: &str,
) {
    let harness = Harness::new().await;
    Mock::given(path("/pets"))
        .respond_with(response)
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "list", "--query", "data"])
        .assert()
        .code(1)
        .stdout("")
        .stderr(contains(error));
}

#[rstest::rstest]
#[case(400, 3)]
#[case(503, 4)]
#[tokio::test(flavor = "multi_thread")]
async fn query_preserves_http_error_body_and_exit_code(
    #[case] status: u16,
    #[case] exit_code: i32,
) {
    let harness = Harness::new().await;
    Mock::given(path("/pets"))
        .respond_with(
            ResponseTemplate::new(status).set_body_json(json!({"title": "Request failed"})),
        )
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "list", "--query", "data", "-o", "json"])
        .assert()
        .code(exit_code)
        .stdout(contains("Request failed"))
        .stderr(contains(format!("HTTP {status}")));
}

#[rstest::rstest]
#[case("")]
#[case(".data")]
#[case("data..items")]
#[case("data.")]
#[tokio::test(flavor = "multi_thread")]
async fn query_rejects_invalid_paths_before_sending(#[case] query: &str) {
    let harness = Harness::new().await;
    Mock::given(path("/pets"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&harness.server)
        .await;
    harness
        .petstore(&["pets", "list", "--query", query])
        .assert()
        .code(2)
        .stderr(contains("nonempty field path"));
}
