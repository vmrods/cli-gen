//! `opencli api …`: registering and inspecting APIs (ADR 0002).

use opencli_core::{ApiName, build_commands};
use opencli_store::{ApiDetails, ApiSummary, SpecImport, Store, VersionSummary};
use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::cache::{self, CachedCommands};
use crate::cli::{ApiCommand, OutputFormat};
use crate::error::CliError;
use crate::output::{emit, table};
use crate::paths::Paths;
use crate::prompt;
use crate::source::SpecSource;

const SPEC_PROMPT: &str = "Path or URL of the OpenAPI document: ";

pub async fn run(command: ApiCommand, format: OutputFormat, paths: &Paths) -> Result<(), CliError> {
    let store = Store::open(&paths.database).await?;
    match command {
        ApiCommand::Add { name, spec } => add(&store, paths, &name, spec, format).await,
        ApiCommand::List => list(&store, format).await,
        ApiCommand::Show { name } => show(&store, &name, format).await,
        ApiCommand::Versions { name } => versions(&store, &name, format).await,
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
    let loaded = SpecSource::parse(&raw_source)?.load().await?;
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
    cache::write(paths, &CachedCommands::new(name, &spec, commands)).await;
    tracing::info!(api = %name, operations = summary.operation_count, "api added");
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
    store.remove_api(name).await?;
    cache::invalidate(paths, name).await;
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
