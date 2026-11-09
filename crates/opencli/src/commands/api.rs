//! `opencli api …`: registering and inspecting APIs (ADR 0002, ADR 0009).

use opencli_auth::CredentialKey;
use opencli_core::{
    ApiName, ArgumentTarget, CommandDiff, CommandSet, CommandSpec, build_commands, diff_commands,
    token_scheme,
};
use opencli_store::{ApiDetails, ApiSummary, SpecImport, Store, VersionChange, VersionSummary};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::audit::{self, AuditEvent};
use crate::cache::{self, CachedCommands};
use crate::cli::{ApiCommand, OutputFormat};
use crate::credentials;
use crate::error::{CliError, StoreFailure};
use crate::output::{Tone, emit, highlight_leading, table};
use crate::paths::Paths;
use crate::server::{self, BaseUrlChoice};
use crate::source::SpecSource;
use crate::{progress, prompt};

const SPEC_PROMPT: &str = "Path or URL of the OpenAPI document: ";

/// `server` is the global `--server`; for `api add` it sets the API's base URL.
pub async fn run(
    command: ApiCommand,
    format: OutputFormat,
    server: Option<&str>,
    paths: &Paths,
) -> Result<(), CliError> {
    let store = Store::open(&paths.database).await?;
    match command {
        ApiCommand::Add { name, spec } => add(&store, paths, &name, spec, server, format).await,
        ApiCommand::SetServer { name, url, clear } => {
            set_server(&store, paths, &name, if clear { None } else { url }, format).await
        }
        ApiCommand::SetCredentials { name, from, clear } => {
            set_credentials(
                &store,
                paths,
                &name,
                if clear { None } else { from },
                format,
            )
            .await
        }
        ApiCommand::List => list(&store, format).await,
        ApiCommand::Show { name } => show(&store, &name, format).await,
        ApiCommand::Commands { name, words } => commands(paths, &name, &words, format).await,
        ApiCommand::Versions { name } => versions(&store, &name, format).await,
        ApiCommand::Refresh {
            name,
            spec,
            dry_run,
        } => refresh(&store, paths, &name, spec, dry_run, format).await,
        ApiCommand::Remove { name, yes } => remove(&store, paths, &name, yes, format).await,
    }
}

async fn add(
    store: &Store,
    paths: &Paths,
    name: &ApiName,
    spec: Option<String>,
    server: Option<&str>,
    format: OutputFormat,
) -> Result<(), CliError> {
    let raw_source = match spec {
        Some(spec) => spec,
        None if prompt::is_interactive() => prompt::ask(SPEC_PROMPT)?,
        None => return Err(CliError::MissingSpec),
    };
    if raw_source.is_empty() {
        return Err(CliError::MissingSpec);
    }
    let source = SpecSource::parse(&raw_source)?;
    let loaded = progress::with_spinner(format!("Loading {raw_source}"), source.load()).await?;
    let spec = opencli_spec::parse(&loaded.text, &loaded.display_name)?;
    // Naming conflicts fail the import before anything is stored (ADR 0002).
    let commands = build_commands(&spec)?;
    let base_url = choose_base_url(name, server, &spec.servers, &loaded.location)?;
    let summary = store
        .add_api(
            name,
            SpecImport {
                source: &loaded.location,
                spec: &spec,
                document: &loaded.text,
                imported_at: OffsetDateTime::now_utc(),
            },
            base_url.as_deref(),
        )
        .await?;
    cache::write(
        paths,
        &CachedCommands::new(name, &loaded.location, base_url.as_deref(), &spec, commands),
    )
    .await;
    tracing::info!(api = %name, operations = summary.operation_count, "api added");
    let event = AuditEvent::ApiAdded {
        api: name.as_str(),
        source: &loaded.location,
    };
    audit::record(paths, &event).await;
    let effective = server::resolve(None, base_url.as_deref(), &spec.servers, &loaded.location)?;
    emit(format, &summary, |summary| {
        format!(
            "Added `{}`: {} {}, {} operations, calling {effective}.",
            summary.name, summary.title, summary.version, summary.operation_count
        )
    })
}

