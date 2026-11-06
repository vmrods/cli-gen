// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use opencli_core::{
    ApiKeyLocation, ApiSpec, HttpMethod, OAuth2Flow, Operation, ParameterLocation,
    SchemeRequirement, SecurityRequirement, SecurityScheme, SpecFormat,
};
use opencli_spec::{SpecError, parse};
use rstest::rstest;
use serde_json::json;

const PETSTORE_YAML: &str = include_str!("../../../fixtures/petstore.yaml");
const PETSTORE_JSON: &str = include_str!("../../../fixtures/petstore.json");

fn petstore() -> ApiSpec {
    parse(PETSTORE_YAML, "petstore.yaml").unwrap()
}

fn operation<'s>(spec: &'s ApiSpec, id: &str) -> &'s Operation {
    spec.operations
        .iter()
        .find(|operation| operation.operation_id.as_deref() == Some(id))
        .unwrap()
}

/// Minimal valid 3.0 document with the given `paths` and `components` YAML.
fn document(paths: &str, components: &str) -> String {
    format!(
        "openapi: 3.0.3\ninfo:\n  title: Test\n  version: '1'\npaths:\n{}\ncomponents:\n{}\n",
        indent(paths),
        indent(components)
    )
}

fn indent(yaml: &str) -> String {
    yaml.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn validation_messages(text: &str) -> Vec<String> {
    match parse(text, "test.yaml") {
        Err(SpecError::Invalid { issues, .. }) => issues.iter().map(ToString::to_string).collect(),
        other => panic!("expected validation failure, got {other:?}"),
    }
}

#[test]
fn parse_reads_petstore_info_and_servers() {
    let spec = petstore();
    assert_eq!(spec.format, SpecFormat::OpenApi30);
    assert_eq!(spec.title, "Petstore");
    assert_eq!(spec.version, "1.0.0");
    assert_eq!(spec.servers, ["https://eu.petstore.example.com/v1"]);
}

#[test]
fn parse_yields_same_model_from_json_and_yaml() {
    assert_eq!(parse(PETSTORE_JSON, "petstore.json").unwrap(), petstore());
}

#[test]
fn parse_discovers_supported_operations() {
    let spec = petstore();
    let found: Vec<_> = spec
        .operations
        .iter()
        .map(|operation| (operation.method, operation.path.as_str()))
        .collect();
    assert_eq!(
        found,
        [
            (HttpMethod::Get, "/pets"),
            (HttpMethod::Post, "/pets"),
            (HttpMethod::Get, "/pets/{petId}"),
            (HttpMethod::Delete, "/pets/{petId}"),
        ]
    );
}

#[test]
fn parse_resolves_parameter_references() {
    let spec = petstore();
    let limit = &operation(&spec, "listPets").parameters[0];
    assert_eq!(limit.name, "limit");
    assert_eq!(limit.location, ParameterLocation::Query);
    assert!(!limit.required);
    assert_eq!(
        limit.schema,
        Some(json!({"type": "integer", "format": "int32", "maximum": 100}))
    );
}

#[test]
fn parse_inherits_path_level_parameters() {
    let spec = petstore();
    let parameters = &operation(&spec, "showPetById").parameters;
    assert_eq!(parameters.len(), 1);
    assert_eq!(parameters[0].name, "petId");
    assert!(parameters[0].required);
}

#[test]
fn parse_resolves_request_body_reference_and_keeps_schema_ref() {
    let spec = petstore();
    let body = operation(&spec, "createPet").request_body.clone().unwrap();
    assert!(body.required);
    assert_eq!(body.content_type, "application/json");
    assert_eq!(
        body.schema,
        Some(json!({"$ref": "#/components/schemas/NewPet"}))
    );
    assert!(spec.schemas.contains_key("NewPet"));
}

#[rstest]
#[case("listPets", vec![SecurityRequirement { schemes: vec![SchemeRequirement { scheme: "apiKey".into(), scopes: vec![] }] }])]
#[case("createPet", vec![SecurityRequirement { schemes: vec![SchemeRequirement { scheme: "oauth".into(), scopes: vec!["pets:write".into()] }] }])]
#[case("deletePet", vec![])]
fn parse_applies_operation_security_over_document_default(
    #[case] id: &str,
    #[case] expected: Vec<SecurityRequirement>,
) {
    assert_eq!(operation(&petstore(), id).security, expected);
}

#[test]
fn parse_marks_deprecated_operations() {
    let spec = petstore();
    assert!(operation(&spec, "deletePet").deprecated);
    assert!(!operation(&spec, "listPets").deprecated);
}

#[test]
fn parse_maps_security_schemes() {
    let schemes = petstore().security_schemes;
    assert_eq!(
        schemes["apiKey"],
        SecurityScheme::ApiKey {
            name: "X-API-Key".into(),
            location: ApiKeyLocation::Header
        }
    );
    assert_eq!(schemes["basic"], SecurityScheme::HttpBasic);
    assert_eq!(
        schemes["bearer"],
        SecurityScheme::HttpBearer {
            bearer_format: Some("JWT".into())
        }
    );
    let SecurityScheme::OAuth2 { flows } = &schemes["oauth"] else {
        panic!("expected OAuth2 scheme");
    };
    assert!(
        matches!(&flows[0], OAuth2Flow::ClientCredentials { token_url, .. } if token_url == "https://auth.example.com/token")
    );
    assert!(
        matches!(&flows[1], OAuth2Flow::AuthorizationCode { authorization_url, .. } if authorization_url == "https://auth.example.com/authorize")
    );
}

#[test]
fn parse_marks_unknown_http_and_legacy_oauth_schemes_unsupported() {
    let text = document(
        "/a:\n  get:\n    responses:\n      '200':\n        description: ok",
        "securitySchemes:\n  digest:\n    type: http\n    scheme: digest\n  implicit:\n    type: oauth2\n    flows:\n      implicit:\n        authorizationUrl: https://a.example/auth\n        scopes: {}",
    );
    let schemes = parse(&text, "test.yaml").unwrap().security_schemes;
    assert!(matches!(
        schemes["digest"],
        SecurityScheme::Unsupported { .. }
    ));
    assert!(matches!(
        schemes["implicit"],
        SecurityScheme::Unsupported { .. }
    ));
}

#[test]
fn parse_lets_operation_parameter_override_path_parameter() {
    let text = document(
        "/a:\n  parameters:\n    - {name: q, in: query, description: path-level, schema: {type: string}}\n  get:\n    parameters:\n      - {name: q, in: query, description: operation-level, schema: {type: string}}\n    responses:\n      '200':\n        description: ok",
        "{}",
    );
    let spec = parse(&text, "test.yaml").unwrap();
    let parameters = &spec.operations[0].parameters;
    assert_eq!(parameters.len(), 1);
    assert_eq!(
        parameters[0].description.as_deref(),
        Some("operation-level")
    );
}

#[test]
fn parse_reports_json_syntax_error_with_position() {
    let text = "{\n  \"openapi\": \"3.0.3\",\n  oops\n}";
    let error = parse(text, "bad.json").unwrap_err();
    let SpecError::Syntax(syntax) = error else {
        panic!("expected syntax error, got {error:?}");
    };
    let offset = syntax.span.offset();
    assert_eq!(&text[offset..offset + 4], "oops");
}

#[test]
fn parse_reports_yaml_syntax_error() {
    let error = parse("openapi: 3.0.3\ninfo: [unclosed\n", "bad.yaml").unwrap_err();
    assert!(matches!(error, SpecError::Syntax(_)), "{error:?}");
}

#[rstest]
#[case("openapi: 3.3.0\n", "OpenAPI 3.3.0")]
#[case("swagger: '1.2'\n", "Swagger 1.2")]
fn parse_rejects_unsupported_versions(#[case] text: &str, #[case] expected: &str) {
    let error = parse(text, "spec.yaml").unwrap_err();
    assert!(
        matches!(&error, SpecError::UnsupportedVersion { version, .. } if version == expected),
        "{error:?}"
    );
}

#[rstest]
#[case("title: not a spec\n")]
#[case("")]
fn parse_rejects_documents_without_version(#[case] text: &str) {
    assert!(matches!(
        parse(text, "x.yaml"),
        Err(SpecError::UnknownFormat { .. })
    ));
}

#[test]
fn parse_reports_structure_errors_with_path() {
    let error = parse("openapi: 3.0.3\ninfo: just a string\npaths: {}\n", "x.yaml").unwrap_err();
    assert!(
        matches!(&error, SpecError::Structure { path, .. } if path == "info"),
        "{error:?}"
    );
}

#[rstest]
#[case::undeclared_path_parameter(
    "/a/{id}:\n  get:\n    responses:\n      '200':\n        description: ok",
    "{}",
    "GET /a/{id}: path parameter `id` is used in the path but not declared"
)]
#[case::unused_path_parameter(
    "/a:\n  get:\n    parameters:\n      - {name: id, in: path, required: true, schema: {type: string}}\n    responses:\n      '200':\n        description: ok",
    "{}",
    "GET /a: path parameter `id` is declared but not used in the path"
)]
#[case::optional_path_parameter(
    "/a/{id}:\n  get:\n    parameters:\n      - {name: id, in: path, required: false, schema: {type: string}}\n    responses:\n      '200':\n        description: ok",
    "{}",
    "GET /a/{id}: path parameter `id` must be required"
)]
#[case::undefined_security_scheme(
    "/a:\n  get:\n    security: [{missing: []}]\n    responses:\n      '200':\n        description: ok",
    "{}",
    "GET /a: security scheme `missing` is not defined in components.securitySchemes"
)]
#[case::undefined_reference(
    "/a:\n  get:\n    parameters:\n      - $ref: '#/components/parameters/Nope'\n    responses:\n      '200':\n        description: ok",
    "{}",
    "GET /a: reference `#/components/parameters/Nope` points to an undefined component"
)]
#[case::external_reference(
    "/a:\n  get:\n    parameters:\n      - $ref: 'other.yaml#/Limit'\n    responses:\n      '200':\n        description: ok",
    "{}",
    "GET /a: reference `other.yaml#/Limit` is not supported; only local `#/components/parameters/` references are"
)]
#[case::circular_reference(
    "/a:\n  get:\n    parameters:\n      - $ref: '#/components/parameters/A'\n    responses:\n      '200':\n        description: ok",
    "parameters:\n  A: {$ref: '#/components/parameters/B'}\n  B: {$ref: '#/components/parameters/A'}",
    "GET /a: reference chain is circular or deeper than 16 levels"
)]
#[case::empty_request_body(
    "/a:\n  post:\n    requestBody:\n      content: {}\n    responses:\n      '200':\n        description: ok",
    "{}",
    "POST /a: request body declares no content types"
)]
#[case::no_operations(
    "{}",
    "{}",
    "paths: the document defines no GET, POST, PUT, PATCH or DELETE operations"
)]
fn parse_reports_validation_issue(
    #[case] paths: &str,
    #[case] components: &str,
    #[case] expected: &str,
) {
    assert_eq!(
        validation_messages(&document(paths, components)),
        [expected]
    );
}

