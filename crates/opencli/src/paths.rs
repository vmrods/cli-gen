use std::path::PathBuf;

use directories::ProjectDirs;

use crate::error::CliError;

/// Overrides the platform data directory (useful for tests, CI and portable installs).
const HOME_VARIABLE: &str = "OPENCLI_HOME";
const DATABASE_FILE: &str = "metadata.db";
const CACHE_DIRECTORY: &str = "cache";
const CREDENTIALS_FILE: &str = "credentials.enc";

/// Where OpenCLI keeps its files.
#[derive(Debug, Clone)]
pub struct Paths {
    pub database: PathBuf,
    /// Derived command trees, safe to delete (ADR 0003).
    pub cache: PathBuf,
    /// Encrypted credentials, used only without an OS keyring (ADR 0004).
    pub credentials: PathBuf,
}

impl Paths {
    /// Resolves the data directory and creates it if needed.
    pub async fn resolve() -> Result<Self, CliError> {
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
        Ok(Self {
            database: directory.join(DATABASE_FILE),
            cache: directory.join(CACHE_DIRECTORY),
            credentials: directory.join(CREDENTIALS_FILE),
        })
    }
}
