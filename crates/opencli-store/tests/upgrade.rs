//! Databases created by earlier builds must open with the current store, and
//! every backfill in later migrations must hold.

// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use opencli_core::ApiName;
use opencli_store::Store;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use tempfile::TempDir;

const CREATED_AT: &str = "2026-11-05T12:00:00Z";

/// Applies only the migrations M1 shipped (0001–0002), as an old build would have.
async fn database_from_m1(directory: &TempDir) -> std::path::PathBuf {
    let migrations = directory.path().join("m1-migrations");
    std::fs::create_dir(&migrations).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for file in ["0001_initial.sql", "0002_cli_hints.sql"] {
        std::fs::copy(source.join(file), migrations.join(file)).unwrap();
    }
    let database = directory.path().join("metadata.db");
    let options = SqliteConnectOptions::new()
        .filename(&database)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .unwrap();
    Migrator::new(migrations.as_path())
        .await
        .unwrap()
        .run(&pool)
        .await
        .unwrap();

    // Data in the M1 shape: no current_version_id, updated_at or array_delimiter yet.
    for statement in [
        format!("INSERT INTO apis (id, name, source, created_at) VALUES (1, 'legacy', 'legacy.yaml', '{CREATED_AT}')"),
        format!(
            "INSERT INTO spec_versions (id, api_id, format, title, info_version, content_hash, document, imported_at) \
             VALUES (1, 1, 'openapi-3.0', 'Legacy', '1.0', 'hash-1', 'openapi: 3.0.3', '{CREATED_AT}'), \
                    (2, 1, 'openapi-3.0', 'Legacy', '2.0', 'hash-2', 'openapi: 3.0.3', '{CREATED_AT}')"
        ),
        "INSERT INTO operations (id, version_id, position, operation_id, method, path, deprecated, security) \
         VALUES (1, 2, 0, 'getThing', 'GET', '/things/{id}', 0, '[]')"
            .to_owned(),
        "INSERT INTO parameters (operation_id, position, name, location, required) VALUES (1, 0, 'id', 'path', 1)"
            .to_owned(),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(statement)).execute(&pool).await.unwrap();
    }
    pool.close().await;
    database
}

#[tokio::test]
async fn databases_from_m1_upgrade_with_backfilled_columns() {
    let directory = TempDir::new().unwrap();
    let database = database_from_m1(&directory).await;

    let store = Store::open(&database).await.unwrap();
    let legacy = ApiName::parse("legacy").unwrap();

    let summary = store.api_summary(&legacy).await.unwrap();
    assert_eq!(
        summary.version, "2.0",
        "the newest version becomes current (0003)"
    );
    assert_eq!(
        summary.updated_at, summary.imported_at,
        "updated_at is backfilled from created_at (0005)"
    );
    assert_eq!(
        summary.updated_at.to_string(),
        "2026-11-05 12:00:00.0 +00:00:00"
    );

    let spec = store.current_spec(&legacy).await.unwrap();
    let parameter = &spec.operations[0].parameters[0];
    assert_eq!(parameter.name, "id");
    assert_eq!(
        parameter.array_delimiter, None,
        "0004 adds a nullable column"
    );
    assert_eq!(spec.operations[0].cli.name, None);

    let versions = store.list_versions(&legacy).await.unwrap();
    let flags: Vec<_> = versions
        .iter()
        .map(|version| (version.number, version.current))
        .collect();
    assert_eq!(flags, [(1, false), (2, true)]);
}

#[tokio::test]
async fn upgraded_databases_reopen_without_rerunning_migrations() {
    let directory = TempDir::new().unwrap();
    let database = database_from_m1(&directory).await;
    Store::open(&database).await.unwrap();
    let reopened = Store::open(&database).await.unwrap();
    assert_eq!(reopened.list_apis().await.unwrap().len(), 1);
}
