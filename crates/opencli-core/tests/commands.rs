// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeMap;

use opencli_core::{
    ApiSpec, ArgumentSpec, ArgumentTarget, CliHints, CommandSet, HttpMethod, NamingError,
    Operation, Parameter, ParameterLocation, RequestBody, SpecFormat, ValueType, build_commands,
};
use rstest::rstest;
use serde_json::{Value, json};

/// Builds operations with only the fields a test cares about.
struct OperationBuilder(Operation);

fn operation(method: HttpMethod, path: &str) -> OperationBuilder {
    let path_parameters = path
        .split('{')
        .skip(1)
        .filter_map(|segment| segment.split_once('}').map(|(name, _)| name))
        .map(|name| Parameter {
            name: name.to_owned(),
            location: ParameterLocation::Path,
            required: true,
            description: None,
            schema: Some(json!({"type": "string"})),
            array_delimiter: None,
        })
        .collect();
    OperationBuilder(Operation {
        operation_id: None,
        method,
        path: path.to_owned(),
        summary: None,
        description: None,
        deprecated: false,
        parameters: path_parameters,
        request_body: None,
        security: Vec::new(),
        cli: CliHints::default(),
    })
}

impl OperationBuilder {
    fn id(mut self, id: &str) -> Self {
        self.0.operation_id = Some(id.to_owned());
        self
    }

    fn parameter(mut self, name: &str, location: ParameterLocation, schema: Value) -> Self {
        self.0.parameters.push(Parameter {
            name: name.to_owned(),
            location,
            required: false,
            description: Some(format!("{name} help")),
            schema: Some(schema),
            array_delimiter: None,
        });
        self
    }

    fn body(mut self, content_type: &str, schema: Value) -> Self {
        self.0.request_body = Some(RequestBody {
            required: true,
            content_type: content_type.to_owned(),
            description: None,
            schema: Some(schema),
        });
        self
    }

    fn hints(mut self, name: Option<&str>, resource: Option<&[&str]>) -> Self {
        self.0.cli = CliHints {
            name: name.map(str::to_owned),
            resource: resource.map(|words| words.iter().map(|&word| word.to_owned()).collect()),
        };
        self
    }
}

fn spec(operations: Vec<OperationBuilder>) -> ApiSpec {
    ApiSpec {
        format: SpecFormat::OpenApi30,
        title: "Test".into(),
        version: "1".into(),
        description: None,
        servers: vec!["https://api.example.com".into()],
        operations: operations.into_iter().map(|builder| builder.0).collect(),
        security_schemes: BTreeMap::new(),
        schemas: BTreeMap::from([("Limit".to_owned(), json!({"type": "integer"}))]),
    }
}

fn names(set: &CommandSet) -> Vec<String> {
    set.commands
        .iter()
        .map(|command| command.path.join(" "))
        .collect()
}

fn names_of(operations: Vec<OperationBuilder>) -> Vec<String> {
    names(&build_commands(&spec(operations)).unwrap())
}

fn arguments_of(builder: OperationBuilder) -> Vec<ArgumentSpec> {
    build_commands(&spec(vec![builder]))
        .unwrap()
        .commands
        .remove(0)
        .arguments
}

fn flag<'a>(arguments: &'a [ArgumentSpec], name: &str) -> &'a ArgumentSpec {
    arguments
        .iter()
        .find(|argument| argument.name == name)
        .unwrap_or_else(|| {
            panic!(
                "no argument `{name}` in {:?}",
                arguments.iter().map(|a| &a.name).collect::<Vec<_>>()
            )
        })
}

use HttpMethod::{Delete, Get, Patch, Post, Put};

#[rstest]
#[case(Get, "/pets", "pets list")]
#[case(Post, "/pets", "pets create")]
#[case(Get, "/pets/{petId}", "pets get")]
#[case(Put, "/pets/{petId}", "pets replace")]
#[case(Patch, "/pets/{petId}", "pets update")]
#[case(Delete, "/pets/{petId}", "pets delete")]
#[case(Get, "/users/{userId}/orders", "users orders list")]
#[case(Get, "/users/{userId}/orders/{orderId}", "users orders get")]
#[case(Get, "/payment_methods", "payment-methods list")]
#[case(Post, "/users/{userId}/activate", "users activate")]
fn build_commands_names_operations_by_pattern(
    #[case] method: HttpMethod,
    #[case] path: &str,
    #[case] expected: &str,
) {
    assert_eq!(names_of(vec![operation(method, path)]), [expected]);
}

#[test]
fn build_commands_strips_shared_namespace_prefix() {
    let found = names_of(vec![
        operation(Get, "/api/v1/users"),
        operation(Get, "/api/v1/users/{id}"),
    ]);
    assert_eq!(found, ["users list", "users get"]);
}

#[test]
fn build_commands_treats_post_beside_get_as_create_not_action() {
    let found = names_of(vec![
        operation(Get, "/users/{id}/orders"),
        operation(Post, "/users/{id}/orders"),
    ]);
    assert_eq!(found, ["users orders list", "users orders create"]);
}

