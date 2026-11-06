//! Command-path derivation from HTTP method and path (ADR 0002).

use std::collections::{HashMap, HashSet};

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
    let mut operations_per_path: HashMap<String, usize> = HashMap::new();
    let mut parents: HashSet<String> = HashSet::new();
    for operation in operations {
        let key = template_key(&operation.path);
        // Every proper prefix of a path is a parent: `/a/{}/b` makes `/a/{}` and `/a`.
        let mut prefix_end = key.len();
        while let Some(slash) = key[..prefix_end].rfind('/') {
            if slash > 0 {
                parents.insert(key[..slash].to_owned());
            }
            prefix_end = slash;
        }
        *operations_per_path.entry(key).or_default() += 1;
    }
    operations
        .iter()
        .zip(&parsed)
        .map(|(operation, path)| {
            let key = template_key(&operation.path);
            // An action stands alone: nothing else at its path, nothing beneath it.
            let may_be_action = operations_per_path[&key] == 1 && !parents.contains(&key);
            draft(operation, &path[prefix..], may_be_action)
        })
        .collect()
}

/// The path with parameter names erased (`/users/{id}` → `/users/{}`), since
/// specs name the same position differently (`{user_id}`, `{username}`).
fn template_key(path: &str) -> String {
    let mut key = String::with_capacity(path.len());
    let mut in_parameter = false;
    for character in path.trim_end_matches('/').chars() {
        match character {
            '{' => {
                in_parameter = true;
                key.push_str("{}");
            }
            '}' => in_parameter = false,
            other if !in_parameter => key.push(other),
            _ => {}
        }
    }
    key
}

fn draft(operation: &Operation, path: &[Segment<'_>], may_be_action: bool) -> Draft {
    let mut resources: Vec<String> = path
        .iter()
        .filter_map(|segment| match segment {
            Segment::Static(word) => Some(kebab_case(word)),
            Segment::Parameter => None,
        })
        .filter(|word| !word.is_empty())
        .collect();
    let ends_with_parameter = path.last() == Some(&Segment::Parameter);
    let mut verb = pattern_verb(operation.method, ends_with_parameter).to_owned();

    // `POST /users/{id}/activate` is an action on a user. `POST /repos/{r}/check-runs`
    // with `GET …/check-runs/{id}` beside it is a create on a collection.
    let is_action = operation.method == HttpMethod::Post
        && !ends_with_parameter
        && may_be_action
        && path.len() >= 2
        && path[path.len() - 2] == Segment::Parameter;
    // `GET /customers/search` (alone at its path, nothing beneath it, under a
    // named parent) reads as a noun query: `customers search`, `search code`.
    let is_query = operation.method == HttpMethod::Get
        && !ends_with_parameter
        && may_be_action
        && path.len() >= 2
        && matches!(path[path.len() - 2], Segment::Static(_));
    // With an explicit `x-cli-resource`, the path no longer forms the command,
    // so no path word is taken as the verb.
    if (is_action || is_query)
        && operation.cli.resource.is_none()
        && let Some(action) = resources.pop()
    {
        verb = action;
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
    words.push(hinted_verb.unwrap_or(verb));
    Draft { words, from_hints }
}

const fn pattern_verb(method: HttpMethod, ends_with_parameter: bool) -> &'static str {
    match (method, ends_with_parameter) {
        (HttpMethod::Get, false) => "list",
        (HttpMethod::Get, true) => "get",
        (HttpMethod::Post, false) => "create",
        // Stripe-style APIs update items with POST.
        (HttpMethod::Post, true) | (HttpMethod::Patch, _) => "update",
        (HttpMethod::Put, _) => "replace",
        (HttpMethod::Delete, _) => "delete",
    }
}

/// The `operationId`, or the lowercased method when there is none.
pub(crate) fn fallback_verb(operation: &Operation) -> String {
    operation
        .operation_id
        .as_deref()
        .map(kebab_case)
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| operation.method.as_str().to_ascii_lowercase())
}

/// A shorter `operationId`-based verb: only the part after the last `/`
/// (GitHub's `tag/verb-noun`), without leading words that repeat the
/// command's resource words (`actions/set-allowed-actions` under
/// `orgs actions permissions` → `set-allowed-actions`). The method word stays:
/// in `deleteUser`, "delete" is the meaning. `None` when nothing is left.
pub(crate) fn short_fallback_verb(operation: &Operation, resources: &[String]) -> Option<String> {
    let id = operation.operation_id.as_deref()?;
    let tail = kebab_case(id.rsplit('/').next().unwrap_or(id));
    let words: Vec<&str> = tail.split('-').collect();
    let repeated = |word: &str| {
        resources
            .iter()
            .any(|resource| resource.split('-').any(|part| part == word))
    };
    let start = words.iter().position(|word| !repeated(word))?;
    let short = words[start..].join("-");
    (!short.is_empty()).then_some(short)
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

    fn operation(method: HttpMethod, id: &str) -> Operation {
        Operation {
            operation_id: Some(id.to_owned()),
            method,
            path: "/x".into(),
            summary: None,
            description: None,
            deprecated: false,
            parameters: Vec::new(),
            request_body: None,
            security: Vec::new(),
            cli: crate::CliHints::default(),
        }
    }

    #[rstest]
    #[case("actions/set-allowed-actions-organization", &["orgs", "actions", "permissions"], Some("set-allowed-actions-organization"))]
    #[case("gists/get-revision", &["gists"], Some("get-revision"))]
    #[case("deleteUser", &["users"], Some("delete-user"))]
    #[case("listUsers", &["list", "users"], None)]
    #[case("PostAccountsAccountPeople", &["accounts", "people"], Some("post-accounts-account-people"))]
    fn short_fallback_verb_drops_leading_resource_words(
        #[case] id: &str,
        #[case] resources: &[&str],
        #[case] expected: Option<&str>,
    ) {
        let resources: Vec<String> = resources.iter().map(|&word| word.to_owned()).collect();
        assert_eq!(
            short_fallback_verb(&operation(HttpMethod::Get, id), &resources).as_deref(),
            expected
        );
    }

    #[rstest]
    #[case("/users/{id}/repos/{repo_name}", "/users/{}/repos/{}")]
    #[case("/users/", "/users")]
    #[case("/a", "/a")]
    fn template_key_erases_parameter_names(#[case] path: &str, #[case] expected: &str) {
        assert_eq!(template_key(path), expected);
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
