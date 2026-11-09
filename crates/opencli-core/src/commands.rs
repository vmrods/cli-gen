//! The command tree an [`ApiSpec`] exposes (ADR 0009): one [`CommandSpec`] per
//! operation, with flags derived from its parameters and request body.
//! Operations sharing command words are variants of one command.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::naming::{command_words, template_parameters};
use crate::{
    ApiSpec, HttpMethod, Operation, Parameter, ParameterLocation, SecurityRequirement,
    SecurityScheme, is_form_media_type, is_json_media_type, is_multipart_media_type,
    is_secret_field, kebab_case, schema,
};

/// Flags every operation command defines itself; parameters must not shadow them.
const RESERVED_FLAGS: &[&str] = &[
    "help",
    "version",
    "output",
    "query",
    "server",
    "timeout",
    "body",
    "describe",
    "dry-run",
    "yes",
    "interactive",
    "no-validate",
    "no-save",
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
    /// Command words: path words, then the method unless it is GET,
    /// e.g. `["users", "orders", "post"]`. Variants share them.
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
    /// Flag name, without the leading `--`.
    pub name: String,
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

/// Derives one command per operation (ADR 0009).
///
/// # Errors
/// [`NamingError`] when operations with the same command words can't be told
/// apart: different methods, or the same path parameters.
pub fn build_commands(spec: &ApiSpec) -> Result<CommandSet, NamingError> {
    let names = command_words(&spec.operations);
    check_conflicts(&spec.operations, &names)?;
    let mut commands: Vec<CommandSpec> = spec
        .operations
        .iter()
        .zip(names)
        .map(|(operation, path)| command(spec, operation, path))
        .collect();
    keep_path_flag_names(&mut commands);
    Ok(CommandSet {
        servers: spec.servers.clone(),
        security_schemes: spec.security_schemes.clone(),
        commands,
    })
}

fn check_conflicts(operations: &[Operation], names: &[Vec<String>]) -> Result<(), NamingError> {
    let label = |operation: &Operation| format!("{} {}", operation.method, operation.path);
    let mut conflicts = Vec::new();
    for group in groups(names) {
        let first = &operations[group[0]];
        if group
            .iter()
            .any(|&index| operations[index].method != first.method)
        {
            conflicts.push(NamingConflict {
                command: names[group[0]].join(" "),
                operations: group
                    .iter()
                    .map(|&index| label(&operations[index]))
                    .collect(),
            });
            continue;
        }
        let mut seen: HashMap<Vec<&str>, usize> = HashMap::new();
        for &index in &group {
            let mut parameters = template_parameters(&operations[index].path);
            parameters.sort_unstable();
            if let Some(&earlier) = seen.get(&parameters) {
                conflicts.push(NamingConflict {
                    command: names[index].join(" "),
                    operations: vec![label(&operations[earlier]), label(&operations[index])],
                });
            } else {
                seen.insert(parameters, index);
            }
        }
    }
    if conflicts.is_empty() {
        Ok(())
    } else {
        Err(NamingError { conflicts })
    }
}

/// Indices sharing each set of command words, in first-seen order.
fn groups<T: AsRef<[String]>>(names: &[T]) -> Vec<Vec<usize>> {
    let mut by_name: HashMap<&[String], usize> = HashMap::new();
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (index, name) in names.iter().enumerate() {
        if let Some(&group) = by_name.get(name.as_ref()) {
            groups[group].push(index);
        } else {
            by_name.insert(name.as_ref(), groups.len());
            groups.push(vec![index]);
        }
    }
    groups
}

/// Path flags pick a command's variant, so a path parameter in one variant
/// keeps its flag name and any other input of that name in a sibling variant
/// takes its location prefix (`--query-id`).
fn keep_path_flag_names(commands: &mut [CommandSpec]) {
    let paths: Vec<Vec<String>> = commands
        .iter()
        .map(|command| command.path.clone())
        .collect();
    for group in groups(&paths).into_iter().filter(|group| group.len() > 1) {
        let path_flags: HashSet<String> = group
            .iter()
            .flat_map(|&index| &commands[index].arguments)
            .filter(|argument| matches!(argument.target, ArgumentTarget::Path(_)))
            .map(|argument| argument.name.clone())
            .collect();
        for &index in &group {
            let arguments = &mut commands[index].arguments;
            let mut taken: HashSet<String> = arguments
                .iter()
                .map(|argument| argument.name.clone())
                .collect();
            for argument in arguments.iter_mut() {
                if matches!(argument.target, ArgumentTarget::Path(_))
                    || !path_flags.contains(&argument.name)
                {
                    continue;
                }
                let renamed = unique_name(
                    &format!("{}-{}", location_word(&argument.target), argument.name),
                    &taken,
                );
                taken.insert(renamed.clone());
                argument.name = renamed;
            }
        }
    }
}

const fn location_word(target: &ArgumentTarget) -> &'static str {
    match target {
        ArgumentTarget::Path(_) => "path",
        ArgumentTarget::Query(_) => "query",
        ArgumentTarget::Header(_) => "header",
        ArgumentTarget::Cookie(_) => "cookie",
        ArgumentTarget::BodyField(_) => "body",
    }
}

