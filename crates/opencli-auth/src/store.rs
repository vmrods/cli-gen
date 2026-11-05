//! Where credentials live (ADR 0004): the OS keyring, or an encrypted file.

use opencli_core::ApiName;
use secrecy::zeroize::Zeroizing;
use thiserror::Error;

use crate::Credential;

const KEYRING_SERVICE: &str = "opencli";

/// Credentials are stored per API and security scheme, not per spec version,
/// so they survive spec refreshes.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CredentialKey {
    api: String,
    scheme: String,
}

impl CredentialKey {
    #[must_use]
    pub fn new(api: &ApiName, scheme: &str) -> Self {
        Self {
            api: api.to_string(),
            scheme: scheme.to_owned(),
        }
    }

    #[must_use]
    pub fn account(&self) -> String {
        format!("{}/{}", self.api, self.scheme)
    }
}

/// Defined here, by its only consumer, with a fake in tests.
pub trait CredentialStore: Send + Sync {
    /// Shown in `auth status`.
    fn name(&self) -> &'static str;

    /// # Errors
    /// [`CredentialStoreError`] when the backing store can't be read.
    fn get(&self, key: &CredentialKey) -> Result<Option<Credential>, CredentialStoreError>;

    /// # Errors
    /// [`CredentialStoreError`] when the backing store can't be written.
    fn set(&self, key: &CredentialKey, credential: &Credential)
    -> Result<(), CredentialStoreError>;

    /// Returns whether a credential existed.
    ///
    /// # Errors
    /// [`CredentialStoreError`] when the backing store can't be written.
    fn delete(&self, key: &CredentialKey) -> Result<bool, CredentialStoreError>;
}

#[derive(Debug, Error)]
pub enum CredentialStoreError {
    #[error("the OS keyring failed")]
    Keyring(#[source] keyring::Error),
    #[error("could not access the credentials file {path}")]
    File {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the credentials file is corrupt or was modified: {0}")]
    Corrupt(String),
    #[error("wrong passphrase for the credentials file")]
    WrongPassphrase,
    #[error("no passphrase for the credentials file: {0}")]
    NoPassphrase(String),
    #[error("could not encode a credential")]
    Encode(#[from] serde_json::Error),
}

/// Windows Credential Manager, macOS Keychain or Linux Secret Service.
#[derive(Debug, Default, Clone, Copy)]
pub struct KeyringStore;

impl KeyringStore {
    /// False on machines without a usable keyring (e.g. headless Linux).
    #[must_use]
    pub fn is_available() -> bool {
        keyring::Entry::store_status().is_ok()
    }

    fn entry(key: &CredentialKey) -> Result<keyring::Entry, CredentialStoreError> {
        keyring::Entry::new(KEYRING_SERVICE, &key.account()).map_err(CredentialStoreError::Keyring)
    }
}

impl CredentialStore for KeyringStore {
    fn name(&self) -> &'static str {
        "keyring"
    }

    fn get(&self, key: &CredentialKey) -> Result<Option<Credential>, CredentialStoreError> {
        match Self::entry(key)?.get_password() {
            Ok(json) => {
                let json = Zeroizing::new(json);
                serde_json::from_str(&json)
                    .map(Some)
                    .map_err(|error| CredentialStoreError::Corrupt(error.to_string()))
            }
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(CredentialStoreError::Keyring(error)),
        }
    }

    fn set(
        &self,
        key: &CredentialKey,
        credential: &Credential,
    ) -> Result<(), CredentialStoreError> {
        let json = Zeroizing::new(serde_json::to_string(credential)?);
        Self::entry(key)?
            .set_password(&json)
            .map_err(CredentialStoreError::Keyring)
    }

    fn delete(&self, key: &CredentialKey) -> Result<bool, CredentialStoreError> {
        match Self::entry(key)?.delete_credential() {
            Ok(()) => Ok(true),
            Err(keyring::Error::NoEntry) => Ok(false),
            Err(error) => Err(CredentialStoreError::Keyring(error)),
        }
    }
}
