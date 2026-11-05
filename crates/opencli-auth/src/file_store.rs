//! Passphrase-encrypted credentials file, used when no OS keyring is
//! available (ADR 0004). The whole credential map is encrypted with
//! ChaCha20-Poly1305 under a key derived from the passphrase with Argon2id.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use secrecy::zeroize::Zeroizing;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::Credential;
use crate::store::{CredentialKey, CredentialStore, CredentialStoreError};

const FORMAT: u32 = 1;
/// Binds the ciphertext to this file format and its KDF parameters.
const ASSOCIATED_DATA_PREFIX: &str = "opencli-credentials";
const KEY_BYTES: usize = 32;
const SALT_BYTES: usize = 16;
const NONCE_BYTES: usize = 12;
/// OWASP's recommended Argon2id minimum: 19 MiB, 2 iterations, 1 lane.
const ARGON2_MEMORY_KIB: u32 = 19 * 1024;
const ARGON2_ITERATIONS: u32 = 2;
const ARGON2_PARALLELISM: u32 = 1;

/// Supplies the passphrase on first use (environment variable or prompt).
pub type PassphraseSource = Box<dyn Fn() -> Result<SecretString, String> + Send + Sync>;

pub struct EncryptedFileStore {
    path: PathBuf,
    passphrase: PassphraseSource,
    key: OnceLock<DerivedKey>,
}

struct DerivedKey {
    salt: Vec<u8>,
    key: Zeroizing<[u8; KEY_BYTES]>,
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    format: u32,
    kdf: KdfParameters,
    nonce: String,
    ciphertext: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct KdfParameters {
    algorithm: String,
    salt: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
}

type CredentialMap = BTreeMap<String, Credential>;

impl std::fmt::Debug for EncryptedFileStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EncryptedFileStore")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl EncryptedFileStore {
    #[must_use]
    pub fn new(path: impl Into<PathBuf>, passphrase: PassphraseSource) -> Self {
        Self {
            path: path.into(),
            passphrase,
            key: OnceLock::new(),
        }
    }

    fn read_all(&self) -> Result<CredentialMap, CredentialStoreError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(CredentialMap::new());
            }
            Err(source) => return Err(self.file_error(source)),
        };
        let envelope: Envelope = serde_json::from_slice(&bytes)
            .map_err(|error| CredentialStoreError::Corrupt(error.to_string()))?;
        if envelope.format != FORMAT {
            return Err(CredentialStoreError::Corrupt(format!(
                "unsupported format {}",
                envelope.format
            )));
        }
        let salt = decode(&envelope.kdf.salt)?;
        let key = self.key_for(&salt, &envelope.kdf)?;
        let nonce = Nonce::try_from(decode(&envelope.nonce)?.as_slice())
            .map_err(|_| CredentialStoreError::Corrupt("bad nonce length".into()))?;
        let plaintext = Zeroizing::new(
            cipher(&key.key)
                .decrypt(
                    &nonce,
                    Payload {
                        msg: &decode(&envelope.ciphertext)?,
                        aad: associated_data(&envelope.kdf).as_bytes(),
                    },
                )
                // Authentication failure: wrong passphrase, or a tampered file.
                .map_err(|_| CredentialStoreError::WrongPassphrase)?,
        );
        serde_json::from_slice(&plaintext)
            .map_err(|error| CredentialStoreError::Corrupt(error.to_string()))
    }

    fn write_all(&self, credentials: &CredentialMap) -> Result<(), CredentialStoreError> {
        let key = if let Some(key) = self.key.get() {
            key
        } else {
            let salt = random_bytes::<SALT_BYTES>()?.to_vec();
            self.key_for(&salt, &default_parameters(&salt))?
        };
        let kdf = default_parameters(&key.salt);
        let nonce = random_bytes::<NONCE_BYTES>()?;
        let plaintext = Zeroizing::new(serde_json::to_vec(credentials)?);
        let ciphertext = cipher(&key.key)
            .encrypt(
                &Nonce::from(nonce),
                Payload {
                    msg: &plaintext,
                    aad: associated_data(&kdf).as_bytes(),
                },
            )
            .map_err(|_| CredentialStoreError::Corrupt("encryption failed".into()))?;
        let envelope = Envelope {
            format: FORMAT,
            kdf,
            nonce: BASE64.encode(nonce),
            ciphertext: BASE64.encode(ciphertext),
        };
        let bytes = serde_json::to_vec_pretty(&envelope)?;
        write_private(&self.path, &bytes).map_err(|source| self.file_error(source))
    }

    /// Derives the key once per process; later calls must use the same salt.
    fn key_for(
        &self,
        salt: &[u8],
        kdf: &KdfParameters,
    ) -> Result<&DerivedKey, CredentialStoreError> {
        if let Some(key) = self.key.get() {
            if key.salt == salt {
                return Ok(key);
            }
            return Err(CredentialStoreError::Corrupt(
                "salt changed while in use".into(),
            ));
        }
        let passphrase = (self.passphrase)().map_err(CredentialStoreError::NoPassphrase)?;
        let params = Params::new(
            kdf.memory_kib,
            kdf.iterations,
            kdf.parallelism,
            Some(KEY_BYTES),
        )
        .map_err(|error| CredentialStoreError::Corrupt(error.to_string()))?;
        let mut key = Zeroizing::new([0_u8; KEY_BYTES]);
        Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
            .hash_password_into(passphrase.expose_secret().as_bytes(), salt, key.as_mut())
            .map_err(|error| CredentialStoreError::Corrupt(error.to_string()))?;
        Ok(self.key.get_or_init(|| DerivedKey {
            salt: salt.to_vec(),
            key,
        }))
    }

    fn file_error(&self, source: std::io::Error) -> CredentialStoreError {
        CredentialStoreError::File {
            path: self.path.display().to_string(),
            source,
        }
    }
}

