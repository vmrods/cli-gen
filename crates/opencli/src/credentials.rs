//! Opens the credential store (ADR 0004) and runs its blocking calls off the
//! async runtime.

use std::io;
use std::path::Path;
use std::sync::Arc;

use opencli_auth::{AuthError, CredentialStore, EncryptedFileStore, KeyringStore};
use secrecy::{ExposeSecret, SecretString};

use crate::error::CliError;
use crate::paths::Paths;
use crate::prompt;

const STORE_VARIABLE: &str = "OPENCLI_CREDENTIAL_STORE";
const PASSPHRASE_VARIABLE: &str = "OPENCLI_PASSPHRASE";
const KEYRING: &str = "keyring";
const FILE: &str = "file";

pub type SharedStore = Arc<dyn CredentialStore>;

/// The keyring when one is available, otherwise the encrypted file;
/// `OPENCLI_CREDENTIAL_STORE=keyring|file` forces a choice.
pub fn open(paths: &Paths) -> Result<SharedStore, CliError> {
    let requested = std::env::var(STORE_VARIABLE)
        .ok()
        .filter(|value| !value.is_empty());
    match requested.as_deref() {
        Some(KEYRING) | None if KeyringStore::is_available() => Ok(Arc::new(KeyringStore)),
        Some(KEYRING) => Err(CliError::KeyringUnavailable),
        Some(FILE) => Ok(file_store(paths)),
        Some(other) => Err(CliError::InvalidCredentialStore(other.to_owned())),
        None => {
            tracing::info!(path = %paths.credentials.display(), "no OS keyring available; using the encrypted credentials file");
            Ok(file_store(paths))
        }
    }
}

fn file_store(paths: &Paths) -> SharedStore {
    let path = paths.credentials.clone();
    let exists_at = path.clone();
    Arc::new(EncryptedFileStore::new(
        path,
        Box::new(move || passphrase(&exists_at)),
    ))
}

fn passphrase(path: &Path) -> Result<SecretString, String> {
    if let Some(value) = std::env::var(PASSPHRASE_VARIABLE)
        .ok()
        .filter(|value| !value.is_empty())
    {
        return Ok(SecretString::from(value));
    }
    if !prompt::is_interactive() {
        return Err(format!(
            "set {PASSPHRASE_VARIABLE}, or run in an interactive terminal"
        ));
    }
    let read = |question: &str| {
        rpassword::prompt_password(question)
            .map(SecretString::from)
            .map_err(|error: io::Error| error.to_string())
    };
    if path.exists() {
        return read("Passphrase for the OpenCLI credentials file: ");
    }
    let first = read("Choose a passphrase for the new OpenCLI credentials file: ")?;
    let second = read("Repeat the passphrase: ")?;
    if first.expose_secret() != second.expose_secret() {
        return Err("the passphrases did not match".to_owned());
    }
    if first.expose_secret().is_empty() {
        return Err("the passphrase must not be empty".to_owned());
    }
    Ok(first)
}

/// Keyring and Argon2 calls block, so they run on a blocking thread.
pub async fn blocking<T, F>(work: F) -> Result<T, CliError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AuthError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|error| CliError::Internal(error.to_string()))?
        .map_err(CliError::from)
}
