//! Asks for an operation's inputs in a terminal: every argument and body
//! field with `--interactive`, otherwise only the required ones left out
//! (ADR 0002). Scripts and agents, with no terminal, are never asked.

use std::collections::BTreeSet;

use opencli_core::{
    ArgumentSpec, ArgumentTarget, CommandSpec, ValueType, is_form_media_type, is_json_media_type,
    is_secret_field,
};
use opencli_runtime::{PlanError, typed_value};
use serde_json::{Map, Value};

use crate::error::CliError;
use crate::prompt;

const LIST_SEPARATOR: char = ',';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillMode {
    /// `--interactive`: every argument and body field, optional ones skippable.
    Everything,
    /// A terminal without `--interactive`: only required values not given.
    MissingRequired,
}

/// Where answers come from: the terminal, or a script in tests.
pub trait Asker {
    fn ask(&mut self, question: &str) -> Result<String, CliError>;
    fn ask_secret(&mut self, question: &str) -> Result<String, CliError>;
    fn say(&mut self, message: &str) -> Result<(), CliError>;
}

pub struct Terminal;

impl Asker for Terminal {
    fn ask(&mut self, question: &str) -> Result<String, CliError> {
        prompt::ask_line(question)
    }

    fn ask_secret(&mut self, question: &str) -> Result<String, CliError> {
        prompt::ask_secret(question)
    }

    fn say(&mut self, message: &str) -> Result<(), CliError> {
        prompt::say(message)
    }
}

/// Fills `values[i]` for `command.arguments[i]` where nothing was given.
/// Fields of a JSON or form body are left to [`body`], which knows from the
/// schema which are required or secret, and asks in the schema's order.
pub fn arguments(
    command: &CommandSpec,
    values: &mut [Option<Vec<String>>],
    mode: FillMode,
    asker: &mut impl Asker,
) -> Result<(), CliError> {
    let body_asks_fields = command
        .body
        .as_ref()
        .is_some_and(|spec| spec.schema.is_some() && is_fillable(&spec.content_type));
    for (argument, slot) in command.arguments.iter().zip(values.iter_mut()) {
        let body_field = matches!(argument.target, ArgumentTarget::BodyField(_));
        if slot.is_some()
            || (body_field && body_asks_fields)
            || (mode == FillMode::MissingRequired && !argument.required)
        {
            continue;
        }
        *slot = ask_argument(argument, asker)?;
    }
    Ok(())
}

fn ask_argument(
    argument: &ArgumentSpec,
    asker: &mut impl Asker,
) -> Result<Option<Vec<String>>, CliError> {
    let question = argument_question(argument);
    loop {
        let answer = asker.ask(&question)?;
        if answer.is_empty() {
            if argument.required {
                asker.say("A value is required.")?;
                continue;
            }
            return Ok(None);
        }
        let values: Vec<String> = if argument.multiple {
            split_list(&answer)
        } else {
            vec![answer]
        };
        match values
            .iter()
            .find_map(|value| argument_problem(argument, value))
        {
            Some(problem) => asker.say(&problem)?,
            None => return Ok(Some(values)),
        }
    }
}

fn argument_question(argument: &ArgumentSpec) -> String {
    let location = match &argument.target {
        ArgumentTarget::Path(_) => "path",
        ArgumentTarget::Query(_) => "query",
        ArgumentTarget::Header(_) => "header",
        ArgumentTarget::Cookie(_) => "cookie",
        ArgumentTarget::BodyField(_) => "body",
    };
    let mut details = vec![location.to_owned(), value_type_name(argument.value_type)];
    if !argument.allowed_values.is_empty() {
        details.push(format!("one of {}", argument.allowed_values.join(", ")));
    }
    if argument.multiple {
        details.push("comma-separated".to_owned());
    }
    question(&argument.name, &details, argument.required)
}

fn argument_problem(argument: &ArgumentSpec, value: &str) -> Option<String> {
    if !argument.allowed_values.is_empty() && !argument.allowed_values.iter().any(|v| v == value) {
        return Some(format!(
            "`{value}` is not one of {}.",
            argument.allowed_values.join(", ")
        ));
    }
    typed_value(&argument.name, value, argument.value_type)
        .err()
        .map(|error| answer_problem(value, &error))
}