#[test]
fn parse_reports_duplicate_operation_ids_and_all_issues_together() {
    let text = document(
        "/a:\n  get:\n    operationId: same\n    responses:\n      '200':\n        description: ok\n/b/{id}:\n  get:\n    operationId: same\n    responses:\n      '200':\n        description: ok",
        "{}",
    );
    assert_eq!(
        validation_messages(&text),
        [
            "GET /b/{id}: path parameter `id` is used in the path but not declared",
            "GET /b/{id}: operationId `same` is also used by GET /a",
        ]
    );
}

#[test]
fn parse_reads_cli_naming_extensions() {
    let text = document(
        "/a:\n  get:\n    x-cli-name: recent\n    x-cli-resource: [audit, logs]\n    responses:\n      '200':\n        description: ok\n/b:\n  get:\n    x-cli-resource: daily reports\n    responses:\n      '200':\n        description: ok",
        "{}",
    );
    let spec = parse(&text, "test.yaml").unwrap();
    assert_eq!(spec.operations[0].cli.name.as_deref(), Some("recent"));
    assert_eq!(
        spec.operations[0].cli.resource,
        Some(vec!["audit".to_owned(), "logs".to_owned()])
    );
    assert_eq!(spec.operations[1].cli.name, None);
    assert_eq!(
        spec.operations[1].cli.resource,
        Some(vec!["daily".to_owned(), "reports".to_owned()])
    );
}

