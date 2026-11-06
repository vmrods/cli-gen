//! Executable plugins: `opencli-<name>` anywhere on `PATH`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use opencli_core::kebab_case;

pub const EXECUTABLE_PREFIX: &str = "opencli-";

/// The first `opencli-<name>` executable on `path`. Names must be kebab-case,
/// so a crafted name can't point outside the `PATH` directories.
#[must_use]
pub fn find_executable(name: &str, path: Option<&OsStr>) -> Option<PathBuf> {
    if name.is_empty() || kebab_case(name) != name {
        return None;
    }
    let file = format!("{EXECUTABLE_PREFIX}{name}");
    std::env::split_paths(path?)
        .flat_map(|directory| candidates(&directory, &file))
        .find(|candidate| is_executable(candidate))
}

/// Every `opencli-*` executable on `path`, first occurrence of each name wins.
#[must_use]
pub fn list_executables(path: Option<&OsStr>) -> Vec<(String, PathBuf)> {
    let Some(path) = path else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = Vec::new();
    for directory in std::env::split_paths(path) {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        let mut here: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|candidate| is_executable(candidate))
            .filter_map(|candidate| {
                let stem = candidate
                    .file_stem()?
                    .to_str()?
                    .strip_prefix(EXECUTABLE_PREFIX)?
                    .to_owned();
                (kebab_case(&stem) == stem).then_some((stem, candidate))
            })
            .collect();
        here.sort();
        for (name, candidate) in here {
            if !found.iter().any(|(existing, _)| *existing == name) {
                found.push((name, candidate));
            }
        }
    }
    found
}

#[cfg(windows)]
fn candidates(directory: &Path, file: &str) -> Vec<PathBuf> {
    ["exe", "cmd", "bat"]
        .iter()
        .map(|extension| directory.join(format!("{file}.{extension}")))
        .collect()
}

#[cfg(not(windows))]
fn candidates(directory: &Path, file: &str) -> Vec<PathBuf> {
    vec![directory.join(file)]
}

#[cfg(unix)]
fn is_executable(candidate: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(candidate)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(candidate: &Path) -> bool {
    let runnable = candidate
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            ["exe", "cmd", "bat"].contains(&extension.to_ascii_lowercase().as_str())
        });
    runnable && candidate.is_file()
}

// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    fn write(directory: &Path, name: &str, executable: bool) {
        let file = directory.join(name);
        std::fs::write(&file, "#!/bin/sh\n").unwrap();
        let mode = if executable { 0o755 } else { 0o644 };
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn find_and_list_executables_on_path() {
        let first = tempfile::TempDir::new().unwrap();
        let second = tempfile::TempDir::new().unwrap();
        write(first.path(), "opencli-hello", true);
        write(first.path(), "opencli-plain-file", false);
        write(second.path(), "opencli-hello", true);
        write(second.path(), "opencli-world", true);
        write(second.path(), "unrelated", true);
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();

        assert_eq!(
            find_executable("hello", Some(&path)),
            Some(first.path().join("opencli-hello"))
        );
        assert_eq!(find_executable("plain-file", Some(&path)), None);
        assert_eq!(find_executable("../hello", Some(&path)), None);
        let names: Vec<_> = list_executables(Some(&path))
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["hello", "world"]);
    }
}
