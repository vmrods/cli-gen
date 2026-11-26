//! Swagger 2.0 → OpenAPI 3.0, so one adapter serves every format (ADR 0001).
//!
//! Covers what commands are built from: servers, parameters (including `body`
//! and `formData`), request bodies, security definitions and schemas.
//! Responses are reduced to their descriptions, which 3.0 requires.

use serde_json::{Map, Value, json};

use crate::ValidationIssue;

const OPENAPI_30: &str = "3.0.3";
const DEFINITIONS_PREFIX: &str = "#/definitions/";
const SCHEMAS_PREFIX: &str = "#/components/schemas/";
const PARAMETERS_PREFIX: &str = "#/parameters/";
const DEFAULT_SCHEME: &str = "https";
const DEFAULT_MEDIA_TYPE: &str = "application/json";
const FORM_MEDIA_TYPE: &str = "application/x-www-form-urlencoded";
const MULTIPART_MEDIA_TYPE: &str = "multipart/form-data";
const METHODS: &[&str] = &["get", "put", "post", "delete", "patch", "options", "head"];
/// Copied as-is from operations; `x-…` extensions (e.g. `x-cli-name`) too.
const OPERATION_FIELDS: &[&str] = &[
    "operationId",
    "summary",
    "description",
    "deprecated",
    "tags",
    "security",
];
/// Validation keywords a 2.0 non-body parameter carries itself; in 3.0 they live in its schema.
const PARAMETER_SCHEMA_FIELDS: &[&str] = &[
    "type",
    "format",
    "items",
    "enum",
    "default",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
    "multipleOf",
];

pub(crate) fn to_openapi30(document: &Value, issues: &mut Vec<ValidationIssue>) -> Value {
    let shared_parameters = object(document.get("parameters"));
    let consumes = strings(document.get("consumes"));
    let mut paths = Map::new();
    for (path, item) in object(document.get("paths")) {
        let Some(item) = item.as_object() else {
            continue;
        };
        let inherited = list(item.get("parameters"));
        let mut converted = Map::new();
        for method in METHODS {
            let Some(operation) = item.get(*method).and_then(Value::as_object) else {
                continue;
            };
            let location = format!("{} {path}", method.to_ascii_uppercase());
            let context = Context {
                shared_parameters: &shared_parameters,
                consumes: &consumes,
                location: &location,
            };
            converted.insert(
                (*method).to_owned(),
                context.operation(operation, &inherited, issues),
            );
        }
        paths.insert(path.clone(), Value::Object(converted));
    }

    let mut result = json!({
        "openapi": OPENAPI_30,
        "info": document.get("info").cloned().unwrap_or_else(|| json!({})),
        "servers": servers(document),
        "paths": paths,
        "components": {
            "schemas": rewrite_references(document.get("definitions").cloned().unwrap_or_else(|| json!({}))),
            "securitySchemes": security_schemes(&object(document.get("securityDefinitions"))),
        },
    });
    if let Some(security) = document.get("security") {
        result["security"] = security.clone();
    }
    result
}

struct Context<'a> {
    shared_parameters: &'a Map<String, Value>,
    consumes: &'a [String],
    location: &'a str,
}

