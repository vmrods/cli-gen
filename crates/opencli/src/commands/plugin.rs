//! `opencli plugin …`: installing, listing and removing plugins (ADR 0005).

use std::io::Write as _;
use std::path::{Path, PathBuf};

use opencli_core::ApiName;
use opencli_plugin::{Plugin, list_executables, parse_manifest};
use opencli_store::Store;
use serde::Serialize;

use crate::audit::{self, AuditEvent};
use crate::cli::{OutputFormat, PluginCommand};
use crate::error::CliError;
use crate::output::{emit, table};
use crate::paths::Paths;
use crate::plugins::{self, MANIFEST_EXTENSION};
use crate::source::SpecSource;
use crate::{progress, prompt};

pub async fn run(
    command: PluginCommand,
    format: OutputFormat,
    paths: &Paths,
) -> Result<(), CliError> {
    match command {
        PluginCommand::Install { source, yes } => install(&source, yes, format, paths).await,
        PluginCommand::List => list(format, paths).await,
        PluginCommand::Remove { name, yes } => remove(&name, yes, format, paths).await,
    }
}

fn manifest_path(paths: &Paths, name: &ApiName) -> PathBuf {
    paths.plugins.join(format!("{name}.{MANIFEST_EXTENSION}"))
}

#[derive(Serialize)]
struct InstallResult<'a> {
    plugin: &'a str,
    version: &'a str,
    api: &'a str,
}

async fn install(
    source: &str,
    yes: bool,
    format: OutputFormat,
    paths: &Paths,
) -> Result<(), CliError> {
    let location = SpecSource::parse(source)?;
    let loaded = progress::with_spinner(format!("Loading {source}"), location.load()).await?;
    let plugin = parse_manifest(&loaded.text)?;
    let target = manifest_path(paths, &plugin.name);
    let replacing = target.exists();

    // Everything the plugin can do is shown before it is trusted.
    writeln!(std::io::stderr(), "{}", describe(&plugin, &loaded.location))
        .map_err(CliError::Terminal)?;
    if !is_registered(paths, &plugin.api).await {
        writeln!(
            std::io::stderr(),
            "Note: no API named `{}` is registered yet; the plugin applies once it is.",
            plugin.api
        )
        .map_err(CliError::Terminal)?;
    }
    let action = if replacing { "replacing" } else { "installing" };
    if !yes {
        if !prompt::is_interactive() {
            return Err(CliError::PluginConfirmationRequired {
                action,
                name: plugin.name.to_string(),
            });
        }
        let question = if replacing {
            "Replace the installed version?"
        } else {
            "Install it?"
        };
        if !prompt::confirm(question)? {
            return Ok(());
        }
    }

    write_manifest(&target, &loaded.text)
        .await
        .map_err(|source| CliError::PluginWrite {
            name: plugin.name.to_string(),
            source,
        })?;
    let event = AuditEvent::PluginInstalled {
        name: plugin.name.as_str(),
        source: &loaded.location,
    };
    audit::record(paths, &event).await;
    let result = InstallResult {
        plugin: plugin.name.as_str(),
        version: &plugin.version,
        api: plugin.api.as_str(),
    };
    emit(format, &result, |result| {
        format!(
            "Installed plugin `{}` {} for `{}`.",
            result.plugin, result.version, result.api
        )
    })
}

fn describe(plugin: &Plugin, origin: &str) -> String {
    let mut lines = vec![format!(
        "Plugin `{}` {} for API `{}`, from {origin}",
        plugin.name, plugin.version, plugin.api
    )];
    if let Some(description) = &plugin.description {
        lines.push(format!("  {description}"));
    }
    for (name, alias) in &plugin.aliases {
        let args = if alias.args.is_empty() {
            String::new()
        } else {
            format!(" {}", alias.args.join(" "))
        };
        lines.push(format!("  Alias:   {name} → {}{args}", alias.command));
    }
    for command in plugin.formats.keys() {
        lines.push(format!("  Format:  {command}"));
    }
    for (scheme, provider) in &plugin.credentials {
        let args = if provider.args.is_empty() {
            String::new()
        } else {
            format!(" {}", provider.args.join(" "))
        };
        lines.push(format!(
            "  Runs:    {}{args}  (to supply credentials for `{scheme}`)",
            provider.exec
        ));
    }
    if plugin.credentials.is_empty() {
        lines.push("  Runs no programs.".to_owned());
    }
    lines.join("\n")
}

