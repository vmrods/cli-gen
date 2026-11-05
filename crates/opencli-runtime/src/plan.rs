//! Resolves a command's arguments into a concrete HTTP request.

use opencli_core::{ArgumentTarget, CommandSpec, HttpMethod, ValueType};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use secrecy::{ExposeSecret, SecretString};
use serde::Serialize;
use serde_json::{Map, Number, Value};
use thiserror::Error;

/// RFC 3986 unreserved characters stay as-is; everything else in a path value is encoded.
const PATH_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestPlan {
    pub method: HttpMethod,
    /// Server URL plus the resolved path; just the path when no server is known.
    pub url: String,
    pub query: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub cookies: Vec<(String, String)>,
    pub body: Option<PlannedBody>,
    /// Added by authentication just before sending; serialized redacted.
    pub credentials: Vec<CredentialPlacement>,
}

/// One credential value and where it goes in the request.
#[derive(Debug, Clone, Serialize)]
pub struct CredentialPlacement {
    #[serde(rename = "in")]
    pub location: CredentialLocation,
    pub name: String,
    #[serde(serialize_with = "redacted")]
    pub value: SecretString,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialLocation {
    Header,
    Query,
    Cookie,
}

impl PartialEq for CredentialPlacement {
    fn eq(&self, other: &Self) -> bool {
        self.location == other.location
            && self.name == other.name
            && self.value.expose_secret() == other.value.expose_secret()
    }
}

const REDACTED: &str = "<redacted>";

fn redacted<S: serde::Serializer>(_: &SecretString, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(REDACTED)
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlannedBody {
    pub content_type: String,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanError {
    #[error("this operation requires a request body")]
    MissingBody,
    #[error("body field flags need a JSON object body, but --body is not an object")]
    BodyNotObject,
    #[error("`{value}` is not a valid {expected} for --{argument}")]
    InvalidValue {
        argument: String,
        value: String,
        expected: &'static str,
    },
    #[error("missing value for path parameter `{0}`")]
    MissingPathValue(String),
}

/// Builds the request for `command`.
///
/// `server` is the resolved base URL (see the binary's server resolution).
/// `values[i]` holds the raw values given for `command.arguments[i]`, if any.
/// `body` is the parsed `--body` input; body-field flags are merged over it.
///
/// # Errors
/// [`PlanError`] when values don't match their declared types, or the body is
/// required but missing.
pub fn plan_request(
    command: &CommandSpec,
    server: Option<&str>,
    values: &[Option<Vec<String>>],
    body: Option<Value>,
) -> Result<RequestPlan, PlanError> {
    let mut path = command.path_template.clone();
    let mut query = Vec::new();
    let mut headers = Vec::new();
    let mut cookies = Vec::new();
    let mut body_fields = Map::new();

    for (argument, given) in command.arguments.iter().zip(values) {
        let Some(given) = given else {
            if let ArgumentTarget::Path(name) = &argument.target {
                return Err(PlanError::MissingPathValue(name.clone()));
            }
            continue;
        };
        match &argument.target {
            ArgumentTarget::Path(name) => {
                let value = given
                    .first()
                    .ok_or_else(|| PlanError::MissingPathValue(name.clone()))?;
                let encoded = utf8_percent_encode(value, PATH_VALUE).to_string();
                path = path.replace(&format!("{{{name}}}"), &encoded);
            }
            ArgumentTarget::Query(name) => query.extend(pairs(name, given)),
            ArgumentTarget::Header(name) => headers.extend(pairs(name, given)),
            ArgumentTarget::Cookie(name) => cookies.extend(pairs(name, given)),
            ArgumentTarget::BodyField(field) => {
                let typed = given
                    .iter()
                    .map(|value| typed_value(&argument.name, value, argument.value_type))
                    .collect::<Result<Vec<_>, _>>()?;
                let value = if argument.multiple {
                    Value::Array(typed)
                } else {
                    typed.into_iter().next().unwrap_or(Value::Null)
                };
                body_fields.insert(field.clone(), value);
            }
        }
    }

    let url = match server {
        Some(server) => format!("{}{path}", server.trim_end_matches('/')),
        None => path,
    };
    Ok(RequestPlan {
        method: command.method,
        url,
        query,
        headers,
        cookies,
        body: planned_body(command, body, body_fields)?,
        credentials: Vec::new(),
    })
}

fn pairs(name: &str, values: &[String]) -> impl Iterator<Item = (String, String)> {
    values
        .iter()
        .map(move |value| (name.to_owned(), value.clone()))
}

fn planned_body(
    command: &CommandSpec,
    body: Option<Value>,
    fields: Map<String, Value>,
) -> Result<Option<PlannedBody>, PlanError> {
    let Some(spec) = &command.body else {
        return Ok(None);
    };
    let value = match (body, fields.is_empty()) {
        (None, true) if spec.required => return Err(PlanError::MissingBody),
        (None, true) => return Ok(None),
        (None, false) => Value::Object(fields),
        (Some(value), true) => value,
        (Some(Value::Object(mut object)), false) => {
            object.extend(fields);
            Value::Object(object)
        }
        (Some(_), false) => return Err(PlanError::BodyNotObject),
    };
    Ok(Some(PlannedBody {
        content_type: spec.content_type.clone(),
        value,
    }))
}

/// Converts a command-line string into the JSON type the schema declares.
///
/// # Errors
/// [`PlanError::InvalidValue`] when `raw` doesn't parse as `value_type`.
pub fn typed_value(argument: &str, raw: &str, value_type: ValueType) -> Result<Value, PlanError> {
    let invalid = |expected| PlanError::InvalidValue {
        argument: argument.to_owned(),
        value: raw.to_owned(),
        expected,
    };
    match value_type {
        ValueType::String => Ok(Value::String(raw.to_owned())),
        ValueType::Integer => raw
            .parse::<i64>()
            .map(Value::from)
            .map_err(|_| invalid("integer")),
        ValueType::Number => raw
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map(Value::Number)
            .ok_or_else(|| invalid("number")),
        ValueType::Boolean => raw
            .parse::<bool>()
            .map(Value::Bool)
            .map_err(|_| invalid("boolean (true or false)")),
    }
}
