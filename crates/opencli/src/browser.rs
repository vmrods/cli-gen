//! Best-effort browser launch for the authorization code flow. The URL is
//! always printed too, so a failure here only costs a copy and paste.

use std::process::{Command, Stdio};

pub fn open(url: &str) {
    let mut command = launcher(url);
    let launched = command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    if let Err(error) = launched {
        tracing::debug!(%error, "could not open a browser");
    }
}

// Arguments are passed directly, never through a shell, so a URL from a spec
// can't inject commands.
#[cfg(target_os = "macos")]
fn launcher(url: &str) -> Command {
    let mut command = Command::new("open");
    command.arg(url);
    command
}

#[cfg(target_os = "windows")]
fn launcher(url: &str) -> Command {
    let mut command = Command::new("rundll32");
    command.args(["url.dll,FileProtocolHandler", url]);
    command
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn launcher(url: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(url);
    command
}
