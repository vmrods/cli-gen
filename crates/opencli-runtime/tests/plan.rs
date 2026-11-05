// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use opencli_core::{ArgumentSpec, ArgumentTarget, BodySpec, CommandSpec, HttpMethod, ValueType};
use opencli_runtime::{PlanError, plan_request, typed_value};
use rstest::rstest;
use serde_json::{Value, json};

fn argument(name: &str, target: ArgumentTarget, value_type: ValueType) -> ArgumentSpec {
    ArgumentSpec {
        name: name.to_owned(),
        positional: matches!(target, ArgumentTarget::Path(_)),
        target,
        required: false,
        help: None,
        value_type,
        multiple: false,
        allowed_values: Vec::new(),
        array_delimiter: None,
    }
}

fn command(arguments: Vec<ArgumentSpec>, body: Option<BodySpec>) -> CommandSpec {
    CommandSpec {
        path: vec!["users".into(), "orders".into(), "create".into()],
        method: HttpMethod::Post,
        path_template: "/users/{userId}/orders".into(),
        operation_id: None,
        summary: None,
        description: None,
        deprecated: false,
        arguments,
        body,
        security: Vec::new(),
    }
}

fn json_body(required: bool) -> BodySpec {
    BodySpec {
        content_type: "application/json".into(),
        required,
        description: None,
        schema: None,
    }
}

fn given(values: &[&str]) -> Vec<String> {
    values.iter().map(|&value| value.to_owned()).collect()
}

fn standard_arguments() -> Vec<ArgumentSpec> {
    let mut tags = argument(
        "tag",
        ArgumentTarget::Query("tag".into()),
        ValueType::String,
    );
    tags.multiple = true;
    let mut labels = argument(
        "labels",
        ArgumentTarget::BodyField("labels".into()),
        ValueType::String,
    );
    labels.multiple = true;
    vec![
        argument(
            "user-id",
            ArgumentTarget::Path("userId".into()),
            ValueType::String,
        ),
        tags,
        argument(
            "x-trace",
            ArgumentTarget::Header("X-Trace".into()),
            ValueType::String,
        ),
        argument(
            "session",
            ArgumentTarget::Cookie("session".into()),
            ValueType::String,
        ),
        argument(
            "quantity",
            ArgumentTarget::BodyField("quantity".into()),
            ValueType::Integer,
        ),
        argument(
            "gift",
            ArgumentTarget::BodyField("gift".into()),
            ValueType::Boolean,
        ),
        labels,
    ]
}

#[test]
fn plan_request_routes_each_argument_to_its_location() {
    let command = command(standard_arguments(), Some(json_body(true)));
    let values = [
        Some(given(&["a b/c"])),
        Some(given(&["red", "blue"])),
        Some(given(&["t-1"])),
        Some(given(&["s-1"])),
        Some(given(&["3"])),
        Some(given(&["true"])),
        Some(given(&["x", "y"])),
    ];
    let plan = plan_request(&command, Some("https://api.example.com/v1/"), &values, None).unwrap();

    assert_eq!(plan.method, HttpMethod::Post);
    assert_eq!(
        plan.url,
        "https://api.example.com/v1/users/a%20b%2Fc/orders"
    );
    assert_eq!(
        plan.query,
        [("tag".into(), "red".into()), ("tag".into(), "blue".into())]
    );
    assert_eq!(plan.headers, [("X-Trace".into(), "t-1".into())]);
    assert_eq!(plan.cookies, [("session".into(), "s-1".into())]);
    let body = plan.body.unwrap();
    assert_eq!(body.content_type, "application/json");
    assert_eq!(
        body.value,
        json!({"quantity": 3, "gift": true, "labels": ["x", "y"]})
    );
}

#[test]
fn plan_request_uses_relative_path_without_servers() {
    let command = command(standard_arguments(), None);
    let mut values = vec![None; 7];
    values[0] = Some(given(&["42"]));
    let plan = plan_request(&command, None, &values, None).unwrap();
    assert_eq!(plan.url, "/users/42/orders");
    assert_eq!(plan.body, None);
}