/// Onboarding asks where the API lives (ADR 0002). `--server` answers up
/// front; in a terminal the spec's servers are offered; otherwise the spec's
/// first server is used if it is usable. `None` means "follow the spec".
fn choose_base_url(
    name: &ApiName,
    server: Option<&str>,
    servers: &[String],
    source: &str,
) -> Result<Option<String>, CliError> {
    if let Some(raw) = server {
        return server::normalize(raw).map(Some);
    }
    let candidates = server::candidates(servers, source);
    if !prompt::is_interactive() {
        return if candidates.is_empty() {
            Err(CliError::BaseUrlRequired(name.to_string()))
        } else {
            Ok(None)
        };
    }
    let listed: Vec<String> = candidates
        .iter()
        .enumerate()
        .map(|(index, url)| {
            let note = if index == 0 { "  (spec default)" } else { "" };
            format!("{url}{note}")
        })
        .collect();
    let question = if candidates.is_empty() {
        "Base URL for this API (the spec has no usable server): ".to_owned()
    } else {
        format!("Base URL for `{name}` [Enter for 1, a number, or a URL]: ")
    };
    loop {
        let answer = prompt::ask_listing(&question, &listed)?;
        match server::interpret_answer(&answer, &candidates) {
            Ok(BaseUrlChoice::FollowSpec) => return Ok(None),
            Ok(BaseUrlChoice::Store(url)) => return Ok(Some(url)),
            Err(problem) => prompt::say(&problem)?,
        }
    }
}

#[derive(Serialize)]
struct ServerSet<'a> {
    api: &'a str,
    base_url: Option<&'a str>,
    effective: &'a str,
}

/// Changes (or clears) the stored base URL and rebuilds the command cache.
async fn set_server(
    store: &Store,
    paths: &Paths,
    name: &ApiName,
    url: Option<String>,
    format: OutputFormat,
) -> Result<(), CliError> {
    let base_url = url.as_deref().map(server::normalize).transpose()?;
    store
        .set_base_url(name, base_url.as_deref(), OffsetDateTime::now_utc())
        .await?;
    cache::invalidate(paths, name).await;
    let cached = cache::load_or_build(paths, name)
        .await?
        .ok_or_else(|| StoreFailure::not_found(name))?;
    let effective = server::resolve(
        None,
        base_url.as_deref(),
        &cached.commands.servers,
        &cached.source,
    )?;
    let event = AuditEvent::ApiServerSet {
        api: name.as_str(),
        base_url: base_url.as_deref(),
    };
    audit::record(paths, &event).await;
    let result = ServerSet {
        api: name.as_str(),
        base_url: base_url.as_deref(),
        effective: &effective,
    };
    emit(format, &result, |result| match result.base_url {
        Some(url) => format!("`{}` now calls {url}.", result.api),
        None => format!(
            "`{}` follows the spec's servers again: {}.",
            result.api, result.effective
        ),
    })
}

#[derive(Serialize)]
struct CredentialsSet<'a> {
    api: &'a str,
    credentials_from: Option<&'a str>,
}

/// Links an API to another's stored credential, or unlinks it (ADR 0011).
async fn set_credentials(
    store: &Store,
    paths: &Paths,
    name: &ApiName,
    from: Option<ApiName>,
    format: OutputFormat,
) -> Result<(), CliError> {
    if let Some(from) = &from {
        check_credentials_link(store, paths, name, from).await?;
    }
    store
        .set_credentials_from(name, from.as_ref(), OffsetDateTime::now_utc())
        .await?;
    cache::invalidate(paths, name).await;
    let event = AuditEvent::ApiCredentialsSet {
        api: name.as_str(),
        credentials_from: from.as_ref().map(ApiName::as_str),
    };
    audit::record(paths, &event).await;
    let result = CredentialsSet {
        api: name.as_str(),
        credentials_from: from.as_ref().map(ApiName::as_str),
    };
    emit(format, &result, |result| match result.credentials_from {
        Some(from) => format!(
            "`{}` now uses `{from}`'s credentials. Sign in with `opencli auth login {from}`.",
            result.api
        ),
        None => format!("`{}` uses its own credentials again.", result.api),
    })
}

