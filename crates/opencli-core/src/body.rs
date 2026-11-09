//! Request bodies against their schema: which fields are secrets, and whether
//! a body matches before it is sent. Schemas are OpenAPI 3.0 shaped (other
//! versions are normalized first, ADR 0001) with local `$ref`s inlined.

use std::fmt;

use serde_json::Value;

/// Stops a badly wrong body from flooding the terminal.
const MAX_PROBLEMS: usize = 20;
const ROOT: &str = "body";
/// Lowercased, punctuation-free name parts that mark a secret.
const SECRET_NAME_PARTS: &[&str] = &["password", "passwd", "secret", "apikey", "privatekey"];
const SECRET_NAME_SUFFIX: &str = "token";

/// One way a body fails its schema, at a path like `body.address.city`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodyProblem {
    pub path: String,
    pub message: String,
}

impl fmt::Display for BodyProblem {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.path, self.message)
    }
}

/// Whether a body field holds a secret, so it must not become a command-line
/// flag (where it would land in shell history and process lists). True for
/// `format: password`, and for string fields named like a password, secret,
/// API key or token, which many generated specs leave unmarked.
#[must_use]
pub fn is_secret_field(name: &str, schema: &Value) -> bool {
    if schema.get("format").and_then(Value::as_str) == Some("password") {
        return true;
    }
    if !matches!(
        schema.get("type").and_then(Value::as_str),
        None | Some("string")
    ) {
        return false;
    }
    let normalized: String = name
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    SECRET_NAME_PARTS
        .iter()
        .any(|part| normalized.contains(part))
        || normalized.ends_with(SECRET_NAME_SUFFIX)
}

/// Checks `value` against `schema`: types, `nullable`, `enum`, `required`
/// (except read-only fields), nested properties and array items, and
/// `allOf`/`anyOf`/`oneOf` (any matching branch is enough). Unknown fields
/// and string formats are allowed, so servers stay the judge of the rest.
#[must_use]
pub fn validate_body(schema: &Value, value: &Value) -> Vec<BodyProblem> {
    let mut problems = Vec::new();
    check(schema, value, ROOT, &mut problems);
    problems.truncate(MAX_PROBLEMS);
    problems
}

fn check(schema: &Value, value: &Value, path: &str, problems: &mut Vec<BodyProblem>) {
    // A `$ref` left in place marks a recursive schema cut short: nothing to check.
    if problems.len() >= MAX_PROBLEMS || schema.get("$ref").is_some() {
        return;
    }
    for branch in schema_list(schema, "allOf") {
        check(branch, value, path, problems);
    }
    let alternatives: Vec<&Value> = schema_list(schema, "anyOf")
        .chain(schema_list(schema, "oneOf"))
        .collect();
    if !alternatives.is_empty() && !alternatives.iter().any(|branch| matches(branch, value)) {
        problems.push(problem(path, "doesn't match any of the allowed shapes"));
        return;
    }
    let types = declared_types(schema);
    if value.is_null() {
        let nullable = schema.get("nullable").and_then(Value::as_bool) == Some(true)
            || types.is_empty()
            || types.contains(&"null");
        if !nullable {
            problems.push(problem(path, "must not be null"));
        }
        return;
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        let listed: Vec<String> = allowed.iter().map(Value::to_string).collect();
        problems.push(problem(
            path,
            &format!("must be one of {}", listed.join(", ")),
        ));
        return;
    }
    if !types.is_empty() && !types.iter().any(|name| has_type(value, name)) {
        problems.push(problem(
            path,
            &format!("expected {}, got {}", types.join(" or "), type_of(value)),
        ));
        return;
    }
    match value {
        Value::Object(fields) => {
            let properties = schema.get("properties").and_then(Value::as_object);
            for name in schema_list(schema, "required").filter_map(Value::as_str) {
                let read_only = properties
                    .and_then(|properties| properties.get(name))
                    .and_then(|property| property.get("readOnly"))
                    .and_then(Value::as_bool)
                    == Some(true);
                if !read_only && !fields.contains_key(name) {
                    problems.push(problem(
                        &format!("{path}.{name}"),
                        "required field is missing",
                    ));
                }
            }
            for (name, property) in properties.into_iter().flatten() {
                if let Some(field) = fields.get(name) {
                    check(property, field, &format!("{path}.{name}"), problems);
                }
            }
        }
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    check(item_schema, item, &format!("{path}[{index}]"), problems);
                }
            }
        }
        _ => {}
    }
}

fn matches(schema: &Value, value: &Value) -> bool {
    let mut problems = Vec::new();
    check(schema, value, ROOT, &mut problems);
    problems.is_empty()
}

fn schema_list<'a>(schema: &'a Value, keyword: &str) -> impl Iterator<Item = &'a Value> {
    schema
        .get(keyword)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
}

/// `type` as a list (3.1 allows arrays); an object schema without `type`
/// still counts as an object when it lists properties.
fn declared_types(schema: &Value) -> Vec<&str> {
    match schema.get("type") {
        Some(Value::String(name)) => vec![name.as_str()],
        Some(Value::Array(names)) => names.iter().filter_map(Value::as_str).collect(),
        _ if schema.get("properties").is_some() => vec!["object"],
        _ => Vec::new(),
    }
}

