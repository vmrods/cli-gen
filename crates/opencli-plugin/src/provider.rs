//! Credential providers: a program prints `{"value": "<secret>"}` on stdout.
//!
//! The program gets `OPENCLI_API`, `OPENCLI_SCHEME` and `OPENCLI_SCHEME_TYPE`
//! in its environment and no stdin. The value means what
//! `OPENCLI_<API>_<SCHEME>` means: an API key, a token, or `user:password`.

use std::process::Stdio;
use std::time::Duration;

use secrecy::SecretString;
use secrecy::zeroize::Zeroizing;
use serde::Deserialize;
use thiserror::Error;
use tokio::process::Command;

use crate::CredentialProvider;

/// Bounds what a misbehaving program can make us print in an error.
const MAX_MESSAGE_CHARACTERS: usize = 200;

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("could not run credential provider `{exec}`")]
    Spawn {
        exec: String,
        #[source]
        source: std::io::Error,
    },
    #[error("credential provider `{exec}` did not finish in time")]
    Timeout { exec: String },
    #[error("credential provider `{exec}` failed{}: {message}", code.map(|code| format!(" with exit code {code}")).unwrap_or_default())]
    Failed {
        exec: String,
        code: Option<i32>,
        message: String,
    },
    #[error("credential provider `{exec}` must print {{\"value\": \"…\"}} on stdout")]
    InvalidOutput { exec: String },
}

#[derive(Deserialize)]
struct Output {
    value: String,
}

/// # Errors
/// [`ProviderError`] when the program can't run, times out, fails, or prints
/// something other than the expected JSON.
pub async fn fetch_credential(
    provider: &CredentialProvider,
    api: &str,
    scheme: &str,
    scheme_type: &str,
    timeout: Duration,
) -> Result<SecretString, ProviderError> {
    let exec = provider.exec.clone();
    let child = Command::new(&provider.exec)
        .args(&provider.args)
        .env("OPENCLI_API", api)
        .env("OPENCLI_SCHEME", scheme)
        .env("OPENCLI_SCHEME_TYPE", scheme_type)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|source| ProviderError::Spawn {
            exec: exec.clone(),
            source,
        })?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| ProviderError::Timeout { exec: exec.clone() })?
        .map_err(|source| ProviderError::Spawn {
            exec: exec.clone(),
            source,
        })?;
    let stdout = Zeroizing::new(output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let message: String = stderr
            .lines()
            .next()
            .unwrap_or_default()
            .chars()
            .take(MAX_MESSAGE_CHARACTERS)
            .collect();
        return Err(ProviderError::Failed {
            exec,
            code: output.status.code(),
            message,
        });
    }
    let parsed: Output = serde_json::from_slice(&stdout)
        .map_err(|_| ProviderError::InvalidOutput { exec: exec.clone() })?;
    if parsed.value.is_empty() {
        return Err(ProviderError::InvalidOutput { exec });
    }
    Ok(SecretString::from(parsed.value))
}

// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::path::PathBuf;

    use secrecy::ExposeSecret;

    use super::*;

    // Written by `install`, not std::fs::write: on Linux, a process another test
    // thread forks while this one holds the file open for writing inherits that
    // handle, and running the script then fails with "Text file busy"
    // (ETXTBSY). Only the short-lived `install` process ever writes the file.
    fn script(directory: &tempfile::TempDir, body: &str) -> PathBuf {
        let source = directory.path().join("provider.sh");
        std::fs::write(&source, format!("#!/bin/sh\n{body}\n")).unwrap();
        let file = directory.path().join("provider");
        let status = std::process::Command::new("install")
            .args(["-m", "755"])
            .args([&source, &file])
            .status()
            .unwrap();
        assert!(status.success(), "install failed for {}", file.display());
        file
    }

    async fn run(body: &str) -> Result<SecretString, ProviderError> {
        let directory = tempfile::TempDir::new().unwrap();
        let provider = CredentialProvider {
            exec: script(&directory, body).display().to_string(),
            args: vec!["--flag".into()],
        };
        fetch_credential(&provider, "crm", "oauth", "oauth2", Duration::from_secs(5)).await
    }

    #[tokio::test]
    async fn fetch_credential_reads_value_and_passes_context() {
        let secret = run(r#"printf '{"value": "%s-%s-%s-%s"}' "$OPENCLI_API" "$OPENCLI_SCHEME" "$OPENCLI_SCHEME_TYPE" "$1""#).await.unwrap();
        assert_eq!(secret.expose_secret(), "crm-oauth-oauth2---flag");
    }

    #[tokio::test]
    async fn fetch_credential_reports_failures_with_first_stderr_line() {
        let error = run("echo 'not signed in' >&2; echo 'second line' >&2; exit 3")
            .await
            .unwrap_err();
        assert!(
            matches!(&error, ProviderError::Failed { code: Some(3), message, .. } if message == "not signed in"),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn fetch_credential_rejects_unexpected_output() {
        assert!(matches!(
            run("echo token-without-json").await,
            Err(ProviderError::InvalidOutput { .. })
        ));
        assert!(matches!(
            run(r#"echo '{"value": ""}'"#).await,
            Err(ProviderError::InvalidOutput { .. })
        ));
    }

    #[tokio::test]
    async fn fetch_credential_times_out() {
        let directory = tempfile::TempDir::new().unwrap();
        let provider = CredentialProvider {
            exec: script(&directory, "sleep 5").display().to_string(),
            args: vec![],
        };
        let error = fetch_credential(&provider, "a", "b", "c", Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(matches!(error, ProviderError::Timeout { .. }), "{error:?}");
    }

    #[tokio::test]
    async fn fetch_credential_reports_missing_programs() {
        let provider = CredentialProvider {
            exec: "/nonexistent/opencli-provider".into(),
            args: vec![],
        };
        let error = fetch_credential(&provider, "a", "b", "c", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(error, ProviderError::Spawn { .. }), "{error:?}");
    }
}
