// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use assert_cmd::{Command, cargo::cargo_bin_cmd};
use predicates::str::contains;
use rstest::rstest;
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

#[rstest]
#[case(&["--help"], "Add an API")]
#[case(&["--help"], "Sign in")]
#[case(&["--help"], "Call it")]
#[case(&["--help"], "Keep it up to date")]
#[case(&["api", "--help"], "opencli api set-server crm")]
#[case(&["api", "add", "--help"], "opencli api add crm --spec ./crm.yaml")]
#[case(&["auth", "--help"], "opencli auth login crm")]
#[case(&["plugin", "--help"], "opencli plugin install")]
#[case(&["completion", "--help"], "opencli completion zsh")]
fn help_shows_examples(#[case] args: &[&str], #[case] expected: &str) {
    let home = TempDir::new().unwrap();
    opencli(&home)
        .args(args)
        .assert()
        .success()
        .stdout(contains("Examples:"))
        .stdout(contains(expected));
}

#[test]
fn fails_with_usage_error_when_argument_unknown() {
    let home = TempDir::new().unwrap();
    opencli(&home).arg("--bogus").assert().code(2);
}