/// Completes a JSON or form body from its schema. Fields already in `body`,
/// or set by a body-field flag (`flagged`), are not asked for again.
pub fn body(
    command: &CommandSpec,
    body: Option<Value>,
    flagged: &BTreeSet<String>,
    mode: FillMode,
    asker: &mut impl Asker,
) -> Result<Option<Value>, CliError> {
    let Some(spec) = &command.body else {
        return Ok(body);
    };
    let Some(schema) = &spec.schema else {
        return Ok(body);
    };
    let fillable = is_fillable(&spec.content_type);
    let existing = match body {
        None => Map::new(),
        Some(Value::Object(fields)) => fields,
        // A non-object body (a string, an array) has no fields to fill in.
        Some(other) => return Ok(Some(other)),
    };
    let nothing_given = existing.is_empty() && flagged.is_empty();
    if !fillable || (mode == FillMode::MissingRequired && nothing_given && !spec.required) {
        return Ok((!existing.is_empty()).then_some(Value::Object(existing)));
    }
    let mut filler = Filler { mode, asker };
    let fields = filler.object(schema, existing, "", flagged)?;
    Ok((spec.required || !fields.is_empty()).then_some(Value::Object(fields)))
}

/// Bodies whose fields can be asked for one by one.
fn is_fillable(content_type: &str) -> bool {
    is_json_media_type(content_type) || is_form_media_type(content_type)
}

struct Filler<'a, A: Asker> {
    mode: FillMode,
    asker: &'a mut A,
}

impl<A: Asker> Filler<'_, A> {
    fn object(
        &mut self,
        schema: &Value,
        mut fields: Map<String, Value>,
        prefix: &str,
        skip: &BTreeSet<String>,
    ) -> Result<Map<String, Value>, CliError> {
        let (properties, required) = object_shape(schema);
        for (name, property) in properties {
            let read_only = property.get("readOnly").and_then(Value::as_bool) == Some(true);
            if read_only || fields.contains_key(&name) || skip.contains(&name) {
                continue;
            }
            let is_required = required.contains(&name);
            if self.mode == FillMode::MissingRequired && !is_required {
                continue;
            }
            let label = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}.{name}")
            };
            if let Some(value) = self.value(&name, &label, &property, is_required)? {
                fields.insert(name, value);
            }
        }
        Ok(fields)
    }

    fn value(
        &mut self,
        name: &str,
        label: &str,
        schema: &Value,
        required: bool,
    ) -> Result<Option<Value>, CliError> {
        match schema_kind(schema) {
            Kind::Object => {
                if !required && !self.yes(&format!("Fill in `{label}`?"))? {
                    return Ok(None);
                }
                let fields = self.object(schema, Map::new(), label, &BTreeSet::new())?;
                Ok(Some(Value::Object(fields)))
            }
            Kind::ObjectList(item) => {
                let mut items = Vec::new();
                while self.yes(&format!("Add an item to `{label}`?"))? {
                    let label = format!("{label}[{}]", items.len());
                    items.push(Value::Object(self.object(
                        item,
                        Map::new(),
                        &label,
                        &BTreeSet::new(),
                    )?));
                }
                Ok((required || !items.is_empty()).then_some(Value::Array(items)))
            }
            Kind::ScalarList(item) => self.scalars(label, item, required),
            Kind::Scalar => self.scalar(name, label, schema, required),
            Kind::Json => self.json(label, required),
        }
    }

    fn scalar(
        &mut self,
        name: &str,
        label: &str,
        schema: &Value,
        required: bool,
    ) -> Result<Option<Value>, CliError> {
        let secret = is_secret_field(name, schema);
        let question = question(label, &schema_details(schema, secret), required);
        loop {
            let answer = if secret {
                self.asker.ask_secret(&question)?
            } else {
                self.asker.ask(&question)?
            };
            if answer.is_empty() {
                if required {
                    self.asker.say("A value is required.")?;
                    continue;
                }
                return Ok(None);
            }
            match scalar_value(label, &answer, schema) {
                Ok(value) => return Ok(Some(value)),
                Err(problem) => self.asker.say(&problem)?,
            }
        }
    }

    fn scalars(
        &mut self,
        label: &str,
        item: &Value,
        required: bool,
    ) -> Result<Option<Value>, CliError> {
        let mut details = schema_details(item, false);
        details.push("comma-separated".to_owned());
        let question = question(label, &details, required);
        loop {
            let answer = self.asker.ask(&question)?;
            if answer.is_empty() {
                return Ok(required.then(|| Value::Array(Vec::new())));
            }
            let parsed: Result<Vec<Value>, String> = split_list(&answer)
                .iter()
                .map(|value| scalar_value(label, value, item))
                .collect();
            match parsed {
                Ok(values) => return Ok(Some(Value::Array(values))),
                Err(problem) => self.asker.say(&problem)?,
            }
        }
    }

    /// Shapes a prompt can't walk (`oneOf`, untyped) take a JSON value.
    fn json(&mut self, label: &str, required: bool) -> Result<Option<Value>, CliError> {
        let question = question(label, &["JSON".to_owned()], required);
        loop {
            let answer = self.asker.ask(&question)?;
            if answer.is_empty() {
                if required {
                    self.asker.say("A value is required.")?;
                    continue;
                }
                return Ok(None);
            }
            match serde_json::from_str(&answer) {
                Ok(value) => return Ok(Some(value)),
                Err(error) => self.asker.say(&format!("Not valid JSON: {error}."))?,
            }
        }
    }

    fn yes(&mut self, question: &str) -> Result<bool, CliError> {
        let answer = self.asker.ask(&format!("{question} [y/N] "))?;
        Ok(matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
    }
}

