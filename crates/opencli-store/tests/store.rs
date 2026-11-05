// Helpers outside #[test] panic on purpose: that is how they fail the calling test.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use opencli_core::{ApiName, ApiSpec};
use opencli_store::{SpecImport, Store, StoreError};
use tempfile::TempDir;
use time::OffsetDateTime;
use time::macros::datetime;

const PETSTORE_YAML: &str = include_str!("../../../fixtures/petstore.yaml");
const IMPORTED_AT: OffsetDateTime = datetime!(2026-11-05 12:00 UTC);

struct Fixture {
    _directory: TempDir,
    path: PathBuf,
    store: Store,
    spec: ApiSpec,
}

impl Fixture {
    async fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("metadata.db");
        let store = Store::open(&path).await.unwrap();
        let spec = opencli_spec::parse(PETSTORE_YAML, "petstore.yaml").unwrap();
        Self {
            _directory: directory,
            path,
            store,
            spec,
        }
    }

    fn import(&self) -> SpecImport<'_> {
        SpecImport {
            source: "fixtures/petstore.yaml",
            spec: &self.spec,
            document: PETSTORE_YAML,
            imported_at: IMPORTED_AT,
        }
    }

    async fn add(&self, name: &str) -> Result<opencli_store::ApiSummary, StoreError> {
        self.store.add_api(&name_of(name), self.import()).await
    }

    async fn count(&self, table: &str) -> i64 {
        let pool = sqlx::SqlitePool::connect(&format!("sqlite://{}", self.path.display()))
            .await
            .unwrap();
        let query = match table {
            "operations" => "SELECT COUNT(*) FROM operations",
            "parameters" => "SELECT COUNT(*) FROM parameters",
            "spec_versions" => "SELECT COUNT(*) FROM spec_versions",
            other => panic!("no count query for {other}"),
        };
        sqlx::query_scalar(query).fetch_one(&pool).await.unwrap()
    }
}

fn name_of(raw: &str) -> ApiName {
    ApiName::parse(raw).unwrap()
}

#[tokio::test]
async fn add_api_returns_summary_of_current_version() {
    let fixture = Fixture::new().await;
    let summary = fixture.add("petstore").await.unwrap();
    assert_eq!(summary.name, "petstore");
    assert_eq!(summary.source, "fixtures/petstore.yaml");
    assert_eq!(summary.format, "openapi-3.0");
    assert_eq!(summary.title, "Petstore");
    assert_eq!(summary.version, "1.0.0");
    assert_eq!(summary.operation_count, 4);
    assert_eq!(summary.imported_at, IMPORTED_AT);
}

#[tokio::test]
async fn add_api_rejects_duplicate_name() {
    let fixture = Fixture::new().await;
    fixture.add("petstore").await.unwrap();
    let error = fixture.add("petstore").await.unwrap_err();
    assert!(
        matches!(&error, StoreError::AlreadyExists(name) if name == "petstore"),
        "{error:?}"
    );
}

#[tokio::test]
async fn list_apis_returns_all_apis_sorted_by_name() {
    let fixture = Fixture::new().await;
    fixture.add("zoo").await.unwrap();
    fixture.add("crm").await.unwrap();
    let names: Vec<_> = fixture
        .store
        .list_apis()
        .await
        .unwrap()
        .into_iter()
        .map(|api| api.name)
        .collect();
    assert_eq!(names, ["crm", "zoo"]);
}

#[tokio::test]
async fn list_apis_is_empty_for_new_store() {
    let fixture = Fixture::new().await;
    assert_eq!(fixture.store.list_apis().await.unwrap(), Vec::new());
}

#[tokio::test]
async fn api_details_include_servers_operations_and_schemes() {
    let fixture = Fixture::new().await;
    fixture.add("petstore").await.unwrap();
    let details = fixture
        .store
        .api_details(&name_of("petstore"))
        .await
        .unwrap();

    assert_eq!(
        details.description.as_deref(),
        Some("Sample pet store used by OpenCLI tests.")
    );
    assert_eq!(details.servers, ["https://eu.petstore.example.com/v1"]);
    let operations: Vec<_> = details
        .operations
        .iter()
        .map(|operation| format!("{} {}", operation.method, operation.path))
        .collect();
    assert_eq!(
        operations,
        [
            "GET /pets",
            "POST /pets",
            "GET /pets/{petId}",
            "DELETE /pets/{petId}"
        ]
    );
    assert!(details.operations[3].deprecated);
    let schemes: Vec<_> = details
        .security_schemes
        .iter()
        .map(|scheme| (scheme.name.as_str(), scheme.kind.as_str()))
        .collect();
    assert_eq!(
        schemes,
        [
            ("apiKey", "api-key"),
            ("basic", "http-basic"),
            ("bearer", "http-bearer"),
            ("oauth", "oauth2")
        ]
    );
}

