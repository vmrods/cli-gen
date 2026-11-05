//! Helpers over JSON Schema values kept in [`crate::ApiSpec::schemas`].

use std::collections::BTreeMap;

use serde_json::{Map, Value};

const LOCAL_SCHEMA_PREFIX: &str = "#/components/schemas/";
/// Bounds `$ref` chains and recursive schemas; deeper refs are left in place.
const MAX_DEPTH: usize = 8;

/// Follows top-level local `$ref`s to the schema they name.
pub(crate) fn resolve<'a>(schema: &'a Value, schemas: &'a BTreeMap<String, Value>) -> &'a Value {
    let mut current = schema;
    for _ in 0..MAX_DEPTH {
        match local_target(current, schemas) {
            Some(next) => current = next,
            None => break,
        }
    }
    current
}

/// Returns a copy with local `$ref`s replaced by their targets, so the
/// schema reads on its own (for `--describe`). Recursion stops at `MAX_DEPTH`.
pub(crate) fn inline(schema: &Value, schemas: &BTreeMap<String, Value>) -> Value {
    inline_at(schema, schemas, 0)
}

fn inline_at(schema: &Value, schemas: &BTreeMap<String, Value>, depth: usize) -> Value {
    if depth >= MAX_DEPTH {
        return schema.clone();
    }
    if let Some(target) = local_target(schema, schemas) {
        return inline_at(target, schemas, depth + 1);
    }
    match schema {
        Value::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(key, value)| (key.clone(), inline_at(value, schemas, depth + 1)))
                .collect::<Map<_, _>>(),
        ),
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| inline_at(item, schemas, depth + 1))
                .collect(),
        ),
        scalar => scalar.clone(),
    }
}

fn local_target<'a>(schema: &Value, schemas: &'a BTreeMap<String, Value>) -> Option<&'a Value> {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(|reference| reference.strip_prefix(LOCAL_SCHEMA_PREFIX))
        .and_then(|name| schemas.get(name))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn schemas() -> BTreeMap<String, Value> {
        BTreeMap::from([
            (
                "Pet".to_owned(),
                json!({"type": "object", "properties": {"tag": {"$ref": "#/components/schemas/Tag"}}}),
            ),
            ("Tag".to_owned(), json!({"type": "string"})),
            (
                "Alias".to_owned(),
                json!({"$ref": "#/components/schemas/Tag"}),
            ),
            (
                "Node".to_owned(),
                json!({"type": "object", "properties": {"next": {"$ref": "#/components/schemas/Node"}}}),
            ),
        ])
    }

    #[test]
    fn resolve_follows_ref_chains() {
        let schemas = schemas();
        assert_eq!(
            resolve(&json!({"$ref": "#/components/schemas/Alias"}), &schemas),
            &json!({"type": "string"})
        );
    }

    #[test]
    fn resolve_returns_unknown_refs_unchanged() {
        let schema = json!({"$ref": "#/components/schemas/Missing"});
        assert_eq!(resolve(&schema, &schemas()), &schema);
    }

    #[test]
    fn inline_replaces_nested_refs() {
        let inlined = inline(&json!({"$ref": "#/components/schemas/Pet"}), &schemas());
        assert_eq!(
            inlined,
            json!({"type": "object", "properties": {"tag": {"type": "string"}}})
        );
    }

    #[test]
    fn inline_terminates_on_recursive_schemas() {
        let inlined = inline(&json!({"$ref": "#/components/schemas/Node"}), &schemas());
        assert!(
            inlined.to_string().contains("$ref"),
            "recursion should stop and keep a $ref"
        );
    }
}