impl Context<'_> {
    fn operation(
        &self,
        operation: &Map<String, Value>,
        inherited: &[Value],
        issues: &mut Vec<ValidationIssue>,
    ) -> Value {
        let mut converted: Map<String, Value> = operation
            .iter()
            .filter(|(key, _)| OPERATION_FIELDS.contains(&key.as_str()) || key.starts_with("x-"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();

        let consumes = Some(strings(operation.get("consumes")))
            .filter(|types| !types.is_empty())
            .unwrap_or_else(|| self.consumes.to_vec());
        let mut parameters = Vec::new();
        let mut body = None;
        let mut form_fields = Map::new();
        let mut form_required = Vec::new();
        let mut has_file = false;
        for parameter in
            self.effective_parameters(inherited, &list(operation.get("parameters")), issues)
        {
            let name = parameter
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            match parameter.get("in").and_then(Value::as_str) {
                Some("body") => body = Some(body_from(&parameter, &consumes)),
                Some("formData") => {
                    has_file |= parameter.get("type").and_then(Value::as_str) == Some("file");
                    if parameter.get("required").and_then(Value::as_bool) == Some(true) {
                        form_required.push(Value::String(name.clone()));
                    }
                    let mut schema = parameter_schema(&parameter);
                    if let Some(description) = parameter.get("description") {
                        schema.insert("description".into(), description.clone());
                    }
                    form_fields.insert(name, Value::Object(schema));
                }
                Some(_) => parameters.push(parameter_from(&parameter)),
                None => issues.push(ValidationIssue::new(
                    self.location,
                    format!("parameter `{name}` has no `in`"),
                )),
            }
        }
        if !form_fields.is_empty() {
            let media_type = if has_file || consumes.iter().any(|kind| kind == MULTIPART_MEDIA_TYPE)
            {
                MULTIPART_MEDIA_TYPE
            } else {
                FORM_MEDIA_TYPE
            };
            let mut schema = json!({"type": "object", "properties": form_fields});
            if !form_required.is_empty() {
                schema["required"] = Value::Array(form_required.clone());
            }
            body = Some(json!({
                "required": !form_required.is_empty(),
                "content": {media_type: {"schema": schema}},
            }));
        }
        converted.insert("parameters".into(), Value::Array(parameters));
        if let Some(body) = body {
            converted.insert("requestBody".into(), body);
        }
        converted.insert("responses".into(), responses(operation.get("responses")));
        Value::Object(converted)
    }

    /// Path-level parameters overridden by operation ones (same name and `in`),
    /// with `#/parameters/…` references resolved.
    fn effective_parameters(
        &self,
        inherited: &[Value],
        own: &[Value],
        issues: &mut Vec<ValidationIssue>,
    ) -> Vec<Value> {
        let key = |parameter: &Value| {
            (
                parameter
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                parameter
                    .get("in")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            )
        };
        let mut merged: Vec<Value> = Vec::new();
        for parameter in inherited.iter().chain(own) {
            let Some(parameter) = self.resolve(parameter, issues) else {
                continue;
            };
            merged.retain(|existing| key(existing) != key(&parameter));
            merged.push(parameter);
        }
        merged
    }

    fn resolve(&self, parameter: &Value, issues: &mut Vec<ValidationIssue>) -> Option<Value> {
        let Some(reference) = parameter.get("$ref").and_then(Value::as_str) else {
            return Some(parameter.clone());
        };
        let found = reference
            .strip_prefix(PARAMETERS_PREFIX)
            .and_then(|name| self.shared_parameters.get(name))
            .cloned();
        if found.is_none() {
            issues.push(ValidationIssue::new(
                self.location,
                format!("reference `{reference}` points to an undefined parameter"),
            ));
        }
        found
    }
}

fn parameter_from(parameter: &Value) -> Value {
    let mut converted = Map::new();
    for key in ["name", "in", "description", "required"] {
        if let Some(value) = parameter.get(key) {
            converted.insert(key.into(), value.clone());
        }
    }
    converted.insert("schema".into(), Value::Object(parameter_schema(parameter)));
    // 2.0's default for arrays is `csv` (`a,b`); 3.0 query parameters repeat by default.
    let collection_format = parameter
        .get("collectionFormat")
        .and_then(Value::as_str)
        .unwrap_or("csv");
    let is_array = parameter.get("type").and_then(Value::as_str) == Some("array");
    if is_array && parameter.get("in").and_then(Value::as_str) == Some("query") {
        let (style, explode) = match collection_format {
            "multi" => ("form", true),
            "ssv" => ("spaceDelimited", false),
            "pipes" => ("pipeDelimited", false),
            _ => ("form", false),
        };
        converted.insert("style".into(), Value::String(style.into()));
        converted.insert("explode".into(), Value::Bool(explode));
    }
    Value::Object(converted)
}

fn parameter_schema(parameter: &Value) -> Map<String, Value> {
    let mut schema: Map<String, Value> = PARAMETER_SCHEMA_FIELDS
        .iter()
        .filter_map(|key| {
            parameter
                .get(*key)
                .map(|value| ((*key).to_owned(), rewrite_references(value.clone())))
        })
        .collect();
    if schema.get("type").and_then(Value::as_str) == Some("file") {
        schema.insert("type".into(), Value::String("string".into()));
        schema.insert("format".into(), Value::String("binary".into()));
    }
    schema
}

fn body_from(parameter: &Value, consumes: &[String]) -> Value {
    let schema = rewrite_references(
        parameter
            .get("schema")
            .cloned()
            .unwrap_or_else(|| json!({})),
    );
    let media_types: Vec<&str> = if consumes.is_empty() {
        vec![DEFAULT_MEDIA_TYPE]
    } else {
        consumes.iter().map(String::as_str).collect()
    };
    let content: Map<String, Value> = media_types
        .into_iter()
        .map(|media_type| (media_type.to_owned(), json!({"schema": schema})))
        .collect();
    let mut body = json!({
        "required": parameter.get("required").and_then(Value::as_bool).unwrap_or(false),
        "content": content,
    });
    if let Some(description) = parameter.get("description") {
        body["description"] = description.clone();
    }
    body
}

/// `schemes` × `host` + `basePath`, HTTPS first. Without `host`, the base
/// path is relative and resolves against the spec's URL, as in 3.x.
fn servers(document: &Value) -> Value {
    let base_path = document
        .get("basePath")
        .and_then(Value::as_str)
        .unwrap_or("/");
    let Some(host) = document.get("host").and_then(Value::as_str) else {
        return json!([{"url": base_path}]);
    };
    let mut schemes = strings(document.get("schemes"));
    if schemes.is_empty() {
        schemes.push(DEFAULT_SCHEME.to_owned());
    }
    schemes.sort_by_key(|scheme| scheme != DEFAULT_SCHEME);
    Value::Array(
        schemes
            .iter()
            .map(|scheme| json!({"url": format!("{scheme}://{host}{}", base_path.trim_end_matches('/'))}))
            .collect(),
    )
}

fn security_schemes(definitions: &Map<String, Value>) -> Value {
    let converted: Map<String, Value> = definitions
        .iter()
        .map(|(name, definition)| (name.clone(), security_scheme(definition)))
        .collect();
    Value::Object(converted)
}

fn security_scheme(definition: &Value) -> Value {
    let field = |key: &str| definition.get(key).cloned().unwrap_or(Value::Null);
    match definition.get("type").and_then(Value::as_str) {
        Some("basic") => json!({"type": "http", "scheme": "basic"}),
        Some("apiKey") => json!({"type": "apiKey", "name": field("name"), "in": field("in")}),
        Some("oauth2") => {
            let scopes = definition
                .get("scopes")
                .cloned()
                .unwrap_or_else(|| json!({}));
            let flow = match definition.get("flow").and_then(Value::as_str) {
                Some("application") => {
                    json!({"clientCredentials": {"tokenUrl": field("tokenUrl"), "scopes": scopes}})
                }
                Some("accessCode") => json!({"authorizationCode": {
                    "authorizationUrl": field("authorizationUrl"), "tokenUrl": field("tokenUrl"), "scopes": scopes
                }}),
                Some("password") => {
                    json!({"password": {"tokenUrl": field("tokenUrl"), "scopes": scopes}})
                }
                _ => {
                    json!({"implicit": {"authorizationUrl": field("authorizationUrl"), "scopes": scopes}})
                }
            };
            json!({"type": "oauth2", "flows": flow})
        }
        // Unknown types surface as unsupported HTTP schemes rather than failing the document.
        _ => json!({"type": "http", "scheme": "unknown"}),
    }
}

fn responses(responses: Option<&Value>) -> Value {
    let converted: Map<String, Value> = object(responses)
        .iter()
        .map(|(status, response)| {
            let description = response
                .get("description")
                .cloned()
                .unwrap_or_else(|| Value::String(String::new()));
            (status.clone(), json!({"description": description}))
        })
        .collect();
    if converted.is_empty() {
        return json!({"default": {"description": ""}});
    }
    Value::Object(converted)
}

/// `#/definitions/X` → `#/components/schemas/X`, everywhere in `value`.
fn rewrite_references(mut value: Value) -> Value {
    match &mut value {
        Value::Object(fields) => {
            for (key, child) in fields.iter_mut() {
                match child {
                    Value::String(reference) if key == "$ref" => {
                        if let Some(name) = reference.strip_prefix(DEFINITIONS_PREFIX) {
                            *reference = format!("{SCHEMAS_PREFIX}{name}");
                        }
                    }
                    _ => *child = rewrite_references(child.take()),
                }
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                *item = rewrite_references(item.take());
            }
        }
        _ => {}
    }
    value
}

fn object(value: Option<&Value>) -> Map<String, Value> {
    value
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

fn list(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value.and_then(Value::as_array).map_or_default(|items| {
        items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn convert(document: &Value) -> (Value, Vec<ValidationIssue>) {
        let mut issues = Vec::new();
        (to_openapi30(document, &mut issues), issues)
    }

    #[test]
    fn servers_combine_schemes_host_and_base_path_https_first() {
        let document =
            json!({"host": "api.example.com", "basePath": "/v1/", "schemes": ["http", "https"]});
        assert_eq!(
            servers(&document),
            json!([{"url": "https://api.example.com/v1"}, {"url": "http://api.example.com/v1"}])
        );
        assert_eq!(
            servers(&json!({"host": "api.example.com"})),
            json!([{"url": "https://api.example.com"}])
        );
        assert_eq!(
            servers(&json!({"basePath": "/v2"})),
            json!([{"url": "/v2"}])
        );
    }

    #[test]
    fn security_definitions_map_to_30_schemes() {
        let cases = [
            (
                json!({"type": "basic"}),
                json!({"type": "http", "scheme": "basic"}),
            ),
            (
                json!({"type": "apiKey", "name": "X-Key", "in": "header"}),
                json!({"type": "apiKey", "name": "X-Key", "in": "header"}),
            ),
            (
                json!({"type": "oauth2", "flow": "application", "tokenUrl": "https://a/t", "scopes": {"r": "Read"}}),
                json!({"type": "oauth2", "flows": {"clientCredentials": {"tokenUrl": "https://a/t", "scopes": {"r": "Read"}}}}),
            ),
            (
                json!({"type": "oauth2", "flow": "accessCode", "authorizationUrl": "https://a/a", "tokenUrl": "https://a/t"}),
                json!({"type": "oauth2", "flows": {"authorizationCode": {"authorizationUrl": "https://a/a", "tokenUrl": "https://a/t", "scopes": {}}}}),
            ),
        ];
        for (definition, expected) in cases {
            assert_eq!(security_scheme(&definition), expected);
        }
    }

    #[test]
    fn body_parameters_become_request_bodies_with_rewritten_references() {
        let document = json!({
            "consumes": ["application/json"],
            "paths": {"/pets": {"post": {"parameters": [
                {"name": "pet", "in": "body", "required": true, "schema": {"$ref": "#/definitions/NewPet"}}
            ]}}}
        });
        let (converted, issues) = convert(&document);
        assert_eq!(issues, []);
        let operation = &converted["paths"]["/pets"]["post"];
        assert_eq!(
            operation["requestBody"],
            json!({"required": true, "content": {"application/json": {"schema": {"$ref": "#/components/schemas/NewPet"}}}})
        );
        assert_eq!(operation["parameters"], json!([]));
    }

    #[test]
    fn form_data_parameters_become_form_bodies() {
        let document = json!({"paths": {"/login": {"post": {"parameters": [
            {"name": "user", "in": "formData", "type": "string", "required": true},
            {"name": "remember", "in": "formData", "type": "boolean"}
        ]}}, "/upload": {"post": {"parameters": [
            {"name": "file", "in": "formData", "type": "file"}
        ]}}}});
        let (converted, _) = convert(&document);
        let login = &converted["paths"]["/login"]["post"]["requestBody"];
        assert_eq!(login["required"], true);
        let schema = &login["content"]["application/x-www-form-urlencoded"]["schema"];
        assert_eq!(schema["properties"]["remember"], json!({"type": "boolean"}));
        assert_eq!(schema["required"], json!(["user"]));
        let upload = &converted["paths"]["/upload"]["post"]["requestBody"]["content"]["multipart/form-data"]
            ["schema"];
        assert_eq!(
            upload["properties"]["file"],
            json!({"type": "string", "format": "binary"})
        );
    }

    #[test]
    fn parameters_move_validation_into_schema_and_map_collection_formats() {
        let document = json!({"paths": {"/pets": {"get": {"parameters": [
            {"name": "limit", "in": "query", "type": "integer", "maximum": 100},
            {"name": "tags", "in": "query", "type": "array", "items": {"type": "string"}},
            {"name": "ids", "in": "query", "type": "array", "items": {"type": "string"}, "collectionFormat": "multi"},
            {"name": "codes", "in": "query", "type": "array", "items": {"type": "string"}, "collectionFormat": "pipes"}
        ]}}}});
        let (converted, _) = convert(&document);
        let parameters = &converted["paths"]["/pets"]["get"]["parameters"];
        assert_eq!(
            parameters[0],
            json!({"name": "limit", "in": "query", "schema": {"type": "integer", "maximum": 100}})
        );
        assert_eq!(
            (&parameters[1]["style"], &parameters[1]["explode"]),
            (&json!("form"), &json!(false))
        );
        assert_eq!(
            (&parameters[2]["style"], &parameters[2]["explode"]),
            (&json!("form"), &json!(true))
        );
        assert_eq!(parameters[3]["style"], "pipeDelimited");
    }

    #[test]
    fn shared_parameter_references_are_inlined_and_operations_override_path_items() {
        let document = json!({
            "parameters": {"Limit": {"name": "limit", "in": "query", "type": "integer"}},
            "paths": {"/pets": {
                "parameters": [{"name": "q", "in": "query", "type": "string", "description": "path-level"}],
                "get": {"parameters": [
                    {"$ref": "#/parameters/Limit"},
                    {"name": "q", "in": "query", "type": "string", "description": "operation-level"},
                    {"$ref": "#/parameters/Missing"}
                ]}
            }}
        });
        let (converted, issues) = convert(&document);
        let parameters = converted["paths"]["/pets"]["get"]["parameters"]
            .as_array()
            .unwrap()
            .clone();
        let names: Vec<_> = parameters
            .iter()
            .map(|parameter| parameter["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["limit", "q"]);
        assert_eq!(parameters[1]["description"], "operation-level");
        assert_eq!(
            issues[0].message,
            "reference `#/parameters/Missing` points to an undefined parameter"
        );
    }

    #[test]
    fn extensions_and_operation_fields_are_kept() {
        let document = json!({"paths": {"/a": {"get": {"operationId": "a", "x-cli-name": "fetch", "produces": ["application/xml"]}}}});
        let (converted, _) = convert(&document);
        let operation = &converted["paths"]["/a"]["get"];
        assert_eq!(operation["operationId"], "a");
        assert_eq!(operation["x-cli-name"], "fetch");
        assert!(operation.get("produces").is_none());
        assert_eq!(
            operation["responses"],
            json!({"default": {"description": ""}})
        );
    }
}
