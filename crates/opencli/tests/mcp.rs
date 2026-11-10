#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Stdio;
use std::time::Duration;

use assert_cmd::cargo::{cargo_bin, cargo_bin_cmd};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SPEC: &str = "openapi: 3.0.3
info: {title: Agent test, version: '1'}
servers: [{url: 'https://unused.example.com'}]
security: [{bearer: []}]
components:
  securitySchemes:
    bearer: {type: http, scheme: bearer}
paths:
  /items:
    get:
      summary: Read the collection
      parameters:
        - {name: limit, in: query, schema: {type: integer}}
        - {name: appToken, in: query, schema: {type: string}}
        - {name: X-Debug, in: header, schema: {type: string}}
        - {name: session, in: cookie, schema: {type: string}}
      responses: {'200': {description: ok}}
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema: {type: object, required: [name], properties: {name: {type: string}}}
      responses: {'201': {description: created}}
  /items/{id}:
    parameters: [{name: id, in: path, required: true, schema: {type: integer}}]
    get:
      responses: {'200': {description: ok}}
    delete:
      responses: {'204': {description: deleted}}
  /login:
    post:
      security: []
      requestBody:
        content:
          application/json:
            schema: {type: object, properties: {password: {type: string, format: password}}}
      responses: {'200': {description: token}}
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema: {type: object, properties: {file: {type: string, format: binary}}}
      responses: {'200': {description: uploaded}}
  /nullable:
    post:
      requestBody:
        required: true
        content:
          application/json:
            schema: {type: string, nullable: true}
      responses: {'200': {description: ok}}
