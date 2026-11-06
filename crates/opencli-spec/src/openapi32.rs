//! OpenAPI 3.2 → 3.1 shape. 3.2 shares 3.1's schema dialect and is otherwise
//! additive; constructs OpenCLI can't express as commands are removed here,
//! after which the 3.1 normalizer takes over (ADR 0001).

use serde_json::{Map, Value};

const OPENAPI_31: &str = "3.1.1";
const QUERY_STRING_LOCATION: &str = "querystring";
const PARAMETERS_PREFIX: &str = "#/components/parameters/";

pub(crate) fn to_openapi31(mut document: Value) -> Value {
    let Some(root) = document.as_object_mut() else {
        return document;
    };
    root.insert("openapi".into(), Value::String(OPENAPI_31.into()));
    root.remove("$self");

    let query_string_parameters = remove_query_string_components(root);
    if let Some(paths) = root.get_mut("paths").and_then(Value::as_object_mut) {
        for item in paths.values_mut().filter_map(Value::as_object_mut) {
            // The QUERY method and custom methods have no command form.
            item.remove("query");
            item.remove("additionalOperations");
            drop_query_string_parameters(item, &query_string_parameters);
            for operation in item.values_mut().filter_map(Value::as_object_mut) {
                drop_query_string_parameters(operation, &query_string_parameters);
            }
        }
    }
    remove_device_authorization_flows(root);
    document
}

/// Removes `components.parameters` entries with `in: querystring`, returning their names.
fn remove_query_string_components(root: &mut Map<String, Value>) -> Vec<String> {
    let Some(parameters) = root
        .get_mut("components")
        .and_then(|components| components.get_mut("parameters"))
        .and_then(Value::as_object_mut)
    else {
        return Vec::new();
    };
    let names: Vec<String> = parameters
        .iter()
        .filter(|(_, parameter)| is_query_string(parameter))
        .map(|(name, _)| name.clone())
        .collect();
    for name in &names {
        parameters.remove(name);
    }
    names
}

/// `in: querystring` (the whole query string as one value) has no flag form.
fn drop_query_string_parameters(owner: &mut Map<String, Value>, removed_components: &[String]) {
    let Some(Value::Array(parameters)) = owner.get_mut("parameters") else {
        return;
    };
    parameters.retain(|parameter| {
        let referenced = parameter
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|reference| reference.strip_prefix(PARAMETERS_PREFIX))
            .is_some_and(|name| removed_components.iter().any(|removed| removed == name));
        !is_query_string(parameter) && !referenced
    });
}

fn is_query_string(parameter: &Value) -> bool {
    parameter.get("in").and_then(Value::as_str) == Some(QUERY_STRING_LOCATION)
}

/// The device authorization grant isn't one OpenCLI implements.
fn remove_device_authorization_flows(root: &mut Map<String, Value>) {
    let Some(schemes) = root
        .get_mut("components")
        .and_then(|components| components.get_mut("securitySchemes"))
        .and_then(Value::as_object_mut)
    else {
        return;
    };
    for scheme in schemes.values_mut() {
        if let Some(flows) = scheme.get_mut("flows").and_then(Value::as_object_mut) {
            flows.remove("deviceAuthorization");
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn to_openapi31_drops_constructs_without_a_command_form() {
        let document = json!({
            "openapi": "3.2.0",
            "$self": "https://example.com/openapi.yaml",
            "paths": {"/pets": {
                "parameters": [{"name": "raw", "in": "querystring", "content": {}}],
                "query": {"operationId": "searchPets"},
                "additionalOperations": {"LINK": {}},
                "get": {"parameters": [
                    {"name": "limit", "in": "query", "schema": {"type": "integer"}},
                    {"$ref": "#/components/parameters/Filter"}
                ]}
            }},
            "components": {
                "parameters": {"Filter": {"name": "filter", "in": "querystring", "content": {}}},
                "securitySchemes": {"oauth": {"type": "oauth2", "flows": {
                    "deviceAuthorization": {"deviceAuthorizationUrl": "https://a/d", "tokenUrl": "https://a/t", "scopes": {}},
                    "clientCredentials": {"tokenUrl": "https://a/t", "scopes": {}}
                }}}
            }
        });
        let converted = to_openapi31(document);
        assert_eq!(converted["openapi"], "3.1.1");
        assert!(converted.get("$self").is_none());
        let item = &converted["paths"]["/pets"];
        assert!(item.get("query").is_none() && item.get("additionalOperations").is_none());
        assert_eq!(item["parameters"], json!([]));
        assert_eq!(
            item["get"]["parameters"],
            json!([{"name": "limit", "in": "query", "schema": {"type": "integer"}}])
        );
        assert_eq!(converted["components"]["parameters"], json!({}));
        assert_eq!(
            converted["components"]["securitySchemes"]["oauth"]["flows"],
            json!({"clientCredentials": {"tokenUrl": "https://a/t", "scopes": {}}})
        );
    }
}
