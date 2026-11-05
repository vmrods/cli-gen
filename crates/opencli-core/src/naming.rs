//! Command-path derivation from HTTP method and path (ADR 0002).

use std::collections::HashMap;

use crate::{HttpMethod, Operation};

/// Leading path segments that version or namespace an API rather than name a
/// resource. Stripped when every path starts with them.
fn is_namespace_segment(segment: &str) -> bool {
    let lowered = segment.to_ascii_lowercase();
    if matches!(lowered.as_str(), "api" | "apis" | "rest") {
        return true;
    }
    lowered.strip_prefix('v').is_some_and(|rest| {
        !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit() || c == '.')
    })
}

/// Converts identifiers like `listPets`, `pet_store`, `HTTPServer` or `v1.0` to kebab-case.
#[must_use]
pub fn kebab_case(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    for (index, &current) in chars.iter().enumerate() {
        if !current.is_ascii_alphanumeric() {
            push_separator(&mut out);
            continue;
        }
        if current.is_ascii_uppercase() && index > 0 {
            let previous = chars[index - 1];
            let next_is_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
            if previous.is_ascii_lowercase()
                || previous.is_ascii_digit()
                || (previous.is_ascii_uppercase() && next_is_lower)
            {
                push_separator(&mut out);
            }
        }
        out.push(current.to_ascii_lowercase());
    }
    out.trim_end_matches('-').to_owned()
}

fn push_separator(out: &mut String) {
    if !out.is_empty() && !out.ends_with('-') {
        out.push('-');
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Segment<'a> {
    Static(&'a str),
    Parameter,
}

pub(crate) fn segments(path: &str) -> Vec<Segment<'_>> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            if segment.contains('{') {
                Segment::Parameter
            } else {
                Segment::Static(segment)
            }
        })
        .collect()
}

/// Number of leading namespace segments (`api`, `v1`, …) shared by every path,
/// never so many that a path would be left empty.
pub(crate) fn namespace_prefix_length(paths: &[Vec<Segment<'_>>]) -> usize {
    let Some(first) = paths.first() else {
        return 0;
    };
    let mut length = 0;
    while let Some(&Segment::Static(word)) = first.get(length) {
        let shared = is_namespace_segment(word)
            && paths
                .iter()
                .all(|path| path.len() > length + 1 && path[length] == Segment::Static(word));
        if !shared {
            break;
        }
        length += 1;
    }
    length
}

/// A proposed name before conflict resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Draft {
    pub words: Vec<String>,
    /// Hint-derived names are never renamed during conflict resolution.
    pub from_hints: bool,
}

pub(crate) fn draft_names(operations: &[Operation]) -> Vec<Draft> {
    let parsed: Vec<Vec<Segment<'_>>> = operations
        .iter()
        .map(|operation| segments(&operation.path))
        .collect();
    let prefix = namespace_prefix_length(&parsed);
    let mut operations_per_path: HashMap<&str, usize> = HashMap::new();
    for operation in operations {
        *operations_per_path
            .entry(operation.path.as_str())
            .or_default() += 1;
    }
    operations
        .iter()
        .zip(&parsed)
        .map(|(operation, path)| {
            let shares_path = operations_per_path[operation.path.as_str()] > 1;
            draft(operation, &path[prefix..], shares_path)
        })
        .collect()
}

fn draft(operation: &Operation, path: &[Segment<'_>], shares_path: bool) -> Draft {
    let mut resources: Vec<String> = path
        .iter()
        .filter_map(|segment| match segment {
            Segment::Static(word) => Some(kebab_case(word)),
            Segment::Parameter => None,
        })
        .filter(|word| !word.is_empty())
        .collect();
    let ends_with_parameter = path.last() == Some(&Segment::Parameter);
    let mut verb = pattern_verb(operation.method, ends_with_parameter).map(str::to_owned);

    // `POST /users/{id}/activate` is an action on a user, not a collection.
    let is_action = operation.method == HttpMethod::Post
        && !ends_with_parameter
        && !shares_path
        && path.len() >= 2
        && path[path.len() - 2] == Segment::Parameter;
    if is_action && let Some(action) = resources.pop() {
        verb = Some(action);
    }

    let hinted_resource = operation.cli.resource.as_ref().map(|words| {
        words
            .iter()
            .map(|word| kebab_case(word))
            .filter(|word| !word.is_empty())
            .collect()
    });
    let hinted_verb = operation
        .cli
        .name
        .as_deref()
        .map(kebab_case)
        .filter(|verb| !verb.is_empty());
    let from_hints = hinted_resource.is_some() || hinted_verb.is_some();
    let mut words: Vec<String> = hinted_resource.unwrap_or(resources);
    words.push(
        hinted_verb
            .or(verb)
            .unwrap_or_else(|| fallback_verb(operation)),
    );
    Draft { words, from_hints }
}

fn pattern_verb(method: HttpMethod, ends_with_parameter: bool) -> Option<&'static str> {
    match (method, ends_with_parameter) {
        (HttpMethod::Get, false) => Some("list"),
        (HttpMethod::Get, true) => Some("get"),
        (HttpMethod::Post, false) => Some("create"),
        (HttpMethod::Post, true) => None,
        (HttpMethod::Put, _) => Some("replace"),
        (HttpMethod::Patch, _) => Some("update"),
        (HttpMethod::Delete, _) => Some("delete"),
    }
}

pub(crate) fn fallback_verb(operation: &Operation) -> String {
    operation
        .operation_id
        .as_deref()
        .map(kebab_case)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| operation.method.as_str().to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    #[rstest]
    #[case("listPets", "list-pets")]
    #[case("pet_store", "pet-store")]
    #[case("HTTPServer", "http-server")]
    #[case("getV2Users", "get-v2-users")]
    #[case("X-Request-Id", "x-request-id")]
    #[case("v1.0", "v1-0")]
    #[case("  spaced  out ", "spaced-out")]
    #[case("already-kebab", "already-kebab")]
    #[case("$$$", "")]
    fn kebab_case_converts_identifiers(#[case] raw: &str, #[case] expected: &str) {
        assert_eq!(kebab_case(raw), expected);
    }

    #[rstest]
    #[case(&["/api/v1/users", "/api/v1/users/{id}"], 2)]
    #[case(&["/v2/users", "/v2/orders"], 1)]
    #[case(&["/pets", "/pets/{id}"], 0)]
    #[case(&["/api/users", "/orders"], 0)]
    #[case(&["/api"], 0)]
    #[case(&["/api/v1"], 1)]
    fn namespace_prefix_length_strips_shared_namespaces(
        #[case] paths: &[&str],
        #[case] expected: usize,
    ) {
        let parsed: Vec<_> = paths.iter().map(|path| segments(path)).collect();
        assert_eq!(namespace_prefix_length(&parsed), expected);
    }
}
