//! Per-API command-tree cache (ADR 0003): lets `opencli <api> …` dispatch
//! without opening SQLite. Written on `api add`, removed on `api remove`,
//! rebuilt from the store whenever it is missing or from another version.

use std::path::PathBuf;

use opencli_core::{ApiName, ApiSpec, CommandSet, build_commands};
use opencli_store::{Store, StoreError};
use serde::{Deserialize, Serialize};

use crate::error::CliError;
use crate::paths::Paths;

/// Bump when `CachedCommands`, `CommandSet` or the naming rules change.
const CACHE_FORMAT: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedCommands {
    pub cache_format: u32,
    pub opencli_version: String,
    pub api: String,
    /// Where the spec came from; relative server URLs resolve against it.
    pub source: String,
    pub title: String,
    pub version: String,
    pub commands: CommandSet,
}

impl CachedCommands {
    pub fn new(api: &ApiName, source: &str, spec: &ApiSpec, commands: CommandSet) -> Self {
        Self {
            cache_format: CACHE_FORMAT,
            opencli_version: env!("CARGO_PKG_VERSION").to_owned(),
            api: api.to_string(),
            source: source.to_owned(),
            title: spec.title.clone(),
            version: spec.version.clone(),
            commands,
        }
    }

    fn is_current_for(&self, api: &ApiName) -> bool {
        self.cache_format == CACHE_FORMAT
            && self.opencli_version == env!("CARGO_PKG_VERSION")
            && self.api == api.as_str()
    }
}

fn file(paths: &Paths, api: &ApiName) -> PathBuf {
    paths.cache.join(format!("{api}.json"))
}

/// Returns the API's commands, or `None` when no such API is registered.
pub async fn load_or_build(
    paths: &Paths,
    api: &ApiName,
) -> Result<Option<CachedCommands>, CliError> {
    if let Some(cached) = read(paths, api).await {
        return Ok(Some(cached));
    }
    let store = Store::open(&paths.database).await?;
    let (summary, spec) = match store.api_summary(api).await {
        Ok(summary) => (summary, store.current_spec(api).await?),
        Err(StoreError::NotFound(_)) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let cached = CachedCommands::new(api, &summary.source, &spec, build_commands(&spec)?);
    write(paths, &cached).await;
    Ok(Some(cached))
}

async fn read(paths: &Paths, api: &ApiName) -> Option<CachedCommands> {
    let bytes = tokio::fs::read(file(paths, api)).await.ok()?;
    let cached: CachedCommands = serde_json::from_slice(&bytes)
        .inspect_err(|error| tracing::debug!(%api, %error, "ignoring unreadable command cache"))
        .ok()?;
    cached.is_current_for(api).then_some(cached)
}

/// Best effort: a failed write only costs a rebuild next time.
pub async fn write(paths: &Paths, cached: &CachedCommands) {
    let Ok(api) = ApiName::parse(&cached.api) else {
        return;
    };
    let target = file(paths, &api);
    let temporary = target.with_extension("json.tmp");
    let result = async {
        tokio::fs::create_dir_all(&paths.cache).await?;
        let bytes = serde_json::to_vec(cached).map_err(std::io::Error::other)?;
        tokio::fs::write(&temporary, bytes).await?;
        tokio::fs::rename(&temporary, &target).await
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(%api, %error, "could not write command cache");
    }
}

pub async fn invalidate(paths: &Paths, api: &ApiName) {
    match tokio::fs::remove_file(file(paths, api)).await {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => tracing::warn!(%api, %error, "could not remove command cache"),
    }
}