/// One level only, and both sides need a single token scheme to carry it.
async fn check_credentials_link(
    store: &Store,
    paths: &Paths,
    name: &ApiName,
    from: &ApiName,
) -> Result<(), CliError> {
    let refuse = |reason: String| Err(CliError::CredentialsLink(reason));
    if name == from {
        return refuse(format!("`{name}` can't use its own credentials"));
    }
    let source = store.api_summary(from).await?;
    if let Some(further) = source.credentials_from {
        return refuse(format!(
            "`{from}` already uses `{further}`'s credentials; link `{name}` to `{further}` instead"
        ));
    }
    let dependents = store.apis_using_credentials_of(name).await?;
    if !dependents.is_empty() {
        return refuse(format!(
            "{} use `{name}`'s credentials, so `{name}` can't take them from another API",
            dependents.join(", ")
        ));
    }
    for api in [name, from] {
        let cached = cache::load_or_build(paths, api)
            .await?
            .ok_or_else(|| StoreFailure::not_found(api))?;
        if token_scheme(&cached.commands.security_schemes).is_none() {
            return refuse(format!(
                "`{api}` needs exactly one token scheme (bearer, or an API key in the Authorization header)"
            ));
        }
    }
    Ok(())
}

async fn list(store: &Store, format: OutputFormat) -> Result<(), CliError> {
    let apis = store.list_apis().await?;
    emit(format, &apis, |apis: &Vec<ApiSummary>| {
        if apis.is_empty() {
            return "No APIs registered. Add one with `opencli api add <name> --spec <file-or-url>`."
                .to_owned();
        }
        let rows: Vec<Vec<String>> = apis
            .iter()
            .map(|api| {
                vec![
                    api.name.clone(),
                    api.title.clone(),
                    api.version.clone(),
                    api.operation_count.to_string(),
                    api.source.clone(),
                ]
            })
            .collect();
        table(&["NAME", "TITLE", "VERSION", "OPERATIONS", "SOURCE"], &rows)
    })
}

async fn show(store: &Store, name: &ApiName, format: OutputFormat) -> Result<(), CliError> {
    let details = store.api_details(name).await?;
    emit(format, &details, render_details)
}

fn render_details(details: &ApiDetails) -> String {
    let summary = &details.summary;
    let schemes: Vec<String> = details
        .security_schemes
        .iter()
        .map(|scheme| format!("{} ({})", scheme.name, scheme.kind))
        .collect();
    let operations: Vec<Vec<String>> = details
        .operations
        .iter()
        .map(|operation| {
            let summary_text = match (&operation.summary, operation.deprecated) {
                (Some(summary), true) => format!("{summary} (deprecated)"),
                (None, true) => "(deprecated)".to_owned(),
                (summary, false) => summary.clone().unwrap_or_default(),
            };
            vec![
                operation.method.clone(),
                operation.path.clone(),
                operation.operation_id.clone().unwrap_or_default(),
                summary_text,
            ]
        })
        .collect();
    let mut lines = vec![format!(
        "{}: {} {} ({})",
        summary.name, summary.title, summary.version, summary.format
    )];
    if let Some(description) = &details.description {
        lines.push(description.clone());
    }
    lines.extend([
        String::new(),
        format!("Source:    {}", summary.source),
        format!("Imported:  {}", timestamp(summary.imported_at)),
        format!("Updated:   {}", timestamp(summary.updated_at)),
        format!(
            "Base URL:  {}",
            summary
                .base_url
                .as_deref()
                .unwrap_or("follows the spec's servers")
        ),
        format!("Servers:   {}", or_none(&details.servers.join(", "))),
        format!("Security:  {}", or_none(&schemes.join(", "))),
        format!(
            "Credentials: {}",
            summary
                .credentials_from
                .as_deref()
                .map_or_else(|| "its own".to_owned(), |from| format!("from `{from}`"))
        ),
        String::new(),
        table(&["METHOD", "PATH", "OPERATION ID", "SUMMARY"], &operations),
    ]);
    lines.join("\n")
}

