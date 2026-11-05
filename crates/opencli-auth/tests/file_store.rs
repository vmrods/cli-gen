// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use opencli_auth::{
    Credential, CredentialKey, CredentialStore, CredentialStoreError, EncryptedFileStore,
    PassphraseSource,
};
use opencli_core::ApiName;
use secrecy::{ExposeSecret, SecretString};
use serde_json::Value;
use tempfile::TempDir;

fn passphrase(value: &'static str) -> PassphraseSource {
    Box::new(move || Ok(SecretString::from(value)))
}

fn counting_passphrase(value: &'static str, calls: &Arc<AtomicUsize>) -> PassphraseSource {
    let calls = Arc::clone(calls);
    Box::new(move || {
        calls.fetch_add(1, Ordering::SeqCst);
        Ok(SecretString::from(value))
    })
}

fn key(scheme: &str) -> CredentialKey {
    CredentialKey::new(&ApiName::parse("crm").unwrap(), scheme)
}

fn api_key(value: &str) -> Credential {
    Credential::ApiKey {
        key: SecretString::from(value.to_owned()),
    }
}

fn stored_key(store: &EncryptedFileStore, scheme: &str) -> Option<String> {
    store
        .get(&key(scheme))
        .unwrap()
        .map(|credential| match credential {
            Credential::ApiKey { key } => key.expose_secret().to_owned(),
            other => panic!("unexpected credential {other:?}"),
        })
}

struct Fixture {
    _directory: TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("credentials.enc");
        Self {
            _directory: directory,
            path,
        }
    }

    fn store(&self, phrase: &'static str) -> EncryptedFileStore {
        EncryptedFileStore::new(&self.path, passphrase(phrase))
    }
}

#[test]
fn set_then_get_round_trips_across_instances() {
    let fixture = Fixture::new();
    fixture
        .store("correct horse")
        .set(&key("apiKey"), &api_key("k-1"))
        .unwrap();
    fixture
        .store("correct horse")
        .set(&key("other"), &api_key("k-2"))
        .unwrap();

    let reopened = fixture.store("correct horse");
    assert_eq!(stored_key(&reopened, "apiKey").as_deref(), Some("k-1"));
    assert_eq!(stored_key(&reopened, "other").as_deref(), Some("k-2"));
    assert_eq!(stored_key(&reopened, "missing"), None);
}

#[test]
fn file_contains_no_plaintext_secrets() {
    let fixture = Fixture::new();
    fixture
        .store("pw")
        .set(&key("apiKey"), &api_key("super-secret-value"))
        .unwrap();
    let contents = std::fs::read_to_string(&fixture.path).unwrap();
    assert!(!contents.contains("super-secret-value"));
    assert!(
        !contents.contains("crm/apiKey"),
        "account names are encrypted too"
    );
}

#[test]
fn wrong_passphrase_is_rejected() {
    let fixture = Fixture::new();
    fixture
        .store("right")
        .set(&key("apiKey"), &api_key("k"))
        .unwrap();
    let error = fixture.store("wrong").get(&key("apiKey")).unwrap_err();
    assert!(
        matches!(error, CredentialStoreError::WrongPassphrase),
        "{error:?}"
    );
}

#[test]
fn tampered_ciphertext_or_parameters_are_detected() {
    for field in ["ciphertext", "kdf"] {
        let fixture = Fixture::new();
        fixture
            .store("pw")
            .set(&key("apiKey"), &api_key("k"))
            .unwrap();
        let mut envelope: Value =
            serde_json::from_slice(&std::fs::read(&fixture.path).unwrap()).unwrap();
        if field == "ciphertext" {
            let mut ciphertext = envelope["ciphertext"].as_str().unwrap().to_owned();
            let flipped = if ciphertext.starts_with('A') {
                "B"
            } else {
                "A"
            };
            ciphertext.replace_range(0..1, flipped);
            envelope["ciphertext"] = Value::String(ciphertext);
        } else {
            envelope["kdf"]["iterations"] = Value::from(3);
        }
        std::fs::write(&fixture.path, serde_json::to_vec(&envelope).unwrap()).unwrap();
        let error = fixture.store("pw").get(&key("apiKey")).unwrap_err();
        assert!(
            matches!(error, CredentialStoreError::WrongPassphrase),
            "{field}: {error:?}"
        );
    }
}

#[test]
fn corrupt_file_is_reported() {
    let fixture = Fixture::new();
    std::fs::write(&fixture.path, "not json").unwrap();
    assert!(matches!(
        fixture.store("pw").get(&key("x")),
        Err(CredentialStoreError::Corrupt(_))
    ));
}

#[test]
fn passphrase_is_requested_lazily_and_once() {
    let fixture = Fixture::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let store = EncryptedFileStore::new(&fixture.path, counting_passphrase("pw", &calls));

    assert_eq!(stored_key(&store, "apiKey"), None);
    assert_eq!(
        calls.load(Ordering::SeqCst),
        0,
        "reading a missing file needs no passphrase"
    );
    assert!(!store.delete(&key("apiKey")).unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 0);

    store.set(&key("a"), &api_key("1")).unwrap();
    store.set(&key("b"), &api_key("2")).unwrap();
    assert_eq!(stored_key(&store, "a").as_deref(), Some("1"));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the derived key is reused");
}

#[test]
fn missing_passphrase_is_reported() {
    let fixture = Fixture::new();
    let store = EncryptedFileStore::new(
        &fixture.path,
        Box::new(|| Err("set OPENCLI_PASSPHRASE".to_owned())),
    );
    let error = store.set(&key("a"), &api_key("1")).unwrap_err();
    assert!(
        matches!(&error, CredentialStoreError::NoPassphrase(message) if message.contains("OPENCLI_PASSPHRASE"))
    );
}

#[test]
fn delete_removes_only_that_credential() {
    let fixture = Fixture::new();
    let store = fixture.store("pw");
    store.set(&key("a"), &api_key("1")).unwrap();
    store.set(&key("b"), &api_key("2")).unwrap();
    assert!(store.delete(&key("a")).unwrap());
    assert!(!store.delete(&key("a")).unwrap());
    let reopened = fixture.store("pw");
    assert_eq!(stored_key(&reopened, "a"), None);
    assert_eq!(stored_key(&reopened, "b").as_deref(), Some("2"));
}

#[cfg(unix)]
#[test]
fn file_is_readable_only_by_owner() {
    use std::os::unix::fs::PermissionsExt as _;
    let fixture = Fixture::new();
    fixture.store("pw").set(&key("a"), &api_key("1")).unwrap();
    let mode = std::fs::metadata(&fixture.path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}
