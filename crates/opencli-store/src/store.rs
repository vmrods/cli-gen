use std::path::Path;

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;

use opencli_core::{
    ApiName, ApiSpec, CliHints, HttpMethod, Operation, Parameter, ParameterLocation, RequestBody,
    SecurityRequirement, SecurityScheme,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use sqlx::{FromRow, Sqlite, Transaction};
use time::OffsetDateTime;

use crate::StoreError;

/// A CLI process issues queries one at a time; more connections buy nothing.
const MAX_CONNECTIONS: u32 = 1;

/// Shared SELECT for [`ApiSummary`]; a macro so `concat!` keeps every query a literal.
macro_rules! summary_query {
    () => {
        "SELECT a.name, a.source, v.format, v.title, v.info_version AS version, v.imported_at, \
                (SELECT COUNT(*) FROM operations o WHERE o.version_id = v.id) AS operation_count \
         FROM apis a \
         JOIN spec_versions v ON v.id = (SELECT MAX(id) FROM spec_versions WHERE api_id = a.id)"
    };
}

#[derive(Debug, Clone)]
pub struct Store {
    pool: SqlitePool,
}

/// A validated specification ready to be stored.
#[derive(Debug, Clone, Copy)]
pub struct SpecImport<'a> {
    /// The file path or URL the document came from, as the user gave it.
    pub source: &'a str,
    pub spec: &'a ApiSpec,
    /// The raw document text, kept for diffing on refresh.
    pub document: &'a str,
    pub imported_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct ApiSummary {
    pub name: String,
    pub source: String,
    pub format: String,
    pub title: String,
    pub version: String,
    pub operation_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub imported_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ApiDetails {
    #[serde(flatten)]
    pub summary: ApiSummary,
    pub description: Option<String>,
    pub servers: Vec<String>,
    pub operations: Vec<OperationSummary>,
    pub security_schemes: Vec<SchemeSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct OperationSummary {
    pub method: String,
    pub path: String,
    pub operation_id: Option<String>,
    pub summary: Option<String>,
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct SchemeSummary {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct VersionSummary {
    pub number: i64,
    pub title: String,
    pub version: String,
    pub content_hash: String,
    pub operation_count: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub imported_at: OffsetDateTime,
    pub current: bool,
}

#[derive(FromRow)]
struct ApiRow {
    id: i64,
}

impl Store {
    /// Opens (creating if needed) the database at `path` and applies migrations.
    ///
    /// # Errors
    /// Fails when the file can't be opened or a migration fails.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(MAX_CONNECTIONS)
            .connect_with(options)
            .await
            .map_err(|source| StoreError::Open {
                path: path.display().to_string(),
                source,
            })?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    /// Registers a new API with its first specification version.
    ///
    /// # Errors
    /// [`StoreError::AlreadyExists`] when `name` is taken; database errors otherwise.
    pub async fn add_api(
        &self,
        name: &ApiName,
        import: SpecImport<'_>,
    ) -> Result<ApiSummary, StoreError> {
        let mut transaction = self.pool.begin().await?;
        let inserted = sqlx::query_as::<_, ApiRow>(
            "INSERT INTO apis (name, source, created_at) VALUES (?, ?, ?) RETURNING id",
        )
        .bind(name.as_str())
        .bind(import.source)
        .bind(import.imported_at)
        .fetch_one(&mut *transaction)
        .await;
        let api = match inserted {
            Ok(api) => api,
            Err(error) if is_unique_violation(&error) => {
                return Err(StoreError::AlreadyExists(name.to_string()));
            }
            Err(error) => return Err(error.into()),
        };
        insert_version(&mut transaction, api.id, import).await?;
        transaction.commit().await?;
        self.api_summary(name).await
    }

    /// # Errors
    /// Database errors only.
    pub async fn list_apis(&self) -> Result<Vec<ApiSummary>, StoreError> {
        let apis = sqlx::query_as::<_, ApiSummary>(concat!(summary_query!(), " ORDER BY a.name"))
            .fetch_all(&self.pool)
            .await?;
        Ok(apis)
    }

    /// # Errors
    /// [`StoreError::NotFound`] when no API has this name.
    pub async fn api_details(&self, name: &ApiName) -> Result<ApiDetails, StoreError> {
        let summary = self.api_summary(name).await?;
        let version_id = self.current_version_id(name).await?;
        let description: Option<String> =
            sqlx::query_scalar("SELECT description FROM spec_versions WHERE id = ?")
                .bind(version_id)
                .fetch_one(&self.pool)
                .await?;
        let servers =
            sqlx::query_scalar("SELECT url FROM servers WHERE version_id = ? ORDER BY position")
                .bind(version_id)
                .fetch_all(&self.pool)
                .await?;
        let operations = sqlx::query_as::<_, OperationSummary>(
            "SELECT method, path, operation_id, summary, deprecated FROM operations \
             WHERE version_id = ? ORDER BY position",
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await?;
        let security_schemes = sqlx::query_as::<_, SchemeSummary>(
            "SELECT name, kind FROM security_schemes WHERE version_id = ? ORDER BY name",
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(ApiDetails {
            summary,
            description,
            servers,
            operations,
            security_schemes,
        })
    }

    /// Versions oldest first; the last one is current.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] when no API has this name.
    pub async fn list_versions(&self, name: &ApiName) -> Result<Vec<VersionSummary>, StoreError> {
        let versions = sqlx::query_as::<_, VersionSummary>(
            "SELECT ROW_NUMBER() OVER (ORDER BY v.id) AS number, v.title, v.info_version AS version, \
                    v.content_hash, v.imported_at, \
                    (SELECT COUNT(*) FROM operations o WHERE o.version_id = v.id) AS operation_count, \
                    v.id = (SELECT MAX(id) FROM spec_versions WHERE api_id = a.id) AS current \
             FROM apis a JOIN spec_versions v ON v.api_id = a.id \
             WHERE a.name = ? ORDER BY v.id",
        )
        .bind(name.as_str())
        .fetch_all(&self.pool)
        .await?;
        if versions.is_empty() {
            return Err(StoreError::NotFound(name.to_string()));
        }
        Ok(versions)
    }

    /// Removes the API with all its versions. Credentials are not touched here.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] when no API has this name.
    pub async fn remove_api(&self, name: &ApiName) -> Result<(), StoreError> {
        let result = sqlx::query("DELETE FROM apis WHERE name = ?")
            .bind(name.as_str())
            .execute(&self.pool)
            .await?;
        if result.rows_affected() == 0 {
            return Err(StoreError::NotFound(name.to_string()));
        }
        Ok(())
    }

    /// Rebuilds the current specification version as a domain model.
    ///
    /// # Errors
    /// [`StoreError::NotFound`] when no API has this name; [`StoreError::Corrupt`]
    /// when stored values no longer decode.
    pub async fn current_spec(&self, name: &ApiName) -> Result<ApiSpec, StoreError> {
        let version_id = self.current_version_id(name).await?;
        let header = sqlx::query_as::<_, VersionHeaderRow>(
            "SELECT format, title, info_version, description FROM spec_versions WHERE id = ?",
        )
        .bind(version_id)
        .fetch_one(&self.pool)
        .await?;
        let servers =
            sqlx::query_scalar("SELECT url FROM servers WHERE version_id = ? ORDER BY position")
                .bind(version_id)
                .fetch_all(&self.pool)
                .await?;
        let mut parameters = self.parameters_by_operation(version_id).await?;
        let operations = sqlx::query_as::<_, OperationRow>(
            "SELECT id, operation_id, method, path, summary, description, deprecated, \
                    request_body, security, cli_name, cli_resource \
             FROM operations WHERE version_id = ? ORDER BY position",
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|row| {
            let operation_parameters = parameters.remove(&row.id).unwrap_or_default();
            row.into_operation(operation_parameters)
        })
        .collect::<Result<Vec<_>, _>>()?;
        let security_schemes = self.named_json(SCHEMES_QUERY, version_id).await?;
        let schemas = self.named_json(SCHEMAS_QUERY, version_id).await?;
        Ok(ApiSpec {
            format: parse_stored(&header.format)?,
            title: header.title,
            version: header.info_version,
            description: header.description,
            servers,
            operations,
            security_schemes,
            schemas,
        })
    }

    async fn parameters_by_operation(
        &self,
        version_id: i64,
    ) -> Result<HashMap<i64, Vec<Parameter>>, StoreError> {
        let rows = sqlx::query_as::<_, ParameterRow>(
            "SELECT p.operation_id AS operation_row_id, p.name, p.location, p.required, \
                    p.description, p.schema \
             FROM parameters p JOIN operations o ON o.id = p.operation_id \
             WHERE o.version_id = ? ORDER BY p.operation_id, p.position",
        )
        .bind(version_id)
        .fetch_all(&self.pool)
        .await?;
        let mut grouped: HashMap<i64, Vec<Parameter>> = HashMap::new();
        for row in rows {
            let operation_row_id = row.operation_row_id;
            grouped
                .entry(operation_row_id)
                .or_default()
                .push(row.into_parameter()?);
        }
        Ok(grouped)
    }

    async fn named_json<T: DeserializeOwned>(
        &self,
        query: &'static str,
        version_id: i64,
    ) -> Result<BTreeMap<String, T>, StoreError> {
        sqlx::query_as::<_, NamedJsonRow>(query)
            .bind(version_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|row| Ok((row.name, decode_json(&row.definition)?)))
            .collect()
    }

    async fn api_summary(&self, name: &ApiName) -> Result<ApiSummary, StoreError> {
        sqlx::query_as::<_, ApiSummary>(concat!(summary_query!(), " WHERE a.name = ?"))
            .bind(name.as_str())
            .fetch_optional(&self.pool)
            .await?
            .ok_or_else(|| StoreError::NotFound(name.to_string()))
    }

    async fn current_version_id(&self, name: &ApiName) -> Result<i64, StoreError> {
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT MAX(v.id) FROM apis a JOIN spec_versions v ON v.api_id = a.id WHERE a.name = ?",
        )
        .bind(name.as_str())
        .fetch_one(&self.pool)
        .await?
        .ok_or_else(|| StoreError::NotFound(name.to_string()))
    }
}

const SCHEMES_QUERY: &str =
    "SELECT name, definition FROM security_schemes WHERE version_id = ? ORDER BY name";
const SCHEMAS_QUERY: &str =
    "SELECT name, definition FROM schemas WHERE version_id = ? ORDER BY name";

#[derive(FromRow)]
struct VersionHeaderRow {
    format: String,
    title: String,
    info_version: String,
    description: Option<String>,
}

#[derive(FromRow)]
struct OperationRow {
    id: i64,
    operation_id: Option<String>,
    method: String,
    path: String,
    summary: Option<String>,
    description: Option<String>,
    deprecated: bool,
    request_body: Option<String>,
    security: String,
    cli_name: Option<String>,
    cli_resource: Option<String>,
}

impl OperationRow {
    fn into_operation(self, parameters: Vec<Parameter>) -> Result<Operation, StoreError> {
        Ok(Operation {
            operation_id: self.operation_id,
            method: parse_stored::<HttpMethod>(&self.method)?,
            path: self.path,
            summary: self.summary,
            description: self.description,
            deprecated: self.deprecated,
            parameters,
            request_body: self
                .request_body
                .as_deref()
                .map(decode_json::<RequestBody>)
                .transpose()?,
            security: decode_json::<Vec<SecurityRequirement>>(&self.security)?,
            cli: CliHints {
                name: self.cli_name,
                resource: self.cli_resource.as_deref().map(decode_json).transpose()?,
            },
        })
    }
}

#[derive(FromRow)]
struct ParameterRow {
    operation_row_id: i64,
    name: String,
    location: String,
    required: bool,
    description: Option<String>,
    schema: Option<String>,
}

impl ParameterRow {
    fn into_parameter(self) -> Result<Parameter, StoreError> {
        Ok(Parameter {
            name: self.name,
            location: parse_stored::<ParameterLocation>(&self.location)?,
            required: self.required,
            description: self.description,
            schema: self.schema.as_deref().map(decode_json).transpose()?,
        })
    }
}

#[derive(FromRow)]
struct NamedJsonRow {
    name: String,
    definition: String,
}

fn parse_stored<T>(raw: &str) -> Result<T, StoreError>
where
    T: FromStr,
    T::Err: std::fmt::Display,
{
    raw.parse()
        .map_err(|error: T::Err| StoreError::Corrupt(error.to_string()))
}

fn decode_json<T: DeserializeOwned>(raw: &str) -> Result<T, StoreError> {
    serde_json::from_str(raw).map_err(|error| StoreError::Corrupt(error.to_string()))
}

async fn insert_version(
    transaction: &mut Transaction<'_, Sqlite>,
    api_id: i64,
    import: SpecImport<'_>,
) -> Result<(), StoreError> {
    let spec = import.spec;
    let version_id: i64 = sqlx::query_scalar(
        "INSERT INTO spec_versions \
         (api_id, format, title, info_version, description, content_hash, document, imported_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(api_id)
    .bind(spec.format.as_str())
    .bind(&spec.title)
    .bind(&spec.version)
    .bind(&spec.description)
    .bind(content_hash(import.document))
    .bind(import.document)
    .bind(import.imported_at)
    .fetch_one(&mut **transaction)
    .await?;

    for (position, url) in (0_i64..).zip(&spec.servers) {
        sqlx::query("INSERT INTO servers (version_id, position, url) VALUES (?, ?, ?)")
            .bind(version_id)
            .bind(position)
            .bind(url)
            .execute(&mut **transaction)
            .await?;
    }
    for (position, operation) in (0_i64..).zip(&spec.operations) {
        insert_operation(transaction, version_id, position, operation).await?;
    }
    for (name, scheme) in &spec.security_schemes {
        sqlx::query(
            "INSERT INTO security_schemes (version_id, name, kind, definition) VALUES (?, ?, ?, ?)",
        )
        .bind(version_id)
        .bind(name)
        .bind(scheme_kind(scheme))
        .bind(serde_json::to_string(scheme)?)
        .execute(&mut **transaction)
        .await?;
    }
    for (name, schema) in &spec.schemas {
        sqlx::query("INSERT INTO schemas (version_id, name, definition) VALUES (?, ?, ?)")
            .bind(version_id)
            .bind(name)
            .bind(serde_json::to_string(schema)?)
            .execute(&mut **transaction)
            .await?;
    }
    Ok(())
}

async fn insert_operation(
    transaction: &mut Transaction<'_, Sqlite>,
    version_id: i64,
    position: i64,
    operation: &Operation,
) -> Result<(), StoreError> {
    let request_body = operation
        .request_body
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let operation_row_id: i64 = sqlx::query_scalar(
        "INSERT INTO operations \
         (version_id, position, operation_id, method, path, summary, description, deprecated, \
          request_body, security, cli_name, cli_resource) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(version_id)
    .bind(position)
    .bind(&operation.operation_id)
    .bind(operation.method.as_str())
    .bind(&operation.path)
    .bind(&operation.summary)
    .bind(&operation.description)
    .bind(operation.deprecated)
    .bind(request_body)
    .bind(serde_json::to_string(&operation.security)?)
    .bind(&operation.cli.name)
    .bind(
        operation
            .cli
            .resource
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?,
    )
    .fetch_one(&mut **transaction)
    .await?;

    for (position, parameter) in (0_i64..).zip(&operation.parameters) {
        let schema = parameter
            .schema
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        sqlx::query(
            "INSERT INTO parameters \
             (operation_id, position, name, location, required, description, schema) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(operation_row_id)
        .bind(position)
        .bind(&parameter.name)
        .bind(parameter.location.as_str())
        .bind(parameter.required)
        .bind(&parameter.description)
        .bind(schema)
        .execute(&mut **transaction)
        .await?;
    }
    Ok(())
}

fn scheme_kind(scheme: &SecurityScheme) -> &'static str {
    match scheme {
        SecurityScheme::ApiKey { .. } => "api-key",
        SecurityScheme::HttpBasic => "http-basic",
        SecurityScheme::HttpBearer { .. } => "http-bearer",
        SecurityScheme::OAuth2 { .. } => "oauth2",
        SecurityScheme::OpenIdConnect { .. } => "open-id-connect",
        SecurityScheme::Unsupported { .. } => "unsupported",
    }
}

fn content_hash(document: &str) -> String {
    format!("{:x}", Sha256::digest(document.as_bytes()))
}

fn is_unique_violation(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(sqlx::error::DatabaseError::is_unique_violation)
}
