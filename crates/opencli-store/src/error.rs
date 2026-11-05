use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("an API named `{0}` already exists")]
    AlreadyExists(String),
    #[error("no API named `{0}`")]
    NotFound(String),
    #[error("could not open the metadata database at {path}")]
    Open {
        path: String,
        #[source]
        source: sqlx::Error,
    },
    #[error("metadata database migration failed")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("metadata database query failed")]
    Database(#[from] sqlx::Error),
    #[error("stored metadata is corrupt: {0}")]
    Corrupt(String),
    #[error("could not encode metadata as JSON")]
    Json(#[from] serde_json::Error),
}
