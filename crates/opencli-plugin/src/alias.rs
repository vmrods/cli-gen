use std::collections::BTreeMap;
use std::ffi::OsString;

use crate::Alias;

/// Rewrites `[…, api, alias, rest…]` into `[…, api, command…, alias args…, rest…]`.
/// Returns `None` when the word after the API isn't an alias.
#[must_use]
pub fn expand_alias(
    args: &[OsString],
    api_index: usize,
    aliases: &BTreeMap<String, &Alias>,
) -> Option<Vec<OsString>> {
    let word = args.get(api_index + 1)?.to_str()?;
    let alias = aliases.get(word)?;
    let mut expanded: Vec<OsString> = args[..=api_index].to_vec();
    expanded.extend(alias.command.split_whitespace().map(OsString::from));
    expanded.extend(alias.args.iter().map(OsString::from));
    expanded.extend_from_slice(&args[api_index + 2..]);
    Some(expanded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(raw: &[&str]) -> Vec<OsString> {
        raw.iter().map(OsString::from).collect()
    }

    #[test]
    fn expand_alias_splices_command_and_arguments() {
        let alias = Alias {
            command: "issues list".into(),
            args: vec!["--assignee".into(), "me".into()],
            description: None,
        };
        let aliases = BTreeMap::from([("mine".to_owned(), &alias)]);
        let expanded = expand_alias(
            &args(&["opencli", "-o", "json", "jira", "mine", "--limit", "5"]),
            3,
            &aliases,
        )
        .unwrap();
        assert_eq!(
            expanded,
            args(&[
                "opencli",
                "-o",
                "json",
                "jira",
                "issues",
                "list",
                "--assignee",
                "me",
                "--limit",
                "5"
            ])
        );
    }

    #[test]
    fn expand_alias_ignores_non_aliases() {
        let aliases = BTreeMap::new();
        assert_eq!(
            expand_alias(&args(&["opencli", "jira", "issues", "list"]), 1, &aliases),
            None
        );
        assert_eq!(expand_alias(&args(&["opencli", "jira"]), 1, &aliases), None);
    }
}
