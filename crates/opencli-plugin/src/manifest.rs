//! The `plugin.toml` format, validated at install and at load.

use std::collections::BTreeMap;
use std::path::Path;

use opencli_core::{ApiName, kebab_case};
use serde::Deserialize;
use thiserror::Error;

/// A validated manifest plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plugin {
    pub name: ApiName,
    pub version: String,
    pub description: Option<String>,
    /// The registered API these additions apply to.
    pub api: ApiName,
    /// `opencli <api> <alias> [args…]` runs `opencli <api> <command> <args> [args…]`.
    pub aliases: BTreeMap<String, Alias>,
    /// Table columns for a command's response, keyed by command path (`issues list`).
    pub formats: BTreeMap<String, Format>,
    /// Programs that supply a credential, keyed by security scheme name.
    pub credentials: BTreeMap<String, CredentialProvider>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Alias {
    /// Command path inside the API, e.g. `issues list`.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Format {
    pub columns: Vec<Column>,
}

/// A dot-separated JSON path (`fields.status.name`, `labels.0`), optionally
/// with its own header.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Column {
    Path(String),
    Labeled { path: String, header: String },
}

impl Column {
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Path(path) | Self::Labeled { path, .. } => path,
        }
    }

    /// The explicit header, or the path's last segment upper-cased.
    #[must_use]
    pub fn header(&self) -> String {
        match self {
            Self::Labeled { header, .. } => header.clone(),
            Self::Path(path) => path.rsplit('.').next().unwrap_or(path).to_ascii_uppercase(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialProvider {
    /// A program name found on `PATH`, or an absolute path. Run directly,
    /// never through a shell.
    pub exec: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManifestError {
    #[error("not a valid plugin manifest: {0}")]
    Syntax(String),
    #[error("invalid plugin manifest: {}", .0.join("; "))]
    Invalid(Vec<String>),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    name: String,
    version: String,
    description: Option<String>,
    api: String,
    #[serde(default)]
    aliases: BTreeMap<String, Alias>,
    #[serde(default)]
    formats: BTreeMap<String, Format>,
    #[serde(default)]
    credentials: BTreeMap<String, CredentialProvider>,
}

/// # Errors
/// [`ManifestError`] for TOML errors and every validation problem at once.
pub fn parse_manifest(text: &str) -> Result<Plugin, ManifestError> {
    let raw: RawManifest =
        toml::from_str(text).map_err(|error| ManifestError::Syntax(error.message().to_owned()))?;
    let mut problems = Vec::new();
    let name = ApiName::parse(&raw.name)
        .map_err(|error| problems.push(format!("name: {error}")))
        .ok();
    let api = ApiName::parse(&raw.api)
        .map_err(|error| problems.push(format!("api: {error}")))
        .ok();
    if raw.version.trim().is_empty() {
        problems.push("version must not be empty".to_owned());
    }
    for (alias, definition) in &raw.aliases {
        if alias.is_empty() || kebab_case(alias) != *alias {
            problems.push(format!("alias `{alias}` must be one kebab-case word"));
        }
        if definition.command.split_whitespace().next().is_none() {
            problems.push(format!("alias `{alias}` needs a command"));
        }
    }
    for (command, format) in &raw.formats {
        if format.columns.is_empty() {
            problems.push(format!("format for `{command}` needs at least one column"));
        }
        if format
            .columns
            .iter()
            .any(|column| column.path().split('.').any(str::is_empty))
        {
            problems.push(format!("format for `{command}` has an empty column path"));
        }
    }
    for (scheme, provider) in &raw.credentials {
        if !is_allowed_program(&provider.exec) {
            problems.push(format!(
                "credential provider for `{scheme}` must be a program name on PATH or an absolute path, not `{}`",
                provider.exec
            ));
        }
    }
    match (name, api) {
        (Some(name), Some(api)) if problems.is_empty() => Ok(Plugin {
            name,
            version: raw.version,
            description: raw.description,
            api,
            aliases: raw.aliases,
            formats: raw.formats,
            credentials: raw.credentials,
        }),
        _ => Err(ManifestError::Invalid(problems)),
    }
}

/// Relative paths like `./x` or `bin/x` would depend on the caller's working
/// directory, so only bare names and absolute paths are accepted.
fn is_allowed_program(exec: &str) -> bool {
    let has_separator = exec.contains('/') || exec.contains('\\');
    !exec.trim().is_empty() && (!has_separator || Path::new(exec).is_absolute())
}

#[cfg(test)]
mod tests {
    use rstest::rstest;

    use super::*;

    const MANIFEST: &str = r#"
name = "jira-helpers"
version = "1.0.0"
description = "Shortcuts for Jira"
api = "jira"

[aliases.mine]
command = "issues list"
args = ["--assignee", "me"]

[formats."issues list"]
columns = ["key", { path = "fields.status.name", header = "STATUS" }]

[credentials.oauth]
exec = "jira-token"
args = ["--profile", "work"]
"#;

    #[test]
    fn parse_manifest_reads_every_section() {
        let plugin = parse_manifest(MANIFEST).unwrap();
        assert_eq!(plugin.name.as_str(), "jira-helpers");
        assert_eq!(plugin.api.as_str(), "jira");
        assert_eq!(plugin.aliases["mine"].command, "issues list");
        assert_eq!(plugin.aliases["mine"].args, ["--assignee", "me"]);
        let columns = &plugin.formats["issues list"].columns;
        assert_eq!(
            (columns[0].path(), columns[0].header().as_str()),
            ("key", "KEY")
        );
        assert_eq!(
            (columns[1].path(), columns[1].header().as_str()),
            ("fields.status.name", "STATUS")
        );
        assert_eq!(plugin.credentials["oauth"].exec, "jira-token");
    }

    #[test]
    fn parse_manifest_needs_only_identity_fields() {
        let plugin = parse_manifest("name = \"x\"\nversion = \"1\"\napi = \"crm\"\n").unwrap();
        assert!(
            plugin.aliases.is_empty() && plugin.formats.is_empty() && plugin.credentials.is_empty()
        );
    }

    #[test]
    fn parse_manifest_rejects_unknown_keys_and_bad_toml() {
        assert!(matches!(
            parse_manifest("name = \"x\"\nversion = \"1\"\napi = \"crm\"\nhooks = 1\n"),
            Err(ManifestError::Syntax(_))
        ));
        assert!(matches!(
            parse_manifest("name = "),
            Err(ManifestError::Syntax(_))
        ));
    }

    #[test]
    fn parse_manifest_reports_every_problem() {
        let text = r#"
name = "Bad Name"
version = ""
api = "api"
[aliases."two words"]
command = " "
[formats."x list"]
columns = []
[credentials.key]
exec = "./relative/tool"
"#;
        let Err(ManifestError::Invalid(problems)) = parse_manifest(text) else {
            panic!("expected validation problems");
        };
        assert_eq!(problems.len(), 7, "{problems:#?}");
    }

    /// "Absolute" differs by platform: Windows needs a drive letter.
    #[cfg(windows)]
    const ABSOLUTE_PROGRAM: &str = r"C:\Tools\jira-token.exe";
    #[cfg(not(windows))]
    const ABSOLUTE_PROGRAM: &str = "/usr/local/bin/jira-token";

    #[rstest]
    #[case("jira-token", true)]
    #[case(ABSOLUTE_PROGRAM, true)]
    #[case("./jira-token", false)]
    #[case("bin/jira-token", false)]
    #[case(r"bin\jira-token", false)]
    #[case("", false)]
    fn programs_must_be_bare_names_or_absolute(#[case] exec: &str, #[case] allowed: bool) {
        assert_eq!(is_allowed_program(exec), allowed);
    }
}