#[test]
fn build_commands_falls_back_to_operation_id_for_post_on_item() {
    assert_eq!(
        names_of(vec![operation(Post, "/jobs/{id}").id("rerunJob")]),
        ["jobs rerun-job"]
    );
}

#[test]
fn build_commands_falls_back_to_method_without_operation_id() {
    assert_eq!(names_of(vec![operation(Post, "/jobs/{id}")]), ["jobs post"]);
}

#[test]
fn build_commands_renames_pattern_conflicts_by_operation_id() {
    let found = names_of(vec![
        operation(Delete, "/users").id("deleteAllUsers"),
        operation(Delete, "/users/{id}").id("deleteUser"),
        operation(Get, "/users"),
    ]);
    assert_eq!(
        found,
        ["users delete-all-users", "users delete-user", "users list"]
    );
}

#[test]
fn build_commands_applies_hints() {
    let found = names_of(vec![
        operation(Post, "/users/{id}/archive-request").hints(Some("archive"), None),
        operation(Get, "/reports/daily").hints(None, Some(&["daily-reports"])),
        operation(Get, "/x").hints(Some("whoAmI"), Some(&["me"])),
    ]);
    assert_eq!(
        found,
        ["users archive", "daily-reports list", "me who-am-i"]
    );
}

#[test]
fn build_commands_reports_unresolvable_duplicates() {
    let error = build_commands(&spec(vec![
        operation(Delete, "/users"),
        operation(Delete, "/users/{id}"),
    ]))
    .unwrap_err();
    assert_eq!(error.conflicts.len(), 1);
    assert_eq!(error.conflicts[0].command, "users delete");
    assert_eq!(
        error.conflicts[0].operations,
        ["DELETE /users", "DELETE /users/{id}"]
    );
}

#[test]
fn build_commands_never_renames_hinted_operations() {
    let error: NamingError = build_commands(&spec(vec![
        operation(Get, "/a")
            .id("a")
            .hints(Some("same"), Some(&["x"])),
        operation(Get, "/b")
            .id("b")
            .hints(Some("same"), Some(&["x"])),
    ]))
    .unwrap_err();
    assert_eq!(error.conflicts[0].command, "x same");
}

#[test]
fn build_commands_reports_command_that_is_also_a_group() {
    let error = build_commands(&spec(vec![
        operation(Get, "/users"),
        operation(Get, "/other").hints(Some("recent"), Some(&["users", "list"])),
    ]))
    .unwrap_err();
    assert_eq!(error.conflicts[0].command, "users list");
    assert_eq!(error.conflicts[0].operations, ["GET /users", "GET /other"]);
}

#[test]
fn build_commands_keeps_servers_and_operation_details() {
    let set = build_commands(&spec(vec![operation(Get, "/pets/{petId}").id("showPet")])).unwrap();
    assert_eq!(set.servers, ["https://api.example.com"]);
    let command = &set.commands[0];
    assert_eq!(command.method, Get);
    assert_eq!(command.path_template, "/pets/{petId}");
    assert_eq!(command.operation_id.as_deref(), Some("showPet"));
}

#[test]
fn arguments_make_path_parameters_positional_in_template_order() {
    let mut builder = operation(Get, "/users/{userId}/orders/{orderId}");
    builder.0.parameters.reverse();
    let arguments = arguments_of(builder);
    let positional: Vec<_> = arguments
        .iter()
        .filter(|a| a.positional)
        .map(|a| (a.name.as_str(), &a.target))
        .collect();
    assert_eq!(
        positional,
        [
            ("user-id", &ArgumentTarget::Path("userId".into())),
            ("order-id", &ArgumentTarget::Path("orderId".into())),
        ]
    );
    assert!(arguments.iter().all(|argument| argument.required));
}

#[test]
fn arguments_turn_query_and_header_parameters_into_typed_flags() {
    let arguments = arguments_of(
        operation(Get, "/pets")
            .parameter(
                "pageSize",
                ParameterLocation::Query,
                json!({"$ref": "#/components/schemas/Limit"}),
            )
            .parameter(
                "status",
                ParameterLocation::Query,
                json!({"type": "string", "enum": ["open", "closed"]}),
            )
            .parameter(
                "tags",
                ParameterLocation::Query,
                json!({"type": "array", "items": {"type": "integer"}}),
            )
            .parameter(
                "X-Request-Id",
                ParameterLocation::Header,
                json!({"type": "string"}),
            ),
    );
    let page_size = flag(&arguments, "page-size");
    assert_eq!(page_size.value_type, ValueType::Integer);
    assert_eq!(page_size.target, ArgumentTarget::Query("pageSize".into()));
    assert_eq!(page_size.help.as_deref(), Some("pageSize help"));
    assert_eq!(
        flag(&arguments, "status").allowed_values,
        ["open", "closed"]
    );
    let tags = flag(&arguments, "tags");
    assert!(tags.multiple);
    assert_eq!(tags.value_type, ValueType::Integer);
    assert_eq!(
        flag(&arguments, "x-request-id").target,
        ArgumentTarget::Header("X-Request-Id".into())
    );
}

