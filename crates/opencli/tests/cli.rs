// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::{Command, cargo::cargo_bin_cmd};
use predicates::str::contains;
use tempfile::TempDir;

/// Every test gets its own OpenCLI home so the real data directory is never touched.
fn opencli(home: &TempDir) -> Command {
    let mut command = cargo_bin_cmd!("opencli");
    command
        .env("OPENCLI_HOME", home.path())
        .env("OPENCLI_CREDENTIAL_STORE", "file")
        .env("OPENCLI_PASSPHRASE", "test-passphrase");
    command
}

#[test]
fn prints_version_when_version_flag_given() {
    let home = TempDir::new().unwrap();
    opencli(&home)
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn prints_usage_when_help_flag_given() {
    let home = TempDir::new().unwrap();
    opencli(&home)
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage: opencli"));
}

#[test]
fn fails_with_usage_error_when_argument_unknown() {
    let home = TempDir::new().unwrap();
    opencli(&home).arg("--bogus").assert().code(2);
}