async fn is_registered(paths: &Paths, api: &ApiName) -> bool {
    match Store::open(&paths.database).await {
        Ok(store) => store.api_summary(api).await.is_ok(),
        Err(_) => false,
    }
}

/// Atomic, and readable only by the owner: a manifest can name programs to run.
async fn write_manifest(target: &Path, text: &str) -> std::io::Result<()> {
    if let Some(directory) = target.parent() {
        tokio::fs::create_dir_all(directory).await?;
    }
    let temporary = target.with_extension("tmp");
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&temporary).await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, text.as_bytes()).await?;
    file.sync_all().await?;
    tokio::fs::rename(&temporary, target).await
}

#[derive(Serialize)]
struct Listing {
    manifests: Vec<ManifestSummary>,
    executables: Vec<ExecutableSummary>,
}

#[derive(Serialize)]
struct ManifestSummary {
    name: String,
    version: String,
    api: String,
    aliases: Vec<String>,
    formats: Vec<String>,
    providers: Vec<String>,
}

#[derive(Serialize)]
struct ExecutableSummary {
    name: String,
    path: String,
}

async fn list(format: OutputFormat, paths: &Paths) -> Result<(), CliError> {
    let manifests = plugins::load_all(paths)
        .await
        .into_iter()
        .map(|plugin| ManifestSummary {
            name: plugin.name.to_string(),
            version: plugin.version,
            api: plugin.api.to_string(),
            aliases: plugin.aliases.into_keys().collect(),
            formats: plugin.formats.into_keys().collect(),
            providers: plugin.credentials.into_keys().collect(),
        })
        .collect();
    let executables = list_executables(std::env::var_os("PATH").as_deref())
        .into_iter()
        .map(|(name, path)| ExecutableSummary {
            name,
            path: path.display().to_string(),
        })
        .collect();
    emit(
        format,
        &Listing {
            manifests,
            executables,
        },
        render_listing,
    )
}

fn render_listing(listing: &Listing) -> String {
    if listing.manifests.is_empty() && listing.executables.is_empty() {
        return "No plugins installed. Install one with `opencli plugin install <plugin.toml>`."
            .to_owned();
    }
    let mut sections = Vec::new();
    if !listing.manifests.is_empty() {
        let rows: Vec<Vec<String>> = listing
            .manifests
            .iter()
            .map(|manifest| {
                vec![
                    manifest.name.clone(),
                    manifest.version.clone(),
                    manifest.api.clone(),
                    manifest.aliases.join(", "),
                    manifest.formats.join(", "),
                    manifest.providers.join(", "),
                ]
            })
            .collect();
        sections.push(table(
            &[
                "NAME",
                "VERSION",
                "API",
                "ALIASES",
                "FORMATS",
                "CREDENTIALS",
            ],
            &rows,
        ));
    }
    if !listing.executables.is_empty() {
        let rows: Vec<Vec<String>> = listing
            .executables
            .iter()
            .map(|executable| {
                vec![
                    format!("opencli {}", executable.name),
                    executable.path.clone(),
                ]
            })
            .collect();
        sections.push(table(&["COMMAND", "EXECUTABLE"], &rows));
    }
    sections.join("\n\n")
}

#[derive(Serialize)]
struct Removed<'a> {
    removed: &'a str,
}

async fn remove(
    name: &ApiName,
    yes: bool,
    format: OutputFormat,
    paths: &Paths,
) -> Result<(), CliError> {
    let target = manifest_path(paths, name);
    if !target.exists() {
        return Err(CliError::PluginNotFound(name.to_string()));
    }
    if !yes {
        if !prompt::is_interactive() {
            return Err(CliError::PluginConfirmationRequired {
                action: "removing",
                name: name.to_string(),
            });
        }
        if !prompt::confirm(&format!("Remove plugin `{name}`?"))? {
            return Ok(());
        }
    }
    tokio::fs::remove_file(&target)
        .await
        .map_err(|source| CliError::PluginWrite {
            name: name.to_string(),
            source,
        })?;
    audit::record(
        paths,
        &AuditEvent::PluginRemoved {
            name: name.as_str(),
        },
    )
    .await;
    emit(
        format,
        &Removed {
            removed: name.as_str(),
        },
        |removed| format!("Removed plugin `{}`.", removed.removed),
    )
}
