//! Append-only audit trail of API calls and changes to APIs and credentials,
//! as JSON lines in the data directory. Never contains secrets: credentials
//! appear only as scheme names, and URLs never carry query strings.

use serde::Serialize;
use time::OffsetDateTime;
use tokio::io::AsyncWriteExt;

use crate::paths::Paths;

const DISABLE_VARIABLE: &str = "OPENCLI_AUDIT";
const DISABLED: &str = "off";
/// Rotated to `audit.log.1` past this size, so the log can't grow without bound.
const ROTATE_BYTES: u64 = 10 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(tag = "event")]
pub enum AuditEvent<'a> {
    #[serde(rename = "request")]
    Request {
        api: &'a str,
        command: String,
        method: String,
        /// Path only; query strings may carry credentials.
        url: &'a str,
        status: Option<u16>,
        outcome: &'static str,
        duration_ms: u128,
        /// Security schemes whose credentials were sent (names, never values).
        schemes: Vec<String>,
    },
    #[serde(rename = "api.add")]
    ApiAdded { api: &'a str, source: &'a str },
    #[serde(rename = "api.refresh")]
    ApiRefreshed {
        api: &'a str,
        source: &'a str,
        outcome: &'static str,
    },
    #[serde(rename = "api.remove")]
    ApiRemoved { api: &'a str },
    #[serde(rename = "api.set-server")]
    ApiServerSet {
        api: &'a str,
        base_url: Option<&'a str>,
    },
    #[serde(rename = "api.set-credentials")]
    ApiCredentialsSet {
        api: &'a str,
        credentials_from: Option<&'a str>,
    },
    #[serde(rename = "auth.login")]
    Login {
        api: &'a str,
        scheme: &'a str,
        store: &'static str,
    },
    #[serde(rename = "auth.logout")]
    Logout { api: &'a str, schemes: &'a [String] },
    #[serde(rename = "plugin.install")]
    PluginInstalled { name: &'a str, source: &'a str },
    #[serde(rename = "plugin.remove")]
    PluginRemoved { name: &'a str },
}

#[derive(Serialize)]
struct Record<'a> {
    #[serde(with = "time::serde::rfc3339")]
    timestamp: OffsetDateTime,
    #[serde(flatten)]
    event: &'a AuditEvent<'a>,
}

/// Best effort: a failed audit write is logged, never fatal to the command.
pub async fn record(paths: &Paths, event: &AuditEvent<'_>) {
    if std::env::var(DISABLE_VARIABLE).is_ok_and(|value| value.eq_ignore_ascii_case(DISABLED)) {
        return;
    }
    if let Err(error) = append(paths, event).await {
        tracing::warn!(%error, "could not write the audit log");
    }
}

async fn append(paths: &Paths, event: &AuditEvent<'_>) -> std::io::Result<()> {
    let record = Record {
        timestamp: OffsetDateTime::now_utc(),
        event,
    };
    let mut line = serde_json::to_vec(&record).map_err(std::io::Error::other)?;
    line.push(b'\n');
    if tokio::fs::metadata(&paths.audit)
        .await
        .is_ok_and(|metadata| metadata.len() > ROTATE_BYTES)
    {
        tokio::fs::rename(&paths.audit, paths.audit.with_extension("log.1")).await?;
    }
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&paths.audit).await?;
    file.write_all(&line).await?;
    file.flush().await
}
