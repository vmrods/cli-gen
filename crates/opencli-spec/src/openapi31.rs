//! OpenAPI 3.1 → 3.0 shape, so one adapter serves both (ADR 0001).
//!
//! 3.1's structure matches 3.0 apart from a few objects; the real difference
//! is the schema dialect (JSON Schema 2020-12). Schemas are rewritten to the
//! closest 3.0 form, which loses only spelling (`type: [x, "null"]` becomes
//! `nullable: true`), not anything commands are built from.

use serde_json::{Map, Value, json};

const OPENAPI_30: &str = "3.0.3";
const PATH_ITEMS_PREFIX: &str = "#/components/pathItems/";
const NULL_TYPE: &str = "null";
/// JSON Schema keywords whose values are themselves schemas.
const SUBSCHEMA_KEYWORDS: &[&str] = &[
    "items",
    "not",
    "additionalProperties",
    "contains",
    "if",
    "then",
    "else",
];
const SUBSCHEMA_LIST_KEYWORDS: &[&str] = &["allOf", "anyOf", "oneOf"];
const SUBSCHEMA_MAP_KEYWORDS: &[&str] = &[
    "properties",
    "patternProperties",
    "$defs",
    "dependentSchemas",
];

pub(crate) fn to_openapi30(mut document: Value) -> Value {
    let Some(root) = document.as_object_mut() else {
        return document;
    };
    root.insert("openapi".into(), Value::String(OPENAPI_30.into()));
    // 3.1 makes `paths` optional (a spec may hold only webhooks).
    root.entry("paths").or_insert_with(|| json!({}));
    root.remove("webhooks");
    root.remove("jsonSchemaDialect");
    if let Some(info) = root.get_mut("info").and_then(Value::as_object_mut) {
        // 3.1 allows a license with only an SPDX `identifier`; 3.0 requires `name`.
        info.remove("license");
        info.remove("summary");
    }
    inline_path_item_references(root);
    convert_mutual_tls(root);
    if let Some(schemas) = root
        .get_mut("components")
        .and_then(|components| components.get_mut("schemas"))
        .and_then(Value::as_object_mut)
    {
        schemas.values_mut().for_each(normalize_schema);
    }
    normalize_schema_positions(&mut document);
    document
}

/// 3.0 has no `components.pathItems`; inline the referenced items.
fn inline_path_item_references(root: &mut Map<String, Value>) {
    let path_items = root
        .get_mut("components")
        .and_then(Value::as_object_mut)
        .and_then(|components| components.remove("pathItems"))
        .unwrap_or_default();
    let Some(paths) = root.get_mut("paths").and_then(Value::as_object_mut) else {
        return;
    };
    for item in paths.values_mut() {
        let target = item
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| reference.strip_prefix(PATH_ITEMS_PREFIX))
            .and_then(|name| path_items.get(name));
        if let Some(target) = target {
            *item = target.clone();
        }
    }
}