#[test]
fn arguments_prefix_reserved_and_duplicate_flag_names_with_location() {
    let arguments = arguments_of(
        operation(Get, "/pets")
            .parameter(
                "output",
                ParameterLocation::Query,
                json!({"type": "string"}),
            )
            .parameter("trace", ParameterLocation::Query, json!({"type": "string"}))
            .parameter(
                "trace",
                ParameterLocation::Header,
                json!({"type": "string"}),
            ),
    );
    let names: Vec<_> = arguments
        .iter()
        .map(|argument| argument.name.as_str())
        .collect();
    assert_eq!(names, ["query-output", "query-trace", "header-trace"]);
}

#[test]
fn arguments_ignore_reserved_headers() {
    let arguments = arguments_of(
        operation(Get, "/pets")
            .parameter(
                "Accept",
                ParameterLocation::Header,
                json!({"type": "string"}),
            )
            .parameter(
                "Authorization",
                ParameterLocation::Header,
                json!({"type": "string"}),
            ),
    );
    assert_eq!(arguments, []);
}

#[test]
fn arguments_expose_scalar_json_body_fields_as_optional_flags() {
    let arguments = arguments_of(operation(Post, "/customers").body(
        "application/json",
        json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {"type": "string", "description": "Full name"},
                "age": {"type": "integer"},
                "id": {"type": "string", "readOnly": true},
                "address": {"type": "object"},
                "tags": {"type": "array", "items": {"type": "string"}}
            }
        }),
    ));
    let names: Vec<_> = arguments
        .iter()
        .map(|argument| argument.name.as_str())
        .collect();
    // Flags follow the spec's property order.
    assert_eq!(names, ["name", "age", "tags"]);
    let name = flag(&arguments, "name");
    assert_eq!(name.target, ArgumentTarget::BodyField("name".into()));
    assert!(!name.required);
    assert_eq!(name.help.as_deref(), Some("Full name"));
    assert!(flag(&arguments, "tags").multiple);
}

#[test]
fn arguments_prefix_body_fields_that_clash_with_parameters() {
    let arguments = arguments_of(
        operation(Post, "/customers")
            .parameter("name", ParameterLocation::Query, json!({"type": "string"}))
            .body("application/json", json!({"type": "object", "properties": {"name": {"type": "string"}, "describe": {"type": "string"}}})),
    );
    assert_eq!(
        flag(&arguments, "name").target,
        ArgumentTarget::Query("name".into())
    );
    assert_eq!(
        flag(&arguments, "body-name").target,
        ArgumentTarget::BodyField("name".into())
    );
    assert_eq!(
        flag(&arguments, "body-describe").target,
        ArgumentTarget::BodyField("describe".into())
    );
}

#[test]
fn arguments_skip_body_fields_for_non_json_bodies() {
    let arguments = arguments_of(operation(Post, "/upload").body(
        "text/plain",
        json!({"type": "object", "properties": {"a": {"type": "string"}}}),
    ));
    assert_eq!(arguments, []);
}

#[test]
fn body_schema_is_inlined_for_describe() {
    let mut spec = spec(vec![operation(Post, "/pets").body(
        "application/json",
        json!({"$ref": "#/components/schemas/NewPet"}),
    )]);
    spec.schemas.insert(
        "NewPet".into(),
        json!({"type": "object", "properties": {"name": {"type": "string"}}}),
    );
    let set = build_commands(&spec).unwrap();
    let body = set.commands[0].body.clone().unwrap();
    assert_eq!(
        body.schema,
        Some(json!({"type": "object", "properties": {"name": {"type": "string"}}}))
    );
    assert_eq!(
        flag(&set.commands[0].arguments, "name").target,
        ArgumentTarget::BodyField("name".into())
    );
}

#[test]
fn arguments_make_binary_multipart_fields_file_flags() {
    let arguments = arguments_of(operation(Post, "/files").body(
        "multipart/form-data",
        json!({"type": "object", "properties": {
            "title": {"type": "string"},
            "document": {"type": "string", "format": "binary"},
            "pages": {"type": "array", "items": {"type": "string", "contentMediaType": "image/png"}}
        }}),
    ));
    assert_eq!(flag(&arguments, "title").value_type, ValueType::String);
    assert_eq!(flag(&arguments, "document").value_type, ValueType::File);
    let pages = flag(&arguments, "pages");
    assert_eq!((pages.value_type, pages.multiple), (ValueType::File, true));
}

#[test]
fn binary_fields_outside_multipart_stay_strings() {
    let arguments = arguments_of(operation(Post, "/blobs").body(
        "application/json",
        json!({"type": "object", "properties": {"data": {"type": "string", "format": "binary"}}}),
    ));
    assert_eq!(flag(&arguments, "data").value_type, ValueType::String);
}