#[test]
fn plan_request_merges_body_flags_over_body_input() {
    let command = command(standard_arguments(), Some(json_body(true)));
    let mut values = vec![None; 7];
    values[0] = Some(given(&["1"]));
    values[4] = Some(given(&["5"]));
    let plan = plan_request(
        &command,
        None,
        &values,
        Some(json!({"quantity": 1, "note": "hi"})),
    )
    .unwrap();
    assert_eq!(
        plan.body.unwrap().value,
        json!({"quantity": 5, "note": "hi"})
    );
}

#[test]
fn plan_request_passes_non_object_body_through() {
    let command = command(standard_arguments(), Some(json_body(true)));
    let mut values = vec![None; 7];
    values[0] = Some(given(&["1"]));
    let plan = plan_request(&command, None, &values, Some(json!([1, 2]))).unwrap();
    assert_eq!(plan.body.unwrap().value, json!([1, 2]));
}

#[test]
fn plan_request_rejects_body_flags_with_non_object_body() {
    let command = command(standard_arguments(), Some(json_body(true)));
    let mut values = vec![None; 7];
    values[0] = Some(given(&["1"]));
    values[4] = Some(given(&["5"]));
    assert_eq!(
        plan_request(&command, None, &values, Some(json!([1]))),
        Err(PlanError::BodyNotObject)
    );
}

#[rstest]
#[case(true, Err(PlanError::MissingBody))]
#[case(false, Ok(None))]
fn plan_request_handles_absent_body(
    #[case] required: bool,
    #[case] expected: Result<Option<Value>, PlanError>,
) {
    let command = command(standard_arguments(), Some(json_body(required)));
    let mut values = vec![None; 7];
    values[0] = Some(given(&["1"]));
    let result =
        plan_request(&command, None, &values, None).map(|plan| plan.body.map(|body| body.value));
    assert_eq!(result, expected);
}

#[test]
fn plan_request_requires_path_values() {
    let command = command(standard_arguments(), None);
    assert_eq!(
        plan_request(&command, None, &vec![None; 7], None),
        Err(PlanError::MissingPathValue("userId".into()))
    );
}

#[test]
fn plan_request_reports_badly_typed_body_field() {
    let command = command(standard_arguments(), Some(json_body(true)));
    let mut values = vec![None; 7];
    values[0] = Some(given(&["1"]));
    values[4] = Some(given(&["many"]));
    assert_eq!(
        plan_request(&command, None, &values, None),
        Err(PlanError::InvalidValue {
            argument: "quantity".into(),
            value: "many".into(),
            expected: "integer"
        })
    );
}

#[rstest]
#[case("text", ValueType::String, json!("text"))]
#[case("-7", ValueType::Integer, json!(-7))]
#[case("2.5", ValueType::Number, json!(2.5))]
#[case("false", ValueType::Boolean, json!(false))]
fn typed_value_converts_to_declared_type(
    #[case] raw: &str,
    #[case] value_type: ValueType,
    #[case] expected: Value,
) {
    assert_eq!(typed_value("x", raw, value_type), Ok(expected));
}

#[rstest]
#[case("1.5", ValueType::Integer)]
#[case("NaN", ValueType::Number)]
#[case("yes", ValueType::Boolean)]
fn typed_value_rejects_mismatched_input(#[case] raw: &str, #[case] value_type: ValueType) {
    assert!(matches!(
        typed_value("x", raw, value_type),
        Err(PlanError::InvalidValue { .. })
    ));
}

#[test]
fn plan_request_joins_delimited_array_values() {
    let mut csv = argument(
        "ids",
        ArgumentTarget::Query("ids".into()),
        ValueType::String,
    );
    csv.multiple = true;
    csv.array_delimiter = Some(',');
    let mut header = argument(
        "x-tags",
        ArgumentTarget::Header("X-Tags".into()),
        ValueType::String,
    );
    header.multiple = true;
    header.array_delimiter = Some('|');
    let mut user_id = argument(
        "user-id",
        ArgumentTarget::Path("userId".into()),
        ValueType::String,
    );
    user_id.positional = true;
    let command = command(vec![user_id, csv, header], None);
    let values = [
        Some(given(&["1"])),
        Some(given(&["a", "b", "c"])),
        Some(given(&["x", "y"])),
    ];
    let plan = plan_request(&command, None, &values, None).unwrap();
    assert_eq!(plan.query, [("ids".into(), "a,b,c".into())]);
    assert_eq!(plan.headers, [("X-Tags".into(), "x|y".into())]);
}