";
const TEST_TOKEN: &str = "mcp-test-secret";
const REPLY_TIMEOUT: Duration = Duration::from_secs(10);

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
        let spec = harness.home.path().join("api.yaml");
        std::fs::write(&spec, SPEC).unwrap();
        for name in ["demo", "private"] {
            cargo_bin_cmd!("opencli")
                .env("OPENCLI_HOME", harness.home.path())
                .env("OPENCLI_CREDENTIAL_STORE", "file")
                .env("OPENCLI_PASSPHRASE", "mcp-tests-only")
                .args([
                    "--server",
                    &harness.server.uri(),
                    "api",
                    "add",
                    name,
                    "--spec",
                ])
                .arg(&spec)
                .assert()
                .success();
        }
        harness
    }

    async fn connect(&self, writes: bool, max_bytes: u32) -> Session {
        self.connect_filter(writes, max_bytes, None, Some(TEST_TOKEN))
            .await
    }

    async fn connect_filter(
        &self,
        writes: bool,
        max_bytes: u32,
        operation: Option<&str>,
        token: Option<&str>,
    ) -> Session {
        let mut command = Command::new(cargo_bin!("opencli"));
        command
            .env("OPENCLI_HOME", self.home.path())
            .env("OPENCLI_CREDENTIAL_STORE", "file")
            .env("OPENCLI_PASSPHRASE", "mcp-tests-only")
            .env_remove("OPENCLI_DEMO_BEARER")
            .env_remove("OPENCLI_PRIVATE_BEARER")
            .args([
                "--timeout",
                "1",
                "mcp",
                "serve",
                "--api",
                "demo",
                "--max-output-bytes",
                &max_bytes.to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if let Some(token) = token {
            command.env("OPENCLI_DEMO_BEARER", token);
        }
        if writes {
            command.arg("--allow-writes");
        }
        if let Some(operation) = operation {
            command.args(["--operation", operation]);
        }
        let mut child = command.spawn().unwrap();
        let input = child.stdin.take().unwrap();
        let output = BufReader::new(child.stdout.take().unwrap());
        let mut session = Session {
            child,
            input: Some(input),
            output,
            next_id: 0,
        };
        let init = session.rpc("initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"opencli-test","version":"1"}})).await;
        assert_eq!(init["result"]["serverInfo"]["name"], "opencli", "{init}");
        session
            .write(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await;
        session
    }
}

struct Session {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    next_id: u32,
}

impl Session {
    async fn close(mut self) {
        drop(self.input.take());
        let status = tokio::time::timeout(REPLY_TIMEOUT, self.child.wait())
            .await
            .expect("MCP server did not exit on EOF")
            .unwrap();
        assert!(
            status.success(),
            "MCP server failed during shutdown: {status}"
        );
    }

    async fn write(&mut self, request: Value) {
        let mut bytes = serde_json::to_vec(&request).unwrap();
        bytes.push(b'\n');
        let input = self.input.as_mut().unwrap();
        input.write_all(&bytes).await.unwrap();
        input.flush().await.unwrap();
    }

    async fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let id = self.next_id;
        self.write(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await;
        tokio::time::timeout(REPLY_TIMEOUT, async {
            loop {
                let mut line = String::new();
                assert_ne!(
                    self.output.read_line(&mut line).await.unwrap(),
                    0,
                    "server exited"
                );
                let reply: Value = serde_json::from_str(&line).unwrap();
                if reply["id"] == id {
                    return reply;
                }
            }
        })
        .await
        .expect("MCP response timed out")
    }

    async fn call(&mut self, name: &str, arguments: Value, error: bool) -> Value {
        let reply = self
            .rpc("tools/call", json!({"name":name,"arguments":arguments}))
            .await;
        assert_eq!(reply["result"]["isError"], error, "{reply}");
        reply["result"]["structuredContent"].clone()
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn discovery_exposes_five_tools_and_only_allowed_apis() {
    let harness = Harness::new().await;
    let mut session = harness.connect(false, 32_768).await;
    let tools = session.rpc("tools/list", json!({})).await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 5);
    let apis = session.call("list_apis", json!({}), false).await;
    assert_eq!(apis["apis"].as_array().unwrap().len(), 1);
    assert_eq!(apis["apis"][0]["name"], "demo");
    let denied = session
        .call("search_operations", json!({"api":"private"}), true)
        .await;
    assert_eq!(denied["error"]["code"], "api_not_allowed");
    let search = session
        .call(
            "search_operations",
            json!({"api":"demo","query":"GET items","limit":1}),
            false,
        )
        .await;
    assert_eq!(search["total_items"], 2);
    assert_eq!(search["next_offset"], 1);
    let second = session
        .call(
            "search_operations",
            json!({"api":"demo","query":"GET items","offset":1,"limit":1}),
            false,
        )
        .await;
    assert_eq!(second["next_offset"], Value::Null);
    let described = session
        .call(
            "describe_operation",
            json!({"api":"demo","operation":"GET /items/{id}"}),
            false,
        )
        .await;
    assert_eq!(described["contract"]["arguments"][0]["name"], "id");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn planning_validates_and_redacts_without_sending_or_loading_credentials() {
    let harness = Harness::new().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(false, 32_768).await;
    let planned = session
        .call(
            "plan_request",
            json!({"api":"demo","operation":"POST /login","body":{"password":"private-password"}}),
            false,
        )
        .await;
    assert_eq!(planned["plan"]["body"]["value"]["password"], "<redacted>");
    assert_eq!(planned["plan"]["credentials"], json!([]));
    let missing = session
        .call(
            "plan_request",
            json!({"api":"demo","operation":"GET /items/{id}"}),
            true,
        )
        .await;
    assert_eq!(missing["error"]["code"], "missing_argument");
    let invalid = session
        .call(
            "plan_request",
            json!({"api":"demo","operation":"POST /items","body":{"name":3}}),
            true,
        )
        .await;
    assert_eq!(invalid["error"]["code"], "invalid_body");
    let unknown = session.call("plan_request", json!({"api":"demo","operation":"GET /items","arguments":{"server":"https://evil.example"}}), true).await;
    assert_eq!(unknown["error"]["code"], "invalid_arguments");
    session.close().await;
}

#[rstest::rstest]
#[case(false, false)]
#[case(false, true)]
#[case(true, false)]
#[tokio::test(flavor = "multi_thread")]
async fn writes_require_both_server_opt_in_and_call_confirmation(
    #[case] writes: bool,
    #[case] confirmed: bool,
) {
    let harness = Harness::new().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(writes, 32_768).await;
    let result = session.call("execute_operation", json!({"api":"demo","operation":"POST /items","body":{"name":"Ada"},"confirmed":confirmed}), true).await;
    assert_eq!(result["error"]["code"], "write_not_allowed");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reads_use_credentials_select_fields_paginate_and_audit() {
    let harness = Harness::new().await;
    Mock::given(method("GET"))
        .and(path("/items"))
        .and(header("Authorization", format!("Bearer {TEST_TOKEN}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"data":[{"id":1,"token":"private-token"},{"id":2}],"secret":"private-value"}),
        ))
        .expect(1)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(false, 32_768).await;
    let result = session
        .call(
            "execute_operation",
            json!({"api":"demo","operation":"GET /items","query":"data","limit":1}),
            false,
        )
        .await;
    assert_eq!(result["http_status"], 200);
    assert_eq!(result["data"], json!([{"id":1,"token":"<redacted>"}]));
    assert_eq!(result["next_offset"], 1);
    assert!(!result.to_string().contains("private-token"));
    let audit = std::fs::read_to_string(harness.home.path().join("audit.log")).unwrap();
    assert!(audit.contains("http") || audit.contains("request"));
    assert!(!audit.contains(TEST_TOKEN));
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn authorized_write_uses_the_registered_contract() {
    let harness = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/items"))
        .and(body_json(json!({"name":"Ada"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id":7})))
        .expect(1)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(true, 32_768).await;
    let result = session.call("execute_operation", json!({"api":"demo","operation":"POST /items","arguments":{"name":"Ada"},"confirmed":true}), false).await;
    assert_eq!(result["http_status"], 201);
    assert_eq!(result["data"]["id"], 7);
    session.close().await;
}

#[rstest::rstest]
#[case("POST /upload", json!({"file":"/etc/passwd"}), Value::Null, "local_file_not_allowed")]
#[case("POST /login", json!({}), json!({"password":"test"}), "login_requires_cli")]
#[tokio::test(flavor = "multi_thread")]
async fn model_inputs_cannot_upload_local_files_or_run_sign_ins(
    #[case] operation: &str,
    #[case] arguments: Value,
    #[case] body: Value,
    #[case] code: &str,
) {
    let harness = Harness::new().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(true, 32_768).await;
    let result = session.call("execute_operation", json!({"api":"demo","operation":operation,"arguments":arguments,"body":body,"confirmed":true}), true).await;
    assert_eq!(result["error"]["code"], code);
    session.close().await;
}

#[rstest::rstest]
#[case(401)]
#[case(503)]
#[tokio::test(flavor = "multi_thread")]
async fn http_failures_preserve_structured_status_and_redacted_body(#[case] status: u16) {
    let harness = Harness::new().await;
    Mock::given(path("/items"))
        .respond_with(
            ResponseTemplate::new(status)
                .set_body_json(json!({"title":"denied","access_token":"private"})),
        )
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(false, 32_768).await;
    let result = session
        .call(
            "execute_operation",
            json!({"api":"demo","operation":"GET /items","query":"data"}),
            true,
        )
        .await;
    assert_eq!(result["http_status"], status);
    assert_eq!(result["data"]["title"], "denied");
    assert_eq!(result["data"]["access_token"], "<redacted>");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn large_results_are_not_silently_cut_into_invalid_json() {
    let harness = Harness::new().await;
    Mock::given(path("/items"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"description":"x".repeat(3000)})),
        )
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(false, 1024).await;
    let result = session
        .call(
            "execute_operation",
            json!({"api":"demo","operation":"GET /items"}),
            true,
        )
        .await;
    assert_eq!(result["error"]["code"], "result_too_large");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_is_a_recoverable_tool_error() {
    let harness = Harness::new().await;
    Mock::given(path("/items"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_delay(Duration::from_secs(3))
                .set_body_json(json!([])),
        )
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(false, 32_768).await;
    let result = session
        .call(
            "execute_operation",
            json!({"api":"demo","operation":"GET /items"}),
            true,
        )
        .await;
    assert_eq!(result["error"]["code"], "timeout");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn successful_empty_responses_are_not_reported_as_failed_writes() {
    let harness = Harness::new().await;
    Mock::given(method("DELETE"))
        .and(path("/items/7"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(true, 32_768).await;
    let result = session.call("execute_operation", json!({"api":"demo","operation":"DELETE /items/{id}","arguments":{"id":7},"confirmed":true}), false).await;
    assert_eq!(result["http_status"], 204);
    assert_eq!(result["data"], Value::Null);
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn operation_allowlist_applies_to_discovery_description_and_execution() {
    let harness = Harness::new().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&harness.server)
        .await;
    let mut session = harness
        .connect_filter(false, 32_768, Some("GET /items"), Some(TEST_TOKEN))
        .await;
    let apis = session.call("list_apis", json!({}), false).await;
    assert_eq!(apis["apis"][0]["operation_count"], 1);
    let search = session
        .call("search_operations", json!({"api":"demo"}), false)
        .await;
    assert_eq!(search["total_items"], 1);
    for tool in ["describe_operation", "plan_request", "execute_operation"] {
        let result = session
            .call(
                tool,
                json!({"api":"demo","operation":"GET /items/{id}"}),
                true,
            )
            .await;
        assert_eq!(result["error"]["code"], "operation_not_found");
    }
    session.close().await;
}

#[rstest::rstest]
#[case(json!({"api":"demo","operation":"POST /items","body":{"name":"Ada"},"confirmed":true,"query":"missing"}), "invalid_query")]
#[case(json!({"api":"demo","operation":"POST /items","body":{"name":"Ada"},"confirmed":true}), "result_too_large")]
#[tokio::test(flavor = "multi_thread")]
async fn post_response_errors_preserve_evidence_the_write_was_sent(
    #[case] arguments: Value,
    #[case] code: &str,
) {
    let harness = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/items"))
        .respond_with(
            ResponseTemplate::new(201).set_body_json(json!({"description":"x".repeat(3000)})),
        )
        .expect(1)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(true, 1024).await;
    let result = session.call("execute_operation", arguments, true).await;
    assert_eq!(result["error"]["code"], code);
    assert_eq!(result["request_sent"], true);
    assert_eq!(result["http_status"], 201);
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_cannot_be_replayed_to_page_output() {
    let harness = Harness::new().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(201))
        .expect(0)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(true, 32_768).await;
    let result = session.call("execute_operation", json!({"api":"demo","operation":"POST /items","body":{"name":"Ada"},"confirmed":true,"offset":1}), true).await;
    assert_eq!(result["error"]["code"], "write_paging_not_allowed");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn nullable_json_body_remains_distinct_from_missing_body() {
    let harness = Harness::new().await;
    Mock::given(method("POST"))
        .and(path("/nullable"))
        .and(body_json(Value::Null))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok":true})))
        .expect(1)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(true, 32_768).await;
    let result = session
        .call(
            "execute_operation",
            json!({"api":"demo","operation":"POST /nullable","body":null,"confirmed":true}),
            false,
        )
        .await;
    assert_eq!(result["http_status"], 200);
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn argument_payloads_and_error_results_are_bounded() {
    let harness = Harness::new().await;
    let mut session = harness.connect(false, 1024).await;
    let result = session
        .call(
            "plan_request",
            json!({"api":"demo","operation":"GET /items","arguments":{"x".repeat(3000):"invalid"}}),
            true,
        )
        .await;
    assert_eq!(result["error"]["code"], "result_too_large");
    let result = session
        .call(
            "plan_request",
            json!({"api":"demo","operation":"POST /items","body":{"name":"x".repeat(70_000)}}),
            true,
        )
        .await;
    assert_eq!(result["error"]["code"], "input_too_large");
    session.close().await;
}

#[rstest::rstest]
#[case(&["mcp", "serve"], 2)]
#[case(&["mcp", "serve", "--api", "missing"], 1)]
#[case(&["mcp", "serve", "--api", "demo", "--operation", "GET /missing"], 1)]
#[case(&["--server", "https://override.example", "mcp", "serve", "--api", "demo"], 1)]
#[tokio::test(flavor = "multi_thread")]
async fn startup_rejects_invalid_configuration_without_protocol_noise(
    #[case] arguments: &[&str],
    #[case] code: i32,
) {
    let harness = Harness::new().await;
    cargo_bin_cmd!("opencli")
        .env("OPENCLI_HOME", harness.home.path())
        .env("OPENCLI_CREDENTIAL_STORE", "file")
        .env("OPENCLI_PASSPHRASE", "mcp-tests-only")
        .args(arguments)
        .assert()
        .code(code)
        .stdout("");
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_credentials_return_an_error_before_the_request_is_sent() {
    let harness = Harness::new().await;
    Mock::given(path("/items"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect_filter(false, 32_768, None, None).await;
    let result = session
        .call(
            "execute_operation",
            json!({"api":"demo","operation":"GET /items"}),
            true,
        )
        .await;
    assert_eq!(result["error"]["code"], "authentication_required");
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn previews_redact_query_secrets_headers_and_cookies() {
    let harness = Harness::new().await;
    let mut session = harness.connect(false, 32_768).await;
    let result = session.call("plan_request", json!({"api":"demo","operation":"GET /items","arguments":{
        "limit":10,"app-token":"private-token","x-debug":"private-header","session":"private-cookie"
    }}), false).await;
    assert_eq!(result["plan"]["headers"][0][1], "<redacted>");
    assert_eq!(result["plan"]["cookies"][0][1], "<redacted>");
    let query = result["plan"]["query"].as_array().unwrap();
    assert!(query.contains(&json!(["appToken", "<redacted>"])));
    assert!(query.contains(&json!(["limit", "10"])));
    assert!(!result.to_string().contains("private-"));
    session.close().await;
}

#[rstest::rstest]
#[case("list_apis", json!({"unexpected":true}), "invalid_arguments")]
#[case("unknown_tool", json!({}), "unknown_tool")]
#[case("search_operations", json!({"api":"Bad_API"}), "api_not_allowed")]
#[case("search_operations", json!({"api":"demo","limit":0}), "invalid_arguments")]
#[case("plan_request", json!({"api":"demo","operation":"GET /items","body":null}), "invalid_arguments")]
#[case("plan_request", json!({"api":"demo","operation":"GET /items","arguments":{"limit":[1,2]}}), "invalid_arguments")]
#[case("plan_request", json!({"api":"demo","operation":"GET /items","arguments":{"limit":"not-an-integer"}}), "invalid_arguments")]
#[case("plan_request", json!({"api":"demo","operation":"POST /items"}), "invalid_request")]
#[tokio::test(flavor = "multi_thread")]
async fn malformed_tool_inputs_produce_structured_errors_without_http(
    #[case] tool: &str,
    #[case] arguments: Value,
    #[case] code: &str,
) {
    let harness = Harness::new().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(false, 32_768).await;
    let result = session.call(tool, arguments, true).await;
    assert_eq!(result["error"]["code"], code);
    session.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn text_responses_preserve_the_status_instead_of_dropping_the_request_outcome() {
    let harness = Harness::new().await;
    Mock::given(path("/items"))
        .respond_with(ResponseTemplate::new(200).set_body_string("plain text"))
        .expect(1)
        .mount(&harness.server)
        .await;
    let mut session = harness.connect(false, 32_768).await;
    let result = session
        .call(
            "execute_operation",
            json!({"api":"demo","operation":"GET /items"}),
            true,
        )
        .await;
    assert_eq!(result["error"]["code"], "non_json_response");
    assert_eq!(result["http_status"], 200);
    assert_eq!(result["request_sent"], true);
    session.close().await;
}