enum Kind<'a> {
    Object,
    ObjectList(&'a Value),
    ScalarList(&'a Value),
    Scalar,
    Json,
}

fn schema_kind(schema: &Value) -> Kind<'_> {
    match schema.get("type").and_then(Value::as_str) {
        Some("object") => Kind::Object,
        Some("array") => match schema.get("items") {
            Some(item) if matches!(schema_kind(item), Kind::Object) => Kind::ObjectList(item),
            Some(item) if matches!(schema_kind(item), Kind::Scalar) => Kind::ScalarList(item),
            _ => Kind::Json,
        },
        Some("string" | "integer" | "number" | "boolean") => Kind::Scalar,
        None if schema.get("properties").is_some() || schema.get("allOf").is_some() => Kind::Object,
        None if schema.get("enum").is_some() => Kind::Scalar,
        _ => Kind::Json,
    }
}

/// Properties and required names of an object schema, merged over `allOf`.
fn object_shape(schema: &Value) -> (Vec<(String, Value)>, BTreeSet<String>) {
    let mut properties: Vec<(String, Value)> = Vec::new();
    let mut required = BTreeSet::new();
    let parts = std::iter::once(schema).chain(
        schema
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten(),
    );
    for part in parts {
        for (name, property) in part
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            if !properties.iter().any(|(existing, _)| existing == name) {
                properties.push((name.clone(), property.clone()));
            }
        }
        required.extend(
            part.get("required")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned),
        );
    }
    (properties, required)
}

fn schema_details(schema: &Value, secret: bool) -> Vec<String> {
    let mut details = vec![
        schema
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("string")
            .to_owned(),
    ];
    if let Some(allowed) = enum_texts(schema) {
        details.push(format!("one of {}", allowed.join(", ")));
    }
    if secret {
        details.push("hidden".to_owned());
    }
    details
}

fn scalar_value(label: &str, raw: &str, schema: &Value) -> Result<Value, String> {
    if let Some(allowed) = enum_texts(schema)
        && !allowed.iter().any(|value| value == raw)
    {
        return Err(format!("`{raw}` is not one of {}.", allowed.join(", ")));
    }
    let value_type = match schema.get("type").and_then(Value::as_str) {
        Some("integer") => ValueType::Integer,
        Some("number") => ValueType::Number,
        Some("boolean") => ValueType::Boolean,
        _ => ValueType::String,
    };
    typed_value(label, raw, value_type).map_err(|error| answer_problem(raw, &error))
}

/// Phrased for a prompt: the plan error names a `--flag`, which the person
/// answering may never have typed.
fn answer_problem(raw: &str, error: &PlanError) -> String {
    match error {
        PlanError::InvalidValue { expected, .. } => format!("`{raw}` is not a valid {expected}."),
        other => other.to_string(),
    }
}

fn enum_texts(schema: &Value) -> Option<Vec<String>> {
    let values = schema.get("enum").and_then(Value::as_array)?;
    Some(
        values
            .iter()
            .map(|value| match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            })
            .collect(),
    )
}

fn question(label: &str, details: &[String], required: bool) -> String {
    let skip = if required { "" } else { " [Enter to skip]" };
    format!("{label} ({}){skip}: ", details.join(", "))
}