/// 3.0 can't express `mutualTLS`; an unknown HTTP scheme keeps the scheme
/// visible as unsupported instead of failing the whole document.
fn convert_mutual_tls(root: &mut Map<String, Value>) {
    let Some(schemes) = root
        .get_mut("components")
        .and_then(|components| components.get_mut("securitySchemes"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for scheme in schemes.values_mut() {
        if scheme.get("type").and_then(Value::as_str) == Some("mutualTLS") {
            *scheme = json!({"type": "http", "scheme": "mutual-tls"});
        }
    }
}

/// Normalizes the `schema` member of every parameter, header and media type.
fn normalize_schema_positions(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, child) in fields.iter_mut() {
                if key == "schema" {
                    normalize_schema(child);
                }
                normalize_schema_positions(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_schema_positions),
        _ => {}
    }
}

/// Rewrites one 3.1 schema (and its subschemas) into 3.0 form. Idempotent.
fn normalize_schema(schema: &mut Value) {
    let Some(fields) = schema.as_object_mut() else {
        return;
    };
    normalize_type(fields);
    for bound in ["Minimum", "Maximum"] {
        let exclusive = format!("exclusive{bound}");
        if let Some(limit) = fields
            .get(&exclusive)
            .filter(|value| value.is_number())
            .cloned()
        {
            fields.insert(bound.to_ascii_lowercase(), limit);
            fields.insert(exclusive, Value::Bool(true));
        }
    }
    if let Some(constant) = fields.remove("const") {
        fields
            .entry("enum")
            .or_insert_with(|| Value::Array(vec![constant]));
    }
    if let Some(Value::Array(examples)) = fields.remove("examples")
        && let Some(first) = examples.into_iter().next()
    {
        fields.entry("example").or_insert(first);
    }
    // Tuples have no 3.0 form; keep the first item's type as the element type.
    if let Some(Value::Array(prefix)) = fields.remove("prefixItems")
        && let Some(first) = prefix.into_iter().next()
    {
        fields.entry("items").or_insert(first);
    }
    for keyword in SUBSCHEMA_KEYWORDS {
        if let Some(child) = fields.get_mut(*keyword) {
            normalize_schema(child);
        }
    }
    for keyword in SUBSCHEMA_LIST_KEYWORDS {
        if let Some(Value::Array(children)) = fields.get_mut(*keyword) {
            children.iter_mut().for_each(normalize_schema);
        }
    }
    for keyword in SUBSCHEMA_MAP_KEYWORDS {
        if let Some(Value::Object(children)) = fields.get_mut(*keyword) {
            children.values_mut().for_each(normalize_schema);
        }
    }
}

/// `type: ["string", "null"]` → `type: string, nullable: true`; several
/// non-null types have no 3.0 equivalent, so the schema accepts anything.
fn normalize_type(fields: &mut Map<String, Value>) {
    let types: Vec<String> = match fields.get("type") {
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(single)) if single == NULL_TYPE => vec![NULL_TYPE.to_owned()],
        _ => return,
    };
    let nullable = types.iter().any(|kind| kind == NULL_TYPE);
    let concrete: Vec<&String> = types.iter().filter(|kind| *kind != NULL_TYPE).collect();
    match concrete.as_slice() {
        [single] => {
            fields.insert("type".into(), Value::String((*single).clone()));
        }
        _ => {
            fields.remove("type");
        }
    }
    if nullable {
        fields.insert("nullable".into(), Value::Bool(true));
    }
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    fn normalized(schema: Value) -> Value {
        let mut schema = schema;
        normalize_schema(&mut schema);
        schema
    }

    #[rstest]
    #[case(json!({"type": ["string", "null"]}), json!({"type": "string", "nullable": true}))]
    #[case(json!({"type": "null"}), json!({"nullable": true}))]
    #[case(json!({"type": ["string", "integer"]}), json!({}))]
    #[case(json!({"type": "integer", "exclusiveMinimum": 0}), json!({"type": "integer", "minimum": 0, "exclusiveMinimum": true}))]
    #[case(json!({"const": "fixed"}), json!({"enum": ["fixed"]}))]
    #[case(json!({"examples": ["a", "b"]}), json!({"example": "a"}))]
    #[case(json!({"type": "array", "prefixItems": [{"type": "integer"}]}), json!({"type": "array", "items": {"type": "integer"}}))]
    fn normalize_schema_rewrites_31_keywords(#[case] input: Value, #[case] expected: Value) {
        assert_eq!(normalized(input), expected);
    }

    #[test]
    fn normalize_schema_recurses_into_subschemas() {
        let schema = json!({
            "type": "object",
            "properties": {"tag": {"type": ["string", "null"]}},
            "items": {"const": 1},
            "allOf": [{"type": "null"}]
        });
        assert_eq!(
            normalized(schema),
            json!({
                "type": "object",
                "properties": {"tag": {"type": "string", "nullable": true}},
                "items": {"enum": [1]},
                "allOf": [{"nullable": true}]
            })
        );
    }

    #[test]
    fn normalize_schema_is_idempotent() {
        let once = normalized(json!({"type": ["integer", "null"], "exclusiveMaximum": 5}));
        assert_eq!(normalized(once.clone()), once);
    }

    #[test]
    fn to_openapi30_rewrites_document_level_differences() {
        let document = json!({
            "openapi": "3.1.0",
            "info": {"title": "T", "version": "1", "summary": "s", "license": {"identifier": "MIT"}},
            "webhooks": {"ping": {}},
            "jsonSchemaDialect": "https://json-schema.org/draft/2020-12/schema",
            "components": {
                "pathItems": {"Pets": {"get": {"responses": {}}}},
                "securitySchemes": {"mtls": {"type": "mutualTLS"}}
            }
        });
        let converted = to_openapi30(document);
        assert_eq!(converted["openapi"], "3.0.3");
        assert_eq!(converted["paths"], json!({}));
        assert_eq!(converted["info"], json!({"title": "T", "version": "1"}));
        assert!(
            converted.get("webhooks").is_none() && converted.get("jsonSchemaDialect").is_none()
        );
        assert_eq!(
            converted["components"]["securitySchemes"]["mtls"],
            json!({"type": "http", "scheme": "mutual-tls"})
        );
    }

    #[test]
    fn to_openapi30_inlines_path_item_references() {
        let document = json!({
            "openapi": "3.1.0",
            "paths": {"/pets": {"$ref": "#/components/pathItems/Pets"}},
            "components": {"pathItems": {"Pets": {"get": {"operationId": "listPets"}}}}
        });
        let converted = to_openapi30(document);
        assert_eq!(
            converted["paths"]["/pets"],
            json!({"get": {"operationId": "listPets"}})
        );
        assert!(converted["components"].get("pathItems").is_none());
    }

    #[test]
    fn to_openapi30_normalizes_schemas_wherever_they_appear() {
        let document = json!({
            "openapi": "3.1.0",
            "paths": {"/a": {"get": {"parameters": [{"name": "q", "in": "query", "schema": {"type": ["string", "null"]}}]}}},
            "components": {"schemas": {"Tag": {"type": ["string", "null"]}}}
        });
        let converted = to_openapi30(document);
        let nullable_string = json!({"type": "string", "nullable": true});
        assert_eq!(
            converted["paths"]["/a"]["get"]["parameters"][0]["schema"],
            nullable_string
        );
        assert_eq!(converted["components"]["schemas"]["Tag"], nullable_string);
    }
}
