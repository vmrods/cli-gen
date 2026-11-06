//! Plugins (ADR 0005): declarative manifests that add aliases, output formats
//! and credential providers to one API, and `opencli-<name>` executables on
//! `PATH` that run as `opencli <name> …`.

mod alias;
mod columns;
mod discovery;
mod manifest;
mod provider;

pub use alias::expand_alias;
pub use columns::render_columns;
pub use discovery::{EXECUTABLE_PREFIX, find_executable, list_executables};
pub use manifest::{
    Alias, Column, CredentialProvider, Format, ManifestError, Plugin, parse_manifest,
};
pub use provider::{ProviderError, fetch_credential};