/// One command variant in `api commands`.
#[derive(Debug, PartialEq, Eq, Serialize)]
struct CommandRow {
    /// Full invocation with its path flags, e.g. `opencli crm users --id <ID>`.
    command: String,
    method: String,
    path: String,
    summary: Option<String>,
}

async fn commands(
    paths: &Paths,
    name: &ApiName,
    words: &[String],
    format: OutputFormat,
) -> Result<(), CliError> {
    let cached = cache::load_or_build(paths, name)
        .await?
        .ok_or_else(|| StoreFailure::not_found(name))?;
    let rows = matching_commands(name, &cached.commands, words);
    emit(format, &rows, |rows: &Vec<CommandRow>| {
        if rows.is_empty() {
            return format!(
                "No commands of `{name}` match `{}`. Try fewer or shorter words.",
                words.join(" ")
            );
        }
        let lines: Vec<Vec<String>> = rows
            .iter()
            .map(|row| {
                vec![
                    row.command.clone(),
                    format!("{} {}", row.method, row.path),
                    row.summary.clone().unwrap_or_default(),
                ]
            })
            .collect();
        table(&["COMMAND", "CALLS", "SUMMARY"], &lines)
    })
}

/// Commands whose words, method, path, summary or operationId contain every
/// one of `words`, ignoring case. No words lists them all.
fn matching_commands(api: &ApiName, set: &CommandSet, words: &[String]) -> Vec<CommandRow> {
    let wanted: Vec<String> = words
        .iter()
        .flat_map(|word| word.split_whitespace())
        .map(str::to_lowercase)
        .collect();
    set.commands
        .iter()
        .filter(|command| {
            let haystack = search_text(command);
            wanted.iter().all(|word| haystack.contains(word.as_str()))
        })
        .map(|command| CommandRow {
            command: invocation(api, command),
            method: command.method.to_string(),
            path: command.path_template.clone(),
            summary: command.summary.clone(),
        })
        .collect()
}

fn search_text(command: &CommandSpec) -> String {
    [
        command.path.join(" "),
        command.method.to_string(),
        command.path_template.clone(),
        command.summary.clone().unwrap_or_default(),
        command.operation_id.clone().unwrap_or_default(),
    ]
    .join(" ")
    .to_lowercase()
}

fn invocation(api: &ApiName, command: &CommandSpec) -> String {
    let path_flags = command
        .arguments
        .iter()
        .filter(|argument| matches!(argument.target, ArgumentTarget::Path(_)))
        .map(|argument| {
            format!(
                "--{} <{}>",
                argument.name,
                argument.name.to_ascii_uppercase().replace('-', "_")
            )
        });
    std::iter::once(format!("opencli {api}"))
        .chain(command.path.iter().cloned())
        .chain(path_flags)
        .collect::<Vec<_>>()
        .join(" ")
}

async fn versions(store: &Store, name: &ApiName, format: OutputFormat) -> Result<(), CliError> {
    let versions = store.list_versions(name).await?;
    emit(format, &versions, |versions: &Vec<VersionSummary>| {
        let rows: Vec<Vec<String>> = versions
            .iter()
            .map(|version| {
                vec![
                    format!(
                        "{}{}",
                        version.number,
                        if version.current { " *" } else { "" }
                    ),
                    version.version.clone(),
                    version.operation_count.to_string(),
                    version.content_hash.chars().take(12).collect(),
                    timestamp(version.imported_at),
                ]
            })
            .collect();
        table(&["#", "VERSION", "OPERATIONS", "HASH", "IMPORTED"], &rows)
    })
}

#[derive(Serialize)]
struct Removed<'a> {
    removed: &'a str,
}

