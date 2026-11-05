//! `opencli api …`: registering and inspecting APIs (ADR 0002).

use opencli_auth::CredentialKey;
use opencli_core::{ApiName, CommandDiff, CommandSet, build_commands, diff_commands};
use opencli_store::{ApiDetails, ApiSummary, SpecImport, Store, VersionChange, VersionSummary};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::audit::{self, AuditEvent};
use crate::cache::{self, CachedCommands};
use crate::cli::{ApiCommand, OutputFormat};
use crate::credentials;
use crate::error::CliError;
use crate::output::{Tone, emit, highlight_leading, table};
use crate::paths::Paths;
use crate::source::SpecSource;
use crate::{progress, prompt};

const SPEC_PROMPT: &str = "Path or URL of the OpenAPI document: ";

pub async fn run(command: ApiCommand, format: OutputFormat, paths: &Paths) -> Result<(), CliError> {
    let store = Store::open(&paths.database).await?;
    match command {
        ApiCommand::Add { name, spec } => add(&store, paths, &name, spec, format).await,
        ApiCommand::List => list(&store, format).await,
        ApiCommand::Show { name } => show(&store, &name, format).await,
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
    let summary = store
        .add_api(
            name,
            SpecImport {
                source: &loaded.location,
                spec: &spec,
                document: &loaded.text,
                imported_at: OffsetDateTime::now_utc(),
            },
        )
        .await?;
    cache::write(
        paths,
        &CachedCommands::new(name, &loaded.location, &spec, commands),
    )
    .await;
    tracing::info!(api = %name, operations = summary.operation_count, "api added");
    let event = AuditEvent::ApiAdded {
        api: name.as_str(),
        source: &loaded.location,
    };
    audit::record(paths, &event).await;
    emit(format, &summary, |summary| {
        format!(
            "Added `{}`: {} {}, {} operations.",
            summary.name, summary.title, summary.version, summary.operation_count
        )
    })
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
        format!("Servers:   {}", or_none(&details.servers.join(", "))),
        format!("Security:  {}", or_none(&schemes.join(", "))),
        String::new(),
        table(&["METHOD", "PATH", "OPERATION ID", "SUMMARY"], &operations),
    ]);
    lines.join("\n")
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
    store.remove_api(name).await?;
    cache::invalidate(paths, name).await;
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
            &CachedCommands::new(name, &loaded.location, &new_spec, new_commands),
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
            update.details.join("; "),
        ]
    }));
    rows
}
