//! The same Petstore, written in every supported format, must produce the
//! same commands. This is the contract ADR 0001 relies on.

// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use opencli_core::{ApiSpec, CommandSet, OAuth2Flow, SecurityScheme, SpecFormat, build_commands};
use opencli_spec::parse;
use rstest::rstest;

const OPENAPI_30: &str = include_str!("../../../fixtures/petstore.yaml");
const OPENAPI_31: &str = include_str!("../../../fixtures/petstore-3.1.yaml");
const OPENAPI_32: &str = include_str!("../../../fixtures/petstore-3.2.yaml");
const SWAGGER_20_YAML: &str = include_str!("../../../fixtures/petstore-2.0.yaml");
const SWAGGER_20_JSON: &str = include_str!("../../../fixtures/petstore-2.0.json");

fn reference() -> (ApiSpec, CommandSet) {
    let spec = parse(OPENAPI_30, "petstore.yaml").unwrap();
    let commands = build_commands(&spec).unwrap();
    (spec, commands)
}

/// Everything a user scripts against: names, arguments and where they go,
/// bodies, and which credentials each command needs.
fn surface(commands: &CommandSet) -> Vec<String> {
    commands
        .commands
        .iter()
        .map(|command| {
            format!(
                "{} | {} {} | {:?} | args {:?} | body {:?} | security {:?} | deprecated {}",
                command.path.join(" "),
                command.method,
                command.path_template,
                command.operation_id,
                command
                    .arguments
                    .iter()
                    .map(|argument| (
                        &argument.name,
                        argument.positional,
                        &argument.target,
                        argument.required,
                        argument.value_type,
                        argument.multiple,
                        argument.array_delimiter,
                    ))
                    .collect::<Vec<_>>(),
                command
                    .body
                    .as_ref()
                    .map(|body| (&body.content_type, body.required)),
                command.security,
                command.deprecated,
            )
        })
        .collect()
}

#[rstest]
#[case::openapi_31(OPENAPI_31, SpecFormat::OpenApi31)]
#[case::openapi_32(OPENAPI_32, SpecFormat::OpenApi32)]
#[case::swagger_20_yaml(SWAGGER_20_YAML, SpecFormat::Swagger20)]
#[case::swagger_20_json(SWAGGER_20_JSON, SpecFormat::Swagger20)]
fn every_format_exposes_the_same_commands(#[case] text: &str, #[case] format: SpecFormat) {
    let (expected_spec, expected_commands) = reference();
    let spec = parse(text, "petstore").unwrap();
    let commands = build_commands(&spec).unwrap();

    assert_eq!(spec.format, format);
    assert_eq!(
        (&spec.title, &spec.version, &spec.description, &spec.servers),
        (
            &expected_spec.title,
            &expected_spec.version,
            &expected_spec.description,
            &expected_spec.servers
        )
    );
    assert_eq!(surface(&commands), surface(&expected_commands));
    let summaries = |set: &CommandSet| {
        set.commands
            .iter()
            .map(|command| command.summary.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(summaries(&commands), summaries(&expected_commands));
}

#[rstest]
#[case::openapi_31(OPENAPI_31)]
#[case::openapi_32(OPENAPI_32)]
#[case::swagger_20(SWAGGER_20_YAML)]
fn every_format_keeps_body_schemas_by_reference(#[case] text: &str) {
    let spec = parse(text, "petstore").unwrap();
    let create = spec
        .operations
        .iter()
        .find(|operation| operation.operation_id.as_deref() == Some("createPet"))
        .unwrap();
    let body = create.request_body.as_ref().unwrap();
    assert_eq!(
        body.schema,
        Some(serde_json::json!({"$ref": "#/components/schemas/NewPet"}))
    );
    assert!(spec.schemas.contains_key("NewPet") && spec.schemas.contains_key("Pet"));
}

#[rstest]
#[case::openapi_31(OPENAPI_31)]
#[case::openapi_32(OPENAPI_32)]
fn openapi_3x_declares_the_same_security_schemes(#[case] text: &str) {
    assert_eq!(
        parse(text, "petstore").unwrap().security_schemes,
        reference().0.security_schemes,
        "3.2's deviceAuthorization flow is dropped, leaving the same schemes"
    );
}

#[test]
fn swagger_20_maps_its_security_definitions() {
    let expected = reference().0.security_schemes;
    let schemes = parse(SWAGGER_20_YAML, "petstore").unwrap().security_schemes;
    assert_eq!(schemes["apiKey"], expected["apiKey"]);
    assert_eq!(schemes["basic"], expected["basic"]);
    // 2.0 allows one flow per definition and has no bearer scheme.
    let SecurityScheme::OAuth2 { flows } = &schemes["oauth"] else {
        panic!("expected OAuth2");
    };
    assert!(
        matches!(flows.as_slice(), [OAuth2Flow::ClientCredentials { token_url, .. }] if token_url == "https://auth.example.com/token")
    );
    assert!(!schemes.contains_key("bearer"));
}

#[test]
fn openapi_31_schema_dialect_is_normalized() {
    let spec = parse(OPENAPI_31, "petstore").unwrap();
    assert_eq!(
        spec.schemas["Pet"]["properties"]["tag"],
        serde_json::json!({"type": "string", "nullable": true})
    );
}

#[test]
fn swagger_20_relative_base_path_becomes_a_relative_server() {
    let text = SWAGGER_20_YAML.replace("host: eu.petstore.example.com\n", "");
    assert_eq!(parse(&text, "petstore").unwrap().servers, ["/v1"]);
}

#[test]
fn swagger_20_form_data_becomes_a_form_body_with_flags() {
    let text = "swagger: '2.0'\ninfo: {title: Forms, version: '1'}\npaths:\n  /login:\n    post:\n      consumes: [application/x-www-form-urlencoded]\n      parameters:\n        - {name: user, in: formData, type: string, required: true}\n        - {name: remember, in: formData, type: boolean}\n      responses: {'200': {description: ok}}\n";
    let spec = parse(text, "forms.yaml").unwrap();
    let commands = build_commands(&spec).unwrap();
    let login = &commands.commands[0];
    assert_eq!(
        login.body.as_ref().unwrap().content_type,
        "application/x-www-form-urlencoded"
    );
    let flags: Vec<_> = login
        .arguments
        .iter()
        .map(|argument| argument.name.as_str())
        .collect();
    assert_eq!(flags, ["user", "remember"]);
}

#[test]
fn swagger_20_validation_issues_are_reported_with_operation_location() {
    let text = "swagger: '2.0'\ninfo: {title: Bad, version: '1'}\npaths:\n  /a/{id}:\n    get:\n      parameters:\n        - $ref: '#/parameters/Missing'\n      responses: {'200': {description: ok}}\n";
    let Err(opencli_spec::SpecError::Invalid { issues, .. }) = parse(text, "bad.yaml") else {
        panic!("expected validation issues");
    };
    let messages: Vec<String> = issues.iter().map(ToString::to_string).collect();
    assert_eq!(
        messages,
        [
            "GET /a/{id}: reference `#/parameters/Missing` points to an undefined parameter",
            "GET /a/{id}: path parameter `id` is used in the path but not declared",
        ]
    );
}
