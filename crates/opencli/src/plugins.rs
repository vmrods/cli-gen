//! Loading installed manifest plugins (ADR 0005).

use opencli_core::ApiName;
use opencli_plugin::{Plugin, parse_manifest};

use crate::paths::Paths;

pub const MANIFEST_EXTENSION: &str = "toml";

/// Every installed manifest; unreadable or invalid ones are skipped with a warning.
pub async fn load_all(paths: &Paths) -> Vec<Plugin> {
    let Ok(mut entries) = tokio::fs::read_dir(&paths.plugins).await else {
        return Vec::new();
    };
    let mut plugins = Vec::new();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|extension| extension.to_str()) != Some(MANIFEST_EXTENSION) {
            continue;
        }
        let loaded = tokio::fs::read_to_string(&path)
            .await
            .map_err(|error| error.to_string())
            .and_then(|text| parse_manifest(&text).map_err(|error| error.to_string()));
        match loaded {
            Ok(plugin) => plugins.push(plugin),
            Err(error) => tracing::warn!(path = %path.display(), %error, "skipping invalid plugin"),
        }
    }
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins
}

/// The installed plugins that extend `api`.
pub async fn for_api(paths: &Paths, api: &ApiName) -> Vec<Plugin> {
    load_all(paths)
        .await
        .into_iter()
        .filter(|plugin| &plugin.api == api)
        .collect()
}