fn value_type_name(value_type: ValueType) -> String {
    let name = match value_type {
        ValueType::String => "string",
        ValueType::Integer => "integer",
        ValueType::Number => "number",
        ValueType::Boolean => "boolean",
        ValueType::File => "file path",
    };
    name.to_owned()
}

fn split_list(answer: &str) -> Vec<String> {
    answer
        .split(LIST_SEPARATOR)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::VecDeque;

    use opencli_core::{BodySpec, HttpMethod};
    use serde_json::json;

    use super::*;

    /// Answers questions from a script and records what was asked.
    #[derive(Default)]
    struct Script {
        answers: VecDeque<&'static str>,
        asked: Vec<String>,
        secret: Vec<String>,
        said: Vec<String>,
    }

    impl Script {
        fn new(answers: &[&'static str]) -> Self {
            Self {
                answers: answers.iter().copied().collect(),
                ..Self::default()
            }
        }

        fn next(&mut self) -> Result<String, CliError> {
            self.answers
                .pop_front()
                .map(str::to_owned)
                .ok_or_else(|| CliError::Internal("the script ran out of answers".to_owned()))
        }
    }

    impl Asker for Script {
        fn ask(&mut self, question: &str) -> Result<String, CliError> {
            self.asked.push(question.to_owned());
            self.next()
        }

        fn ask_secret(&mut self, question: &str) -> Result<String, CliError> {
            self.secret.push(question.to_owned());
            self.next()
        }

        fn say(&mut self, message: &str) -> Result<(), CliError> {
            self.said.push(message.to_owned());
            Ok(())
        }
    }

    fn argument(name: &str, target: ArgumentTarget, required: bool) -> ArgumentSpec {
        ArgumentSpec {
            name: name.to_owned(),
            target,
            required,
            help: None,
            value_type: ValueType::String,
            multiple: false,
            allowed_values: Vec::new(),
            array_delimiter: None,
        }
    }

    fn command(arguments: Vec<ArgumentSpec>, body: Option<BodySpec>) -> CommandSpec {
        CommandSpec {
            path: vec!["accounts".to_owned(), "create".to_owned()],
            method: HttpMethod::Post,
            path_template: "/accounts".to_owned(),
            operation_id: None,
            summary: None,
            description: None,
            deprecated: false,
            arguments,
            body,
            security: Vec::new(),
        }
    }

    fn json_body(schema: Value, required: bool) -> BodySpec {
        BodySpec {
            content_type: "application/json".to_owned(),
            required,
            description: None,
            schema: Some(schema),
        }
    }

    fn account_schema() -> Value {
        json!({
            "type": "object",
            "required": ["email", "password"],
            "properties": {
                "id": {"type": "integer", "readOnly": true},
                "email": {"type": "string"},
                "password": {"type": "string"},
                "age": {"type": "integer"},
                "role": {"type": "string", "enum": ["admin", "member"]},
                "address": {
                    "type": "object",
                    "required": ["city"],
                    "properties": {"city": {"type": "string"}, "zip": {"type": "string"}}
                },
                "tags": {"type": "array", "items": {"type": "string"}},
                "contacts": {
                    "type": "array",
                    "items": {"type": "object", "properties": {"phone": {"type": "string"}}}
                },
                "settings": {"oneOf": [{"type": "object"}, {"type": "string"}]}
            }
        })
    }

    #[test]
    fn arguments_ask_only_for_missing_required_ones_and_retry_bad_values() {
        let mut limit = argument("limit", ArgumentTarget::Query("limit".to_owned()), true);
        limit.value_type = ValueType::Integer;
        let command = command(
            vec![
                argument("pet-id", ArgumentTarget::Path("petId".to_owned()), true),
                limit,
                argument("tag", ArgumentTarget::Query("tag".to_owned()), false),
            ],
            None,
        );
        let mut values = vec![Some(vec!["7".to_owned()]), None, None];
        let mut script = Script::new(&["", "ten", "10"]);
        arguments(
            &command,
            &mut values,
            FillMode::MissingRequired,
            &mut script,
        )
        .unwrap();
        assert_eq!(
            values,
            [
                Some(vec!["7".to_owned()]),
                Some(vec!["10".to_owned()]),
                None
            ]
        );
        assert_eq!(script.asked[0], "limit (query, integer): ");
        assert_eq!(script.said[0], "A value is required.");
        assert!(
            script.said[1].contains("not a valid integer"),
            "{:?}",
            script.said
        );
    }

    #[test]
    fn arguments_in_everything_mode_ask_optional_ones_and_split_lists() {
        let mut status = argument("status", ArgumentTarget::Query("status".to_owned()), false);
        status.multiple = true;
        status.allowed_values = vec!["sold".to_owned(), "pending".to_owned()];
        let command = command(
            vec![
                status,
                argument("trace", ArgumentTarget::Header("X-Trace".to_owned()), false),
            ],
            None,
        );
        let mut values = vec![None, None];
        let mut script = Script::new(&["sold, gone", "sold, pending", ""]);
        arguments(&command, &mut values, FillMode::Everything, &mut script).unwrap();
        assert_eq!(
            values,
            [Some(vec!["sold".to_owned(), "pending".to_owned()]), None]
        );
        assert_eq!(
            script.asked[0],
            "status (query, string, one of sold, pending, comma-separated) [Enter to skip]: "
        );
        assert_eq!(script.said, ["`gone` is not one of sold, pending."]);
    }

    #[test]
    fn arguments_leave_json_body_fields_to_the_body() {
        let command = command(
            vec![argument(
                "email",
                ArgumentTarget::BodyField("email".to_owned()),
                false,
            )],
            Some(json_body(account_schema(), true)),
        );
        let mut values = vec![None];
        let mut script = Script::new(&[]);
        arguments(&command, &mut values, FillMode::Everything, &mut script).unwrap();
        assert_eq!(values, [None]);
    }

    #[test]
    fn body_in_everything_mode_walks_nested_fields_and_hides_secrets() {
        let command = command(Vec::new(), Some(json_body(account_schema(), true)));
        let flagged = BTreeSet::from(["email".to_owned()]);
        let mut script = Script::new(&[
            "hunter2",               // password, hidden
            "x",                     // age: not an integer
            "30",                    // age
            "owner",                 // role: not allowed
            "member",                // role
            "y",                     // fill in address?
            "Lisbon",                // address.city
            "",                      // address.zip skipped
            "a, b",                  // tags
            "y",                     // add a contact?
            "555",                   // contacts[0].phone
            "n",                     // add another?
            "{\"theme\": \"dark\"}", // settings, as JSON
        ]);
        let filled = body(&command, None, &flagged, FillMode::Everything, &mut script)
            .unwrap()
            .unwrap();
        assert_eq!(
            filled,
            json!({
                "password": "hunter2", "age": 30, "role": "member",
                "address": {"city": "Lisbon"}, "tags": ["a", "b"],
                "contacts": [{"phone": "555"}], "settings": {"theme": "dark"}
            })
        );
        assert_eq!(script.secret, ["password (string, hidden): "]);
        assert!(
            script
                .asked
                .iter()
                .all(|question| !question.starts_with("email"))
        );
        assert!(
            script
                .asked
                .iter()
                .all(|question| !question.starts_with("id"))
        );
        assert_eq!(script.said.len(), 2, "{:?}", script.said);
    }

    #[test]
    fn body_when_fields_are_missing_asks_only_for_required_ones_and_keeps_given_ones() {
        let command = command(Vec::new(), Some(json_body(account_schema(), true)));
        let given = json!({"email": "a@b.c", "age": 3});
        let mut script = Script::new(&["hunter2"]);
        let filled = body(
            &command,
            Some(given),
            &BTreeSet::new(),
            FillMode::MissingRequired,
            &mut script,
        )
        .unwrap();
        assert_eq!(
            filled,
            Some(json!({"email": "a@b.c", "age": 3, "password": "hunter2"}))
        );
        assert_eq!(script.asked.len(), 0, "{:?}", script.asked);
    }

    #[test]
    fn body_leaves_an_optional_body_alone_when_nothing_was_given() {
        let command = command(Vec::new(), Some(json_body(account_schema(), false)));
        let mut script = Script::new(&[]);
        let filled = body(
            &command,
            None,
            &BTreeSet::new(),
            FillMode::MissingRequired,
            &mut script,
        )
        .unwrap();
        assert_eq!(filled, None);
    }

    #[test]
    fn body_keeps_non_object_bodies_as_they_are() {
        let command = command(Vec::new(), Some(json_body(account_schema(), true)));
        let mut script = Script::new(&[]);
        let filled = body(
            &command,
            Some(json!([1, 2])),
            &BTreeSet::new(),
            FillMode::Everything,
            &mut script,
        )
        .unwrap();
        assert_eq!(filled, Some(json!([1, 2])));
    }
}
