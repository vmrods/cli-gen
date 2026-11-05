// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::time::Duration;

use opencli_core::HttpMethod;
use opencli_runtime::{DEFAULT_TIMEOUT, HttpClient, PlannedBody, RequestPlan, Response, SendError};
use serde_json::json;
use wiremock::matchers::{body_json, body_string, header, header_regex, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn plan(method: HttpMethod, url: String) -> RequestPlan {
    RequestPlan {
        method,
        url,
        query: Vec::new(),
        headers: Vec::new(),
        cookies: Vec::new(),
        body: None,
    }
}

fn client() -> HttpClient {
    HttpClient::new(DEFAULT_TIMEOUT).unwrap()
}

#[tokio::test]
async fn send_delivers_query_headers_cookies_and_json_body() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/pets"))
        .and(query_param("tag", "dog"))
        .and(header("x-trace", "t-1"))
        .and(header("cookie", "a=1; b=2"))
        .and(header("content-type", "application/json"))
        .and(header_regex("user-agent", "^opencli/"))
        .and(body_json(json!({"name": "Rex"})))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({"id": "1"})))
        .expect(1)
        .mount(&server)
        .await;
    let mut request = plan(HttpMethod::Post, format!("{}/pets", server.uri()));
    request.query = vec![("tag".into(), "dog".into())];
    request.headers = vec![("X-Trace".into(), "t-1".into())];
    request.cookies = vec![("a".into(), "1".into()), ("b".into(), "2".into())];
    request.body = Some(PlannedBody {
        content_type: "application/json".into(),
        value: json!({"name": "Rex"}),
    });

    let response = client().send(&request).await.unwrap();

    assert_eq!(response.status, 201);
    assert!(response.is_success());
    assert_eq!(response.json(), Some(json!({"id": "1"})));
}

#[tokio::test]
async fn send_passes_text_bodies_through_unquoted() {
    let server = MockServer::start().await;
    Mock::given(body_string("plain words"))
        .and(header("content-type", "text/plain"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let mut request = plan(HttpMethod::Put, format!("{}/note", server.uri()));
    request.body = Some(PlannedBody {
        content_type: "text/plain".into(),
        value: json!("plain words"),
    });
    let response = client().send(&request).await.unwrap();
    assert_eq!(response.status, 204);
    assert_eq!(response.json(), None);
}

#[tokio::test]
async fn send_returns_error_statuses_as_responses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_raw(r#"{"title":"Not Found"}"#, "application/problem+json"),
        )
        .mount(&server)
        .await;
    let response = client()
        .send(&plan(HttpMethod::Get, format!("{}/missing", server.uri())))
        .await
        .unwrap();
    assert_eq!(response.status, 404);
    assert_eq!(response.reason, "Not Found");
    assert!(!response.is_success());
    assert_eq!(response.json(), Some(json!({"title": "Not Found"})));
}

#[tokio::test]
async fn send_times_out_slow_responses() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(5)))
        .mount(&server)
        .await;
    let client = HttpClient::new(Duration::from_millis(100)).unwrap();
    let error = client
        .send(&plan(
            HttpMethod::Get,
            format!("{}/slow?token=secret", server.uri()),
        ))
        .await
        .unwrap_err();
    let SendError::Timeout { url } = error else {
        panic!("expected timeout, got {error:?}");
    };
    assert!(
        !url.contains("secret"),
        "query strings must not leak into errors: {url}"
    );
}

#[tokio::test]
async fn send_reports_refused_connections() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let error = client()
        .send(&plan(HttpMethod::Get, format!("http://127.0.0.1:{port}/")))
        .await
        .unwrap_err();
    assert!(matches!(error, SendError::Connect { .. }), "{error:?}");
}

#[tokio::test]
async fn send_rejects_relative_urls() {
    let error = client()
        .send(&plan(HttpMethod::Get, "/pets".into()))
        .await
        .unwrap_err();
    assert!(matches!(error, SendError::InvalidUrl { .. }), "{error:?}");
}

fn response(content_type: Option<&str>, body: &str) -> Response {
    Response {
        status: 200,
        reason: "OK".into(),
        content_type: content_type.map(str::to_owned),
        body: body.as_bytes().to_vec(),
    }
}

#[test]
fn json_parses_declared_or_undeclared_json_only() {
    assert_eq!(
        response(Some("application/json; charset=utf-8"), "[1]").json(),
        Some(json!([1]))
    );
    assert_eq!(response(None, r#"{"a":1}"#).json(), Some(json!({"a": 1})));
    assert_eq!(response(Some("text/plain"), "[1]").json(), None);
    assert_eq!(response(Some("application/json"), "not json").json(), None);
    assert_eq!(response(Some("text/plain"), "hello").text(), Some("hello"));
}
