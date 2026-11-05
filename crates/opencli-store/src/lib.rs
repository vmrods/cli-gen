//! SQLite metadata store for imported specifications (ADR 0003).

mod error;
mod store;

pub use error::StoreError;
pub use store::{
    ApiDetails, ApiSummary, OperationSummary, SchemeSummary, SpecImport, Store, VersionChange,
    VersionSummary,
};
