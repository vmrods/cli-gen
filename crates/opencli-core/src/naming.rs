//! Command words from each operation's path and method (ADR 0009).

use crate::{HttpMethod, Operation};

/// Leading path segments that namespace an API rather than name a resource.
/// Stripped when every path starts with them. Versions (`v1`) stay, so a
/// later `v2` adds commands beside them instead of renaming them.
fn is_namespace_segment(segment: &str) -> bool {
    matches!(
        segment.to_ascii_lowercase().as_str(),
        "api" | "apis" | "rest"
    )
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

/// Number of leading namespace segments (`api`, `rest`, …) on one path,
/// never so many that nothing would be left. Each path is stripped on its own,
/// so `/api/v1/users` and `/health` give `v1 users` and `health`; a clash this
/// causes (`/api/users` beside `/users`) fails the import like any other.
pub(crate) fn namespace_prefix_length(path: &[Segment<'_>]) -> usize {
    path.iter()
        .take(path.len().saturating_sub(1))
        .take_while(
            |segment| matches!(segment, Segment::Static(word) if is_namespace_segment(word)),
        )
        .count()
}

/// Each operation's command words (ADR 0009): the path's fixed segments, then
/// the method, except for GET. `x-cli-resource` replaces the path words and
/// `x-cli-name` the method word.
pub(crate) fn command_words(operations: &[Operation]) -> Vec<Vec<String>> {
    operations
        .iter()
        .map(|operation| {
            let path = segments(&operation.path);
            words(operation, &path[namespace_prefix_length(&path)..])
        })
        .collect()
}

fn words(operation: &Operation, path: &[Segment<'_>]) -> Vec<String> {
    let mut words: Vec<String> = match &operation.cli.resource {
        Some(hinted) => hinted.iter().map(|word| kebab_case(word)).collect(),
        None => path
            .iter()
            .filter_map(|segment| match segment {
                Segment::Static(word) => Some(kebab_case(word)),
                Segment::Parameter => None,
            })
            .collect(),
    };
    words.retain(|word| !word.is_empty());
    let hinted_method = operation
        .cli
        .name
        .as_deref()
        .map(kebab_case)
        .filter(|name| !name.is_empty());
    if let Some(method) = hinted_method.or_else(|| method_word(operation.method)) {
        words.push(method);
    }
    words
}

/// GET has no word: a command without a method word only reads.
fn method_word(method: HttpMethod) -> Option<String> {
    (method != HttpMethod::Get).then(|| method.as_str().to_ascii_lowercase())
}

/// Path parameter names in template order (`/a/{x}/b/{y}` → `x`, `y`).
pub(crate) fn template_parameters(path: &str) -> Vec<&str> {
    path.split('{')
        .skip(1)
        .filter_map(|segment| segment.split_once('}').map(|(name, _)| name))
        .collect()
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

    fn operation(method: HttpMethod, path: &str) -> Operation {
        Operation {
            operation_id: None,
            method,
            path: path.into(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: Vec::new(),
            request_body: None,
            security: Vec::new(),
            cli: crate::CliHints::default(),
        }
    }

    #[test]
    fn command_words_follow_the_path_and_name_every_method_but_get() {
        let operations = [
            operation(HttpMethod::Get, "/api/v1/countries"),
            operation(HttpMethod::Get, "/api/v1/countries/{id}"),
            operation(HttpMethod::Post, "/api/v1/countries"),
            operation(HttpMethod::Delete, "/api/v1/countries/{id}"),
            operation(HttpMethod::Get, "/api/v1/countries/{id}/cities"),
            operation(HttpMethod::Post, "/api/v1/account/login"),
            operation(
                HttpMethod::Get,
                "/api/v1/tenantBranding/public/{tenantId}/{appKey}",
            ),
        ];
        let words: Vec<String> = command_words(&operations)
            .iter()
            .map(|words| words.join(" "))
            .collect();
        assert_eq!(
            words,
            [
                "v1 countries",
                "v1 countries",
                "v1 countries post",
                "v1 countries delete",
                "v1 countries cities",
                "v1 account login post",
                "v1 tenant-branding public",
            ]
        );
    }

    #[test]
    fn command_words_take_hints_for_the_words_and_the_method_word() {
        let mut renamed = operation(HttpMethod::Delete, "/items");
        renamed.cli.name = Some("deleteCollection".to_owned());
        let mut moved = operation(HttpMethod::Get, "/a/b");
        moved.cli.resource = Some(vec!["things".to_owned()]);
        moved.cli.name = Some("search".to_owned());
        assert_eq!(
            command_words(&[renamed, moved]),
            [vec!["items", "delete-collection"], vec!["things", "search"]]
        );
    }

    #[rstest]
    #[case("/users/{id}/repos/{repo_name}", &["id", "repo_name"])]
    #[case("/users", &[])]
    fn template_parameters_lists_names_in_order(#[case] path: &str, #[case] expected: &[&str]) {
        assert_eq!(template_parameters(path), expected);
    }

    #[rstest]
    #[case("/api/v1/users", 1)]
    #[case("/rest/api/users/{id}", 2)]
    #[case("/v2/users", 0)]
    #[case("/health", 0)]
    #[case("/api", 0)]
    #[case("/api/{id}", 1)]
    fn namespace_prefix_length_strips_leading_namespaces(
        #[case] path: &str,
        #[case] expected: usize,
    ) {
        assert_eq!(namespace_prefix_length(&segments(path)), expected);
    }
}