async fn remove(
    store: &Store,
    paths: &Paths,
    name: &ApiName,
    yes: bool,
    format: OutputFormat,
) -> Result<(), CliError> {
    if !yes {
        if !prompt::is_interactive() {
            return Err(CliError::ConfirmationRequired(name.to_string()));
        }
        if !prompt::confirm(&format!("Remove API `{name}` and all its versions?"))? {
            return Ok(());
        }
    }
    // Read the schemes before the API is gone, so its credentials can go too.
    let schemes: Vec<String> = cache::load_or_build(paths, name)
        .await?
        .map(|cached| cached.commands.security_schemes.into_keys().collect())
        .unwrap_or_default();
    // APIs that used this one's credentials lose the link; their caches follow.
    let dependents = store.apis_using_credentials_of(name).await?;
    store.remove_api(name).await?;
    cache::invalidate(paths, name).await;
    for dependent in dependents.iter().filter_map(|api| ApiName::parse(api).ok()) {
        cache::invalidate(paths, &dependent).await;
    }
    audit::record(paths, &AuditEvent::ApiRemoved { api: name.as_str() }).await;
    remove_credentials(paths, name, schemes).await;
    tracing::info!(api = %name, "api removed");
    emit(
        format,
        &Removed {
            removed: name.as_str(),
        },
        |removed| format!("Removed `{}`.", removed.removed),
    )
}

fn timestamp(value: OffsetDateTime) -> String {
    value.format(&Rfc3339).unwrap_or_else(|_| value.to_string())
}

fn or_none(value: &str) -> &str {
    if value.is_empty() { "none" } else { value }
}

/// Best effort, returning the schemes whose credentials were deleted. Callers
/// have already changed the API, and leftover secrets can still be cleared with
/// `opencli auth logout`.
async fn remove_credentials(paths: &Paths, api: &ApiName, schemes: Vec<String>) -> Vec<String> {
    if schemes.is_empty() {
        return Vec::new();
    }
    let store = match credentials::open(paths) {
        Ok(store) => store,
        Err(error) => {
            tracing::warn!(%api, %error, "could not open the credential store to remove credentials");
            return Vec::new();
        }
    };
    let owner = api.clone();
    let removed = credentials::blocking(move || {
        let mut removed = Vec::new();
        for scheme in schemes {
            if store.delete(&CredentialKey::new(&owner, &scheme))? {
                removed.push(scheme);
            }
        }
        Ok(removed)
    })
    .await;
    removed.unwrap_or_else(|error| {
        tracing::warn!(%api, %error, "could not remove stored credentials");
        Vec::new()
    })
}

#[derive(Serialize)]
struct RefreshResult<'a> {
    api: &'a str,
    /// `unchanged`, `added`, `reactivated`, or `dry-run`.
    outcome: &'static str,
    /// The version that is now current (absent for unchanged and dry runs).
    number: Option<i64>,
    title: &'a str,
    version: &'a str,
    operations: usize,
    breaking: bool,
    diff: &'a CommandDiff,
    credentials_removed: &'a [String],
}

