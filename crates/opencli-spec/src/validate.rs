//! Format-independent checks on the normalized model.

use std::collections::HashMap;

use opencli_core::{ApiSpec, Operation, ParameterLocation};

use crate::ValidationIssue;

pub(crate) fn validate(spec: &ApiSpec) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if spec.operations.is_empty() {
        issues.push(ValidationIssue::new(
            "paths",
            "the document defines no GET, POST, PUT, PATCH or DELETE operations",
        ));
    }
    for operation in &spec.operations {
        check_path_parameters(operation, &mut issues);
        check_security_references(spec, operation, &mut issues);
    }
    check_duplicate_operation_ids(spec, &mut issues);
    issues
}

fn location(operation: &Operation) -> String {
    format!("{} {}", operation.method, operation.path)
}

fn template_parameters(path: &str) -> impl Iterator<Item = &str> {
    path.split('{')
        .skip(1)
        .filter_map(|segment| segment.split_once('}').map(|(name, _)| name))
}

fn check_path_parameters(operation: &Operation, issues: &mut Vec<ValidationIssue>) {
    let declared: Vec<_> = operation
        .parameters
        .iter()
        .filter(|parameter| parameter.location == ParameterLocation::Path)
        .collect();
    for name in template_parameters(&operation.path) {
        if !declared.iter().any(|parameter| parameter.name == name) {
            issues.push(ValidationIssue::new(
                location(operation),
                format!("path parameter `{name}` is used in the path but not declared"),
            ));
        }
    }
    for parameter in declared {
        if !template_parameters(&operation.path).any(|name| name == parameter.name) {
            issues.push(ValidationIssue::new(
                location(operation),
                format!(
                    "path parameter `{}` is declared but not used in the path",
                    parameter.name
                ),
            ));
        }
        if !parameter.required {
            issues.push(ValidationIssue::new(
                location(operation),
                format!("path parameter `{}` must be required", parameter.name),
            ));
        }
    }
}

fn check_security_references(
    spec: &ApiSpec,
    operation: &Operation,
    issues: &mut Vec<ValidationIssue>,
) {
    let schemes = operation
        .security
        .iter()
        .flat_map(|requirement| &requirement.schemes);
    for requirement in schemes {
        if !spec.security_schemes.contains_key(&requirement.scheme) {
            issues.push(ValidationIssue::new(
                location(operation),
                format!(
                    "security scheme `{}` is not defined in components.securitySchemes",
                    requirement.scheme
                ),
            ));
        }
    }
}

fn check_duplicate_operation_ids(spec: &ApiSpec, issues: &mut Vec<ValidationIssue>) {
    let mut first_use: HashMap<&str, &Operation> = HashMap::new();
    for operation in &spec.operations {
        let Some(id) = operation.operation_id.as_deref() else {
            continue;
        };
        if let Some(first) = first_use.insert(id, operation) {
            issues.push(ValidationIssue::new(
                location(operation),
                format!("operationId `{id}` is also used by {}", location(first)),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn template_parameters_extracts_names_in_order() {
        let names: Vec<_> = template_parameters("/users/{userId}/orders/{orderId}").collect();
        assert_eq!(names, ["userId", "orderId"]);
    }

    #[test]
    fn template_parameters_is_empty_without_placeholders() {
        assert_eq!(template_parameters("/users").count(), 0);
    }
}
