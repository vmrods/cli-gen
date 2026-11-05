use assert_cmd::{Command, cargo::cargo_bin_cmd};
use predicates::str::contains;

fn opencli() -> Command {
    cargo_bin_cmd!("opencli")
}

#[test]
fn prints_version_when_version_flag_given() {
    opencli()
        .arg("--version")
        .assert()
        .success()
        .stdout(contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn prints_usage_when_help_flag_given() {
    opencli()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Usage: opencli"));
}

#[test]
fn fails_with_usage_error_when_argument_unknown() {
    opencli().arg("--bogus").assert().code(2);
}