async fn refresh(
    store: &Store,
    paths: &Paths,
    name: &ApiName,
    spec: Option<String>,
    dry_run: bool,
    format: OutputFormat,
) -> Result<(), CliError> {
    let current = store.api_summary(name).await?;
    let raw_source = spec.as_deref().unwrap_or(&current.source);
    let source = SpecSource::parse(raw_source)?;
    let loaded = progress::with_spinner(format!("Loading {raw_source}"), source.load()).await?;
    let new_spec = opencli_spec::parse(&loaded.text, &loaded.display_name)?;
    // Naming conflicts fail the refresh before anything is stored (ADR 0002).
    let new_commands = build_commands(&new_spec)?;
    let old_commands = build_commands(&store.current_spec(name).await?).unwrap_or_else(|error| {
        tracing::warn!(api = %name, %error, "current version no longer builds; diffing against nothing");
        CommandSet {
            servers: Vec::new(),
            security_schemes: std::collections::BTreeMap::new(),
            commands: Vec::new(),
        }
    });
    let diff = diff_commands(&old_commands, &new_commands);

    let (outcome, number, credentials_removed) = if dry_run {
        ("dry-run", None, Vec::new())
    } else {
        let import = SpecImport {
            source: &loaded.location,
            spec: &new_spec,
            document: &loaded.text,
            imported_at: OffsetDateTime::now_utc(),
        };
        let change = store.add_version(name, import).await?;
        cache::write(
            paths,
            &CachedCommands::new(
                name,
                &loaded.location,
                current.base_url.as_deref(),
                &new_spec,
                new_commands,
            )
            .with_credentials_from(current.credentials_from.clone()),
        )
        .await;
        // Credentials for schemes the spec dropped can never be used again.
        let removed = remove_credentials(paths, name, diff.schemes_removed.clone()).await;
        let (outcome, number) = match change {
            VersionChange::Unchanged => ("unchanged", None),
            VersionChange::Added { number } => ("added", Some(number)),
            VersionChange::Reactivated { number } => ("reactivated", Some(number)),
        };
        (outcome, number, removed)
    };
    tracing::info!(api = %name, outcome, "api refreshed");
    if !dry_run {
        let event = AuditEvent::ApiRefreshed {
            api: name.as_str(),
            source: &loaded.location,
            outcome,
        };
        audit::record(paths, &event).await;
    }

    let result = RefreshResult {
        api: name.as_str(),
        outcome,
        number,
        title: &new_spec.title,
        version: &new_spec.version,
        operations: new_spec.operations.len(),
        breaking: diff.is_breaking(),
        diff: &diff,
        credentials_removed: &credentials_removed,
    };
    emit(format, &result, render_refresh)
}

fn render_refresh(result: &RefreshResult<'_>) -> String {
    let described = format!(
        "{} {}, {} operations",
        result.title, result.version, result.operations
    );
    let headline = match (result.outcome, result.number) {
        ("unchanged", _) => format!("`{}` is already up to date.", result.api),
        ("reactivated", Some(number)) => format!(
            "Reactivated version {number} of `{}`: {described}.",
            result.api
        ),
        ("added", Some(number)) => format!(
            "Refreshed `{}` to version {number}: {described}.",
            result.api
        ),
        _ => format!(
            "Dry run, nothing saved. Changes for `{}` ({described}):",
            result.api
        ),
    };
    let mut lines = vec![headline];
    if result.diff.is_empty() {
        if result.outcome == "dry-run" {
            lines.push("No changes.".to_owned());
        }
        return lines.join("\n");
    }
    lines.push(String::new());
    let changes = table(&["CHANGE", "COMMAND", "DETAILS"], &diff_rows(result.diff));
    lines.extend(changes.lines().map(highlight_change));
    let diff = result.diff;
    let schemes: Vec<String> = [
        ("added", &diff.schemes_added),
        ("removed", &diff.schemes_removed),
        ("changed type", &diff.schemes_changed),
    ]
    .into_iter()
    .filter(|(_, names)| !names.is_empty())
    .map(|(label, names)| format!("{label}: {}", names.join(", ")))
    .collect();
    if !schemes.is_empty() {
        lines.push(format!("\nSecurity schemes {}.", schemes.join("; ")));
    }
    if !result.credentials_removed.is_empty() {
        lines.push(format!(
            "Removed stored credentials for: {}.",
            result.credentials_removed.join(", ")
        ));
    }
    for scheme in &diff.schemes_changed {
        lines.push(format!(
            "Stored credentials for `{scheme}` no longer apply; run `opencli auth login {} --scheme {scheme}`.",
            result.api
        ));
    }
    if result.breaking {
        lines.push(
            "\nBreaking: commands or arguments that scripts may rely on were removed or changed."
                .to_owned(),
        );
    }
    lines.join("\n")
}

