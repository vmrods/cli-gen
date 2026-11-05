use std::path::PathBuf;

use directories::ProjectDirs;

use crate::error::CliError;

/// Overrides the platform data directory (useful for tests, CI and portable installs).
const HOME_VARIABLE: &str = "OPENCLI_HOME";
const DATABASE_FILE: &str = "metadata.db";

/// Returns the metadata database path, creating its directory if needed.
pub async fn database_path() -> Result<PathBuf, CliError> {
    let directory = match std::env::var_os(HOME_VARIABLE) {
        Some(home) if !home.is_empty() => PathBuf::from(home),
        _ => ProjectDirs::from("", "", "opencli")
            .ok_or(CliError::NoDataDirectory)?
            .data_dir()
            .to_path_buf(),
    };
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|source| CliError::CreateDataDirectory {
            path: directory.display().to_string(),
            source,
        })?;
    Ok(directory.join(DATABASE_FILE))
}