fn has_type(value: &Value, name: &str) -> bool {
    match name {
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        "number" => value.is_number(),
        "integer" => {
            value.is_i64()
                || value.is_u64()
                || value.as_f64().is_some_and(|number| number.fract() == 0.0)
        }
        "null" => value.is_null(),
        // An unknown type name is the spec's problem, not the body's.
        _ => true,
    }
}

const fn type_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn problem(path: &str, message: &str) -> BodyProblem {
    BodyProblem {
        path: path.to_owned(),
        message: message.to_owned(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    fn user_schema() -> Value {
        json!({
            "type": "object",
            "required": ["email", "id", "role"],
            "properties": {
                "id": {"type": "integer", "readOnly": true},
                "email": {"type": "string"},
                "age": {"type": "integer"},
                "nickname": {"type": "string", "nullable": true},
                "role": {"type": "string", "enum": ["admin", "member"]},
                "address": {
                    "type": "object",
                    "required": ["city"],
                    "properties": {"city": {"type": "string"}}
                },
                "tags": {"type": "array", "items": {"type": "string"}}
            }
        })
    }

    fn messages(schema: &Value, value: &Value) -> Vec<String> {
        validate_body(schema, value)
            .iter()
            .map(ToString::to_string)
            .collect()
    }

    #[test]
    fn validate_body_accepts_a_matching_body_with_unknown_fields() {
        let body = json!({
            "email": "a@b.c", "role": "admin", "age": 30.0, "nickname": null,
            "address": {"city": "Lisbon"}, "tags": ["x"], "extra": true
        });
        assert_eq!(messages(&user_schema(), &body), Vec::<String>::new());
    }

    #[test]
    fn validate_body_reports_every_problem_with_its_path() {
        let body = json!({
            "email": 5, "age": 1.5, "address": {}, "tags": ["ok", 2], "role": "owner"
        });
        assert_eq!(
            messages(&user_schema(), &body),
            [
                "body.email: expected string, got number",
                "body.age: expected integer, got number",
                "body.role: must be one of \"admin\", \"member\"",
                "body.address.city: required field is missing",
                "body.tags[1]: expected string, got number",
            ]
        );
    }

    #[test]
    fn validate_body_skips_read_only_required_fields_but_not_others() {
        assert_eq!(
            messages(&user_schema(), &json!({"email": "a@b.c"})),
            ["body.role: required field is missing"]
        );
    }

    #[test]
    fn validate_body_rejects_null_unless_nullable() {
        assert_eq!(
            messages(&user_schema(), &json!({"email": null, "role": "admin"})),
            ["body.email: must not be null"]
        );
    }

    #[test]
    fn validate_body_checks_the_top_level_type() {
        assert_eq!(
            messages(&user_schema(), &json!([1])),
            ["body: expected object, got array"]
        );
    }

    #[test]
    fn validate_body_applies_all_of_and_any_branch_of_one_of() {
        let schema = json!({
            "allOf": [{"type": "object", "required": ["kind"]}],
            "oneOf": [{"type": "object", "properties": {"kind": {"enum": ["a"]}}},
                      {"type": "object", "properties": {"kind": {"enum": ["b"]}}}]
        });
        assert_eq!(
            messages(&schema, &json!({"kind": "b"})),
            Vec::<String>::new()
        );
        assert_eq!(
            messages(&schema, &json!({"kind": "c"})),
            ["body: doesn't match any of the allowed shapes"]
        );
        assert_eq!(
            messages(&schema, &json!({})),
            ["body.kind: required field is missing"]
        );
    }

    #[test]
    fn validate_body_ignores_schemas_cut_short_by_recursion() {
        let schema = json!({"$ref": "#/components/schemas/Node"});
        assert_eq!(messages(&schema, &json!(42)), Vec::<String>::new());
    }

    #[test]
    fn validate_body_caps_the_number_of_problems() {
        let schema = json!({"type": "array", "items": {"type": "string"}});
        let body = Value::Array(vec![json!(1); 50]);
        assert_eq!(validate_body(&schema, &body).len(), MAX_PROBLEMS);
    }

    #[rstest]
    #[case("password", json!({"type": "string"}), true)]
    #[case("confirmPassword", json!({"type": "string"}), true)]
    #[case("client_secret", json!({"type": "string"}), true)]
    #[case("api-key", json!({}), true)]
    #[case("refreshToken", json!({"type": "string"}), true)]
    #[case("pin", json!({"type": "string", "format": "password"}), true)]
    #[case("passwordExpired", json!({"type": "boolean"}), false)]
    #[case("tokenType", json!({"type": "string"}), false)]
    #[case("email", json!({"type": "string"}), false)]
    fn is_secret_field_matches_formats_and_names(
        #[case] name: &str,
        #[case] schema: Value,
        #[case] expected: bool,
    ) {
        assert_eq!(is_secret_field(name, &schema), expected);
    }
}