#[rstest]
#[case("x-cli-name: 42", "GET /a: `x-cli-name` must be a non-empty string")]
#[case(
    "x-cli-resource: []",
    "GET /a: `x-cli-resource` must be a string of words or a non-empty array of strings"
)]
#[case(
    "x-cli-resource: [1]",
    "GET /a: `x-cli-resource` must be a string of words or a non-empty array of strings"
)]
fn parse_rejects_malformed_cli_extensions(#[case] extension: &str, #[case] expected: &str) {
    let text = document(
        &format!(
            "/a:\n  get:\n    {extension}\n    responses:\n      '200':\n        description: ok"
        ),
        "{}",
    );
    assert_eq!(validation_messages(&text), [expected]);
}

#[test]
fn parse_maps_array_styles_to_delimiters() {
    let text = document(
        "/a:\n  get:\n    parameters:\n      - {name: repeat, in: query, schema: {type: array, items: {type: string}}}\n      - {name: csv, in: query, explode: false, schema: {type: array, items: {type: string}}}\n      - {name: pipes, in: query, style: pipeDelimited, schema: {type: array, items: {type: string}}}\n      - {name: spaces, in: query, style: spaceDelimited, schema: {type: array, items: {type: string}}}\n      - {name: X-Ids, in: header, schema: {type: array, items: {type: string}}}\n    responses:\n      '200':\n        description: ok",
        "{}",
    );
    let spec = parse(&text, "test.yaml").unwrap();
    let delimiters: Vec<_> = spec.operations[0]
        .parameters
        .iter()
        .map(|parameter| (parameter.name.as_str(), parameter.array_delimiter))
        .collect();
    assert_eq!(
        delimiters,
        [
            ("repeat", None),
            ("csv", Some(',')),
            ("pipes", Some('|')),
            ("spaces", Some(' ')),
            ("X-Ids", Some(','))
        ]
    );
}

#[test]
fn parse_reports_missing_root_fields_at_the_document_root() {
    let error = parse("openapi: 3.0.3\npaths: {}\n", "x.yaml").unwrap_err();
    assert!(
        matches!(&error, SpecError::Structure { path, .. } if path == "document root"),
        "{error:?}"
    );
}