/// `base`, or `base-2`, `base-3`, … if it is already taken.
fn unique_name(base: &str, taken: &HashSet<String>) -> String {
    if !taken.contains(base) {
        return base.to_owned();
    }
    // `taken` is finite, so one of its length + 2 candidates is free.
    (2..=taken.len() + 2)
        .map(|number| format!("{base}-{number}"))
        .find(|candidate| !taken.contains(candidate))
        .unwrap_or_else(|| base.to_owned())
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

/// Every input is a flag (ADR 0009): path parameters first, in template
/// order, then query, header and cookie parameters, then body fields. Path
/// parameters keep their names; other clashes take a location prefix.
fn arguments(spec: &ApiSpec, operation: &Operation, body: Option<&BodySpec>) -> Vec<ArgumentSpec> {
    let parameters: Vec<&Parameter> = path_parameters_in_template_order(operation)
        .into_iter()
        .chain(
            operation
                .parameters
                .iter()
                .filter(|parameter| parameter.location != ParameterLocation::Path)
                .filter(|parameter| !is_ignored_header(parameter)),
        )
        .collect();
    let mut name_counts: HashMap<String, usize> = HashMap::new();
    for parameter in &parameters {
        *name_counts.entry(kebab_case(&parameter.name)).or_default() += 1;
    }
    let mut taken = HashSet::new();
    let mut arguments = Vec::new();
    for parameter in parameters {
        let base = kebab_case(&parameter.name);
        let is_path = parameter.location == ParameterLocation::Path;
        let needs_prefix = base.is_empty()
            || RESERVED_FLAGS.contains(&base.as_str())
            || (name_counts[&base] > 1 && !is_path);
        let wanted = if needs_prefix {
            format!("{}-{base}", parameter.location.as_str())
                .trim_end_matches('-')
                .to_owned()
        } else {
            base
        };
        let name = unique_name(&wanted, &taken);
        taken.insert(name.clone());
        arguments.push(parameter_argument(spec, parameter, name));
    }

    if let Some(body) = body {
        arguments.extend(body_field_arguments(body, &taken));
    }
    arguments
}

fn is_ignored_header(parameter: &Parameter) -> bool {
    parameter.location == ParameterLocation::Header
        && IGNORED_HEADERS.contains(&parameter.name.to_ascii_lowercase().as_str())
}

fn path_parameters_in_template_order(operation: &Operation) -> Vec<&Parameter> {
    template_parameters(&operation.path)
        .into_iter()
        .filter_map(|name| {
            operation.parameters.iter().find(|parameter| {
                parameter.location == ParameterLocation::Path && parameter.name == name
            })
        })
        .collect()
}

fn parameter_argument(spec: &ApiSpec, parameter: &Parameter, name: String) -> ArgumentSpec {
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
        target,
        required: parameter.required,
        help: parameter.description.clone(),
        value_type: shape.value_type,
        multiple: shape.multiple,
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
        // Secrets never become flags; they come from a hidden prompt or --body.
        if property_schema.get("readOnly").and_then(Value::as_bool) == Some(true)
            || is_secret_field(property, property_schema)
        {
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
