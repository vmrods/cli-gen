//! The command tree an [`ApiSpec`] exposes (ADR 0002): one leaf command per
//! operation, with arguments derived from its parameters and request body.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::naming::{Draft, draft_names, fallback_verb, short_fallback_verb};
use crate::{
    ApiSpec, HttpMethod, Operation, Parameter, ParameterLocation, SecurityRequirement,
    SecurityScheme, is_form_media_type, is_json_media_type, is_multipart_media_type, kebab_case,
    schema,
};

/// Flags every operation command defines itself; parameters must not shadow them.
const RESERVED_FLAGS: &[&str] = &[
    "help", "version", "output", "server", "timeout", "body", "describe", "dry-run", "yes",
];
/// OpenAPI says these header parameters are ignored: content negotiation and
/// auth are handled by the runtime.
const IGNORED_HEADERS: &[&str] = &["accept", "content-type", "authorization"];
const BODY_FLAG_PREFIX: &str = "body-";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandSet {
    pub servers: Vec<String>,
    pub security_schemes: BTreeMap<String, SecurityScheme>,
    pub commands: Vec<CommandSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    /// Resource words then verb, e.g. `["users", "orders", "list"]`.
    pub path: Vec<String>,
    pub method: HttpMethod,
    /// Path template with `{name}` placeholders.
    pub path_template: String,
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub deprecated: bool,
    pub arguments: Vec<ArgumentSpec>,
    pub body: Option<BodySpec>,
    /// Alternatives, any one of which authorizes the call; empty means none needed.
    pub security: Vec<SecurityRequirement>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArgumentSpec {
    /// Flag name (`--limit`) or positional value name (`pet-id`).
    pub name: String,
    pub positional: bool,
    pub target: ArgumentTarget,
    pub required: bool,
    pub help: Option<String>,
    pub value_type: ValueType,
    /// Repeatable (array parameter or array body field).
    pub multiple: bool,
    pub allowed_values: Vec<String>,
    /// Joins repeated values into one (`a,b`) instead of repeating the parameter.
    #[serde(default)]
    pub array_delimiter: Option<char>,
}

/// Where an argument's value goes in the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "in", content = "name", rename_all = "kebab-case")]
pub enum ArgumentTarget {
    Path(String),
    Query(String),
    Header(String),
    Cookie(String),
    BodyField(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    #[default]
    String,
    Integer,
    Number,
    Boolean,
    /// A local file path whose contents are uploaded (multipart bodies).
    File,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BodySpec {
    pub content_type: String,
    pub required: bool,
    pub description: Option<String>,
    /// Self-contained schema with local `$ref`s inlined.
    pub schema: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{} command name conflict(s)", conflicts.len())]
pub struct NamingError {
    pub conflicts: Vec<NamingConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamingConflict {
    /// The contested command path, space-separated.
    pub command: String,
    /// The operations involved, as `METHOD /path`.
    pub operations: Vec<String>,
}

/// Derives one command per operation.
///
/// Names come from `x-cli-*` hints, then the method/path pattern. When pattern
/// names collide, the colliding operations fall back to their `operationId`.
///
/// # Errors
/// [`NamingError`] when names still collide, or a command name is also a group.
pub fn build_commands(spec: &ApiSpec) -> Result<CommandSet, NamingError> {
    let names = resolve_names(&spec.operations)?;
    let commands = spec
        .operations
        .iter()
        .zip(names)
        .map(|(operation, path)| command(spec, operation, path))
        .collect();
    Ok(CommandSet {
        servers: spec.servers.clone(),
        security_schemes: spec.security_schemes.clone(),
        commands,
    })
}

fn resolve_names(operations: &[Operation]) -> Result<Vec<Vec<String>>, NamingError> {
    let drafts = draft_names(operations);
    let mut names: Vec<Vec<String>> = drafts.iter().map(|draft| draft.words.clone()).collect();
    // Colliding pattern names fall back to a short operationId-based verb, then,
    // if that still collides, to the full operationId. A command that is also a
    // group of other commands falls back the same way.
    for fallback in [Fallback::Short, Fallback::Full] {
        let mut colliding: Vec<usize> = duplicate_groups(&names).into_iter().flatten().collect();
        colliding.extend(group_conflicts(&names).into_iter().map(|(leaf, _)| leaf));
        for index in colliding {
            rename_with_operation_id(
                &mut names[index],
                &drafts[index],
                &operations[index],
                fallback,
            );
        }
    }

    let label = |index: usize| format!("{} {}", operations[index].method, operations[index].path);
    let mut conflicts: Vec<NamingConflict> = duplicate_groups(&names)
        .into_iter()
        .map(|group| NamingConflict {
            command: names[group[0]].join(" "),
            operations: group.into_iter().map(label).collect(),
        })
        .collect();
    conflicts.extend(
        group_conflicts(&names)
            .into_iter()
            .map(|(leaf, nested)| NamingConflict {
                command: names[leaf].join(" "),
                operations: vec![label(leaf), label(nested)],
            }),
    );
    if conflicts.is_empty() {
        Ok(names)
    } else {
        Err(NamingError { conflicts })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fallback {
    Short,
    Full,
}

fn rename_with_operation_id(
    name: &mut [String],
    draft: &Draft,
    operation: &Operation,
    fallback: Fallback,
) {
    if draft.from_hints || operation.operation_id.is_none() {
        return;
    }
    let Some((verb, resources)) = name.split_last_mut() else {
        return;
    };
    *verb = match fallback {
        Fallback::Short => {
            short_fallback_verb(operation, resources).unwrap_or_else(|| fallback_verb(operation))
        }
        Fallback::Full => fallback_verb(operation),
    };
}

/// Indices of names shared by more than one operation, in first-seen order.
fn duplicate_groups(names: &[Vec<String>]) -> Vec<Vec<usize>> {
    let mut by_name: HashMap<&[String], Vec<usize>> = HashMap::new();
    let mut order = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let entry = by_name.entry(name.as_slice()).or_default();
        if entry.is_empty() {
            order.push(name.as_slice());
        }
        entry.push(index);
    }
    order
        .into_iter()
        .filter_map(|name| by_name.remove(name).filter(|group| group.len() > 1))
        .collect()
}

/// Pairs of (leaf, nested) where a command name is also a group of other commands.
fn group_conflicts(names: &[Vec<String>]) -> Vec<(usize, usize)> {
    let mut group_owner: HashMap<&[String], usize> = HashMap::new();
    for (index, name) in names.iter().enumerate() {
        for length in 1..name.len() {
            group_owner.entry(&name[..length]).or_insert(index);
        }
    }
    names
        .iter()
        .enumerate()
        .filter_map(|(index, name)| {
            group_owner
                .get(name.as_slice())
                .map(|&nested| (index, nested))
        })
        .collect()
}

fn command(spec: &ApiSpec, operation: &Operation, path: Vec<String>) -> CommandSpec {
    let body = operation.request_body.as_ref().map(|body| BodySpec {
        content_type: body.content_type.clone(),
        required: body.required,
        description: body.description.clone(),
        schema: body
            .schema
            .as_ref()
            .map(|schema| schema::inline(schema, &spec.schemas)),
    });
    CommandSpec {
        path,
        method: operation.method,
        path_template: operation.path.clone(),
        operation_id: operation.operation_id.clone(),
        summary: operation.summary.clone(),
        description: operation.description.clone(),
        deprecated: operation.deprecated,
        arguments: arguments(spec, operation, body.as_ref()),
        body,
        security: operation.security.clone(),
    }
}

fn arguments(spec: &ApiSpec, operation: &Operation, body: Option<&BodySpec>) -> Vec<ArgumentSpec> {
    let mut arguments: Vec<ArgumentSpec> = path_parameters_in_template_order(operation)
        .into_iter()
        .map(|parameter| parameter_argument(spec, parameter, kebab_case(&parameter.name), true))
        .collect();

    let flag_parameters: Vec<&Parameter> = operation
        .parameters
        .iter()
        .filter(|parameter| parameter.location != ParameterLocation::Path)
        .filter(|parameter| !is_ignored_header(parameter))
        .collect();
    let mut name_counts: HashMap<String, usize> = HashMap::new();
    for parameter in &flag_parameters {
        *name_counts.entry(kebab_case(&parameter.name)).or_default() += 1;
    }
    for parameter in flag_parameters {
        let base = kebab_case(&parameter.name);
        let needs_prefix =
            base.is_empty() || name_counts[&base] > 1 || RESERVED_FLAGS.contains(&base.as_str());
        let name = if needs_prefix {
            format!("{}-{base}", parameter.location.as_str())
                .trim_end_matches('-')
                .to_owned()
        } else {
            base
        };
        arguments.push(parameter_argument(spec, parameter, name, false));
    }

    if let Some(body) = body {
        let taken: HashSet<String> = arguments
            .iter()
            .filter(|argument| !argument.positional)
            .map(|argument| argument.name.clone())
            .collect();
        arguments.extend(body_field_arguments(body, &taken));
    }
    arguments
}

fn is_ignored_header(parameter: &Parameter) -> bool {
    parameter.location == ParameterLocation::Header
        && IGNORED_HEADERS.contains(&parameter.name.to_ascii_lowercase().as_str())
}

fn path_parameters_in_template_order(operation: &Operation) -> Vec<&Parameter> {
    operation
        .path
        .split('{')
        .skip(1)
        .filter_map(|segment| segment.split_once('}').map(|(name, _)| name))
        .filter_map(|name| {
            operation.parameters.iter().find(|parameter| {
                parameter.location == ParameterLocation::Path && parameter.name == name
            })
        })
        .collect()
}

fn parameter_argument(
    spec: &ApiSpec,
    parameter: &Parameter,
    name: String,
    positional: bool,
) -> ArgumentSpec {
    let shape = parameter
        .schema
        .as_ref()
        .map(|schema| Shape::of(schema, &spec.schemas))
        .unwrap_or_default();
    let target = match parameter.location {
        ParameterLocation::Path => ArgumentTarget::Path(parameter.name.clone()),
        ParameterLocation::Query => ArgumentTarget::Query(parameter.name.clone()),
        ParameterLocation::Header => ArgumentTarget::Header(parameter.name.clone()),
        ParameterLocation::Cookie => ArgumentTarget::Cookie(parameter.name.clone()),
    };
    ArgumentSpec {
        name,
        positional,
        target,
        required: parameter.required,
        help: parameter.description.clone(),
        value_type: shape.value_type,
        multiple: shape.multiple && !positional,
        allowed_values: shape.allowed_values,
        array_delimiter: parameter.array_delimiter,
    }
}

/// Scalar top-level properties of a JSON or form body become flags, so
/// `customers create --name Victor` works without writing JSON. Required
/// fields stay optional on the command line because `--body` can supply them.
fn body_field_arguments(body: &BodySpec, taken: &HashSet<String>) -> Vec<ArgumentSpec> {
    let multipart = is_multipart_media_type(&body.content_type);
    if !is_json_media_type(&body.content_type)
        && !is_form_media_type(&body.content_type)
        && !multipart
    {
        return Vec::new();
    }
    let Some(schema) = body.schema.as_ref() else {
        return Vec::new();
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Vec::new();
    };
    let no_schemas = BTreeMap::new();
    let mut used = taken.clone();
    let mut arguments = Vec::new();
    for (property, property_schema) in properties {
        if property_schema.get("readOnly").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let mut shape = Shape::of(property_schema, &no_schemas);
        if !shape.is_scalar {
            continue;
        }
        // In a multipart body, binary fields are files to upload.
        if multipart && is_binary(property_schema) {
            shape.value_type = ValueType::File;
        }
        let base = kebab_case(property);
        let name = [base.clone(), format!("{BODY_FLAG_PREFIX}{base}")]
            .into_iter()
            .find(|candidate| {
                !candidate.is_empty()
                    && !RESERVED_FLAGS.contains(&candidate.as_str())
                    && !used.contains(candidate)
            });
        let Some(name) = name else {
            continue;
        };
        used.insert(name.clone());
        arguments.push(ArgumentSpec {
            name,
            positional: false,
            target: ArgumentTarget::BodyField(property.clone()),
            required: false,
            help: property_schema
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
            value_type: shape.value_type,
            multiple: shape.multiple,
            allowed_values: shape.allowed_values,
            array_delimiter: None,
        });
    }
    arguments
}

/// `format: binary` (3.0, Swagger `file`) or a `contentMediaType` (3.1), on
/// the schema or, for arrays, on its items.
fn is_binary(schema: &Value) -> bool {
    let target = match schema.get("type").and_then(Value::as_str) {
        Some("array") => schema.get("items").unwrap_or(schema),
        _ => schema,
    };
    target.get("format").and_then(Value::as_str) == Some("binary")
        || target.get("contentMediaType").is_some()
}

/// What a schema looks like from the command line.
#[derive(Debug, Default)]
struct Shape {
    value_type: ValueType,
    multiple: bool,
    is_scalar: bool,
    allowed_values: Vec<String>,
}

impl Shape {
    fn of(schema: &Value, schemas: &BTreeMap<String, Value>) -> Self {
        let resolved = schema::resolve(schema, schemas);
        if resolved.get("type").and_then(Value::as_str) == Some("array") {
            let item = resolved
                .get("items")
                .map(|items| Self::of(items, schemas))
                .unwrap_or_default();
            return Self {
                multiple: true,
                ..item
            };
        }
        let (value_type, is_scalar) = match resolved.get("type").and_then(Value::as_str) {
            Some("integer") => (ValueType::Integer, true),
            Some("number") => (ValueType::Number, true),
            Some("boolean") => (ValueType::Boolean, true),
            Some("string") => (ValueType::String, true),
            None if resolved.get("enum").is_some() => (ValueType::String, true),
            _ => (ValueType::String, false),
        };
        let allowed_values = resolved
            .get("enum")
            .and_then(Value::as_array)
            .map(|values| values.iter().filter_map(scalar_text).collect())
            .unwrap_or_default();
        Self {
            value_type,
            multiple: false,
            is_scalar,
            allowed_values,
        }
    }
}

fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}