/// Breaking changes in red, additions in green, other changes in yellow.
fn highlight_change(line: &str) -> String {
    const LABELS: &[(&str, Tone)] = &[
        ("added", Tone::Good),
        ("removed", Tone::Bad),
        ("changed!", Tone::Bad),
        ("renamed", Tone::Notice),
        ("changed", Tone::Notice),
    ];
    LABELS
        .iter()
        .find(|(label, _)| line.starts_with(&format!("{label} ")))
        .map_or_else(
            || line.to_owned(),
            |(label, tone)| highlight_leading(line, label, *tone),
        )
}

fn diff_rows(diff: &CommandDiff) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    rows.extend(diff.added.iter().map(|change| {
        vec![
            "added".to_owned(),
            change.command.clone(),
            change.operation.clone(),
        ]
    }));
    rows.extend(diff.removed.iter().map(|change| {
        vec![
            "removed".to_owned(),
            change.command.clone(),
            change.operation.clone(),
        ]
    }));
    rows.extend(diff.renamed.iter().map(|rename| {
        vec![
            "renamed".to_owned(),
            rename.to.clone(),
            format!("was `{}` ({})", rename.from, rename.operation),
        ]
    }));
    rows.extend(diff.changed.iter().map(|update| {
        let label = if update.breaking {
            "changed!"
        } else {
            "changed"
        };
        vec![
            label.to_owned(),
            update.command.clone(),
            // Variants share a command name, so say which operation changed.
            format!("{}: {}", update.operation, update.details.join("; ")),
        ]
    }));
    rows
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::collections::BTreeMap;

    use opencli_core::{
        ApiSpec, CliHints, HttpMethod, Operation, Parameter, ParameterLocation, SpecFormat,
    };
    use rstest::rstest;
    use serde_json::json;

    use super::*;

    fn operation(method: HttpMethod, path: &str, summary: &str) -> Operation {
        let parameters = path
            .split('{')
            .skip(1)
            .filter_map(|segment| segment.split_once('}').map(|(name, _)| name))
            .map(|name| Parameter {
                name: name.to_owned(),
                location: ParameterLocation::Path,
                required: true,
                description: None,
                schema: Some(json!({"type": "string"})),
                array_delimiter: None,
            })
            .collect();
        Operation {
            operation_id: None,
            method,
            path: path.to_owned(),
            summary: Some(summary.to_owned()),
            description: None,
            deprecated: false,
            parameters,
            request_body: None,
            security: Vec::new(),
            cli: CliHints::default(),
        }
    }

    fn set() -> CommandSet {
        build_commands(&ApiSpec {
            format: SpecFormat::OpenApi30,
            title: "Test".into(),
            version: "1".into(),
            description: None,
            servers: Vec::new(),
            operations: vec![
                operation(HttpMethod::Post, "/api/v1/account/login", "Sign in"),
                operation(HttpMethod::Get, "/api/v1/countries", "List countries"),
                operation(HttpMethod::Get, "/api/v1/countries/{id}", "One country"),
                operation(HttpMethod::Post, "/api/v1/countries", "Add a country"),
            ],
            security_schemes: BTreeMap::new(),
            schemas: BTreeMap::new(),
        })
        .unwrap()
    }

    #[rstest]
    #[case(&[], &["opencli id v1 account login post", "opencli id v1 countries", "opencli id v1 countries --id <ID>", "opencli id v1 countries post"])]
    #[case(&["LOGIN"], &["opencli id v1 account login post"])]
    #[case(&["countries post"], &["opencli id v1 countries post"])]
    #[case(&["country"], &["opencli id v1 countries --id <ID>", "opencli id v1 countries post"])]
    #[case(&["nothing"], &[])]
    fn matching_commands_needs_every_word(#[case] words: &[&str], #[case] expected: &[&str]) {
        let api = ApiName::parse("id").unwrap();
        let words: Vec<String> = words.iter().map(|&word| word.to_owned()).collect();
        let found: Vec<String> = matching_commands(&api, &set(), &words)
            .into_iter()
            .map(|row| row.command)
            .collect();
        assert_eq!(found, expected);
    }
}