#[tokio::test]
async fn api_details_fails_for_unknown_api() {
    let fixture = Fixture::new().await;
    let error = fixture
        .store
        .api_details(&name_of("nope"))
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::NotFound(_)), "{error:?}");
}

#[tokio::test]
async fn list_versions_marks_single_import_current() {
    let fixture = Fixture::new().await;
    fixture.add("petstore").await.unwrap();
    let versions = fixture
        .store
        .list_versions(&name_of("petstore"))
        .await
        .unwrap();
    assert_eq!(versions.len(), 1);
    let version = &versions[0];
    assert_eq!(version.number, 1);
    assert!(version.current);
    assert_eq!(version.operation_count, 4);
    assert_eq!(version.content_hash.len(), 64);
    assert!(version.content_hash.chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn list_versions_fails_for_unknown_api() {
    let fixture = Fixture::new().await;
    let error = fixture
        .store
        .list_versions(&name_of("nope"))
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::NotFound(_)), "{error:?}");
}

#[tokio::test]
async fn remove_api_deletes_all_metadata() {
    let fixture = Fixture::new().await;
    fixture.add("petstore").await.unwrap();
    fixture
        .store
        .remove_api(&name_of("petstore"))
        .await
        .unwrap();

    assert_eq!(fixture.store.list_apis().await.unwrap(), Vec::new());
    assert_eq!(fixture.count("spec_versions").await, 0);
    assert_eq!(fixture.count("operations").await, 0);
    assert_eq!(fixture.count("parameters").await, 0);
}

#[tokio::test]
async fn remove_api_fails_for_unknown_api() {
    let fixture = Fixture::new().await;
    let error = fixture
        .store
        .remove_api(&name_of("nope"))
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::NotFound(_)), "{error:?}");
}

#[tokio::test]
async fn open_preserves_data_across_reopen() {
    let fixture = Fixture::new().await;
    fixture.add("petstore").await.unwrap();
    let reopened = Store::open(&fixture.path).await.unwrap();
    assert_eq!(reopened.list_apis().await.unwrap().len(), 1);
}

#[tokio::test]
async fn open_fails_when_directory_is_missing() {
    let directory = TempDir::new().unwrap();
    let error = Store::open(&directory.path().join("missing/metadata.db"))
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::Open { .. }), "{error:?}");
}

#[tokio::test]
async fn current_spec_round_trips_the_imported_model() {
    let fixture = Fixture::new().await;
    fixture.add("petstore").await.unwrap();
    let stored = fixture
        .store
        .current_spec(&name_of("petstore"))
        .await
        .unwrap();
    assert_eq!(stored, fixture.spec);
}

#[tokio::test]
async fn current_spec_round_trips_cli_hints() {
    let directory = TempDir::new().unwrap();
    let store = Store::open(&directory.path().join("metadata.db"))
        .await
        .unwrap();
    let text = "openapi: 3.0.3\ninfo: {title: Hints, version: '1'}\npaths:\n  /a:\n    get:\n      x-cli-name: recent\n      x-cli-resource: audit logs\n      responses: {'200': {description: ok}}\n";
    let spec = opencli_spec::parse(text, "hints.yaml").unwrap();
    let import = SpecImport {
        source: "hints.yaml",
        spec: &spec,
        document: text,
        imported_at: IMPORTED_AT,
    };
    store.add_api(&name_of("hints"), import).await.unwrap();
    assert_eq!(store.current_spec(&name_of("hints")).await.unwrap(), spec);
}

#[tokio::test]
async fn current_spec_fails_for_unknown_api() {
    let fixture = Fixture::new().await;
    let error = fixture
        .store
        .current_spec(&name_of("nope"))
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::NotFound(_)), "{error:?}");
}