impl CredentialStore for EncryptedFileStore {
    fn name(&self) -> &'static str {
        "encrypted file"
    }

    fn get(&self, key: &CredentialKey) -> Result<Option<Credential>, CredentialStoreError> {
        Ok(self.read_all()?.remove(&key.account()))
    }

    fn set(
        &self,
        key: &CredentialKey,
        credential: &Credential,
    ) -> Result<(), CredentialStoreError> {
        let mut credentials = self.read_all()?;
        credentials.insert(key.account(), credential.clone());
        self.write_all(&credentials)
    }

    fn delete(&self, key: &CredentialKey) -> Result<bool, CredentialStoreError> {
        if !self.path.exists() {
            return Ok(false);
        }
        let mut credentials = self.read_all()?;
        let existed = credentials.remove(&key.account()).is_some();
        if existed {
            self.write_all(&credentials)?;
        }
        Ok(existed)
    }
}

fn default_parameters(salt: &[u8]) -> KdfParameters {
    KdfParameters {
        algorithm: "argon2id".into(),
        salt: BASE64.encode(salt),
        memory_kib: ARGON2_MEMORY_KIB,
        iterations: ARGON2_ITERATIONS,
        parallelism: ARGON2_PARALLELISM,
    }
}

fn associated_data(kdf: &KdfParameters) -> String {
    format!(
        "{ASSOCIATED_DATA_PREFIX}:{FORMAT}:{}:{}:{}:{}:{}",
        kdf.algorithm, kdf.salt, kdf.memory_kib, kdf.iterations, kdf.parallelism
    )
}

fn cipher(key: &[u8; KEY_BYTES]) -> ChaCha20Poly1305 {
    ChaCha20Poly1305::new(key.into())
}

fn decode(value: &str) -> Result<Vec<u8>, CredentialStoreError> {
    BASE64
        .decode(value)
        .map_err(|error| CredentialStoreError::Corrupt(error.to_string()))
}

fn random_bytes<const N: usize>() -> Result<[u8; N], CredentialStoreError> {
    let mut bytes = [0_u8; N];
    getrandom::fill(&mut bytes).map_err(|error| {
        CredentialStoreError::Corrupt(format!("no secure random source: {error}"))
    })?;
    Ok(bytes)
}

/// Writes atomically, readable only by the owner. On Windows the file inherits
/// the user profile directory's owner-only ACL.
fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt as _;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        std::io::Write::write_all(&mut file, bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temporary, path)
}
