//! Durable SurrealDB-backed `RegistryStore` implementation.
//!
//! The control namespace is bound at construction; every read/write
//! resolves against that binding. The store dispatches between the
//! embedded (`Db`) and remote Ws (`Client`) engines by holding each
//! connection behind an enum arm. Every SQL statement that takes
//! user-controlled values uses parameterised queries (`$param`)
//! and the helper [`bind`] centralises the placeholder rendering.
//!
//! Production code constructs `SurrealRegistryStore::connect(...)` or
//! `SurrealRegistryStore::connect_in_memory(...)`; there is no in-memory
//! or unavailable fallback in the production constructor.

use std::sync::Arc;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use surrealdb::Surreal;
use surrealdb::engine::local::Db;
use surrealdb::engine::remote::ws::{Client, Ws, Wss};
use surrealdb::opt::auth as surrealdb_auth;

use super::models::*;
use super::storage::{LeaseFence, RegistryStore, is_safe_identifier};
use crate::error::MemoryError;

/// Backing connection variant. Both arms hold the already-connected
/// `Surreal<C>` handle. The variant is selected at startup based on
/// `SurrealTargetConfig`.
#[derive(Clone)]
pub enum RegistryDb {
    Remote(Arc<Surreal<Client>>),
    Local(Arc<Surreal<Db>>),
}

impl RegistryDb {
    pub fn as_dyn(&self) -> &dyn SurrealHandle {
        match self {
            RegistryDb::Remote(db) => db.as_ref(),
            RegistryDb::Local(db) => db.as_ref(),
        }
    }
}

/// Abstraction over `Surreal<C>` so a single helper can issue
/// `query()` regardless of the connection variant.
#[async_trait]
pub trait SurrealHandle: Send + Sync {
    async fn use_ns_db(&self, namespace: &str, database: &str) -> Result<(), MemoryError>;
    async fn query_json(&self, sql: &str, vars: Option<Value>) -> Result<Vec<Value>, MemoryError>;
    async fn ping(&self) -> bool;
}

#[async_trait]
impl SurrealHandle for Surreal<Client> {
    async fn use_ns_db(&self, namespace: &str, database: &str) -> Result<(), MemoryError> {
        self.use_ns(namespace)
            .use_db(database)
            .await
            .map_err(|err| MemoryError::Storage(format!("bind failed: {err}")))?;
        Ok(())
    }
    async fn query_json(&self, sql: &str, vars: Option<Value>) -> Result<Vec<Value>, MemoryError> {
        let mut q = self.query(sql);
        if let Some(v) = vars {
            q = q.bind(v);
        }
        let mut response = q
            .await
            .map_err(|err| MemoryError::Storage(format!("query failed: {err}")))?;
        let statement_errors = response.take_errors();
        if !statement_errors.is_empty() {
            let details = statement_errors
                .into_iter()
                .map(|(index, error)| format!("statement {index}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(MemoryError::Storage(format!(
                "query statement errors: {details}"
            )));
        }
        let mut out = Vec::new();
        let result: Option<Value> = response
            .take::<Option<Value>>(0)
            .map_err(|err| MemoryError::Storage(format!("take failed: {err}")))?;
        if let Some(value) = result {
            match value {
                Value::Array(values) => out.extend(values),
                Value::Null => {}
                value => out.push(value),
            }
        }
        Ok(out)
    }
    async fn ping(&self) -> bool {
        match self.query("INFO FOR DB").await {
            Ok(mut response) => response.take_errors().is_empty(),
            Err(_) => false,
        }
    }
}

#[async_trait]
impl SurrealHandle for Surreal<Db> {
    async fn use_ns_db(&self, namespace: &str, database: &str) -> Result<(), MemoryError> {
        self.use_ns(namespace)
            .use_db(database)
            .await
            .map_err(|err| MemoryError::Storage(format!("bind failed: {err}")))?;
        Ok(())
    }
    async fn query_json(&self, sql: &str, vars: Option<Value>) -> Result<Vec<Value>, MemoryError> {
        let mut q = self.query(sql);
        if let Some(v) = vars {
            q = q.bind(v);
        }
        let mut response = q
            .await
            .map_err(|err| MemoryError::Storage(format!("query failed: {err}")))?;
        let statement_errors = response.take_errors();
        if !statement_errors.is_empty() {
            let details = statement_errors
                .into_iter()
                .map(|(index, error)| format!("statement {index}: {error}"))
                .collect::<Vec<_>>()
                .join("; ");
            return Err(MemoryError::Storage(format!(
                "query statement errors: {details}"
            )));
        }
        let mut out = Vec::new();
        let result: Option<Value> = response
            .take::<Option<Value>>(0)
            .map_err(|err| MemoryError::Storage(format!("take failed: {err}")))?;
        if let Some(value) = result {
            match value {
                Value::Array(values) => out.extend(values),
                Value::Null => {}
                value => out.push(value),
            }
        }
        Ok(out)
    }
    async fn ping(&self) -> bool {
        match self.query("INFO FOR DB").await {
            Ok(mut response) => response.take_errors().is_empty(),
            Err(_) => false,
        }
    }
}

/// Convert a SurrealDB error string into a typed MemoryError.
fn map_storage_error(context: &str, err: impl std::fmt::Display) -> MemoryError {
    let msg = err.to_string();
    let lower = msg.to_ascii_lowercase();
    if lower.contains("already exists") || lower.contains("duplicate") || lower.contains("unique") {
        MemoryError::Conflict(format!("{context}: {msg}"))
    } else if lower.contains("not found") || lower.contains("no record") {
        MemoryError::NotFound(format!("{context}: {msg}"))
    } else {
        MemoryError::Storage(format!("{context}: {msg}"))
    }
}

fn is_conflict_error(error: &MemoryError) -> bool {
    matches!(error, MemoryError::Conflict(message) if {
        let lower = message.to_ascii_lowercase();
        lower.contains("already exists") || lower.contains("duplicate") || lower.contains("unique")
    })
}

#[cfg(feature = "control-plane")]
fn classify_deletion_error(error: MemoryError) -> MemoryError {
    match error {
        MemoryError::Storage(message) => {
            let lower = message.to_ascii_lowercase();
            if lower.contains("deletion challenge")
                || lower.contains("account is not active")
                || lower.contains("tombstone")
            {
                MemoryError::Conflict(message)
            } else {
                MemoryError::Storage(message)
            }
        }
        other => other,
    }
}

fn migration_checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}

fn migration_ledger_id(file_name: &str) -> String {
    let safe = file_name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    format!("migration_{safe}")
}

fn migration_lease_active(row: &Value) -> Result<bool, MemoryError> {
    let Some(value) = row.get("lease_expires_at") else {
        return Ok(false);
    };
    if value.is_null() {
        return Ok(false);
    }
    let raw = value
        .as_str()
        .or_else(|| value.get("Datetime").and_then(Value::as_str))
        .ok_or_else(|| {
            MemoryError::Storage("migration ledger has an invalid lease expiry".into())
        })?;
    let expiry = DateTime::parse_from_rfc3339(raw).map_err(|error| {
        MemoryError::Storage(format!(
            "migration ledger has an invalid lease expiry: {error}"
        ))
    })?;
    Ok(expiry > chrono::Utc::now())
}

/// Durable control-namespace registry store.
pub struct SurrealRegistryStore {
    db: RegistryDb,
    namespace: String,
    database: String,
}

fn datetime_value(value: &Value) -> Option<DateTime<Utc>> {
    let raw = value
        .as_str()
        .or_else(|| value.get("Datetime").and_then(Value::as_str))
        .or_else(|| value.get("datetime").and_then(Value::as_str))?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|parsed| parsed.with_timezone(&Utc))
}

fn required_datetime(row: &Value, field: &str) -> Result<DateTime<Utc>, MemoryError> {
    datetime_value(row.get(field).unwrap_or(&Value::Null))
        .ok_or_else(|| MemoryError::Storage(format!("registry row has invalid {field}")))
}

fn record_id_value(value: &Value) -> Option<String> {
    fn key_without_table(value: &str) -> String {
        value
            .rsplit_once(':')
            .map_or_else(|| value.to_owned(), |(_, key)| key.to_owned())
    }
    match value {
        Value::String(value) => Some(key_without_table(value)),
        Value::Object(object) => object
            .get("key")
            .or_else(|| object.get("id"))
            .and_then(|value| value.as_str())
            .map(key_without_table)
            .or_else(|| object.get("RecordId").and_then(record_id_value)),
        _ => None,
    }
}

fn row_id(row: &Value, fallback: &str) -> String {
    row.get("id")
        .and_then(record_id_value)
        .unwrap_or_else(|| fallback.to_owned())
}

fn bytes_value(value: &Value) -> Option<[u8; 32]> {
    if let Some(hex_value) = value.as_str() {
        let bytes = hex::decode(hex_value).ok()?;
        return bytes.try_into().ok();
    }
    let array = value.get("0").or(Some(value)).and_then(Value::as_array)?;
    let bytes = array
        .iter()
        .map(|item| item.as_u64().and_then(|value| u8::try_from(value).ok()))
        .collect::<Option<Vec<_>>>()?;
    bytes.try_into().ok()
}

fn status_from_row<T: serde::de::DeserializeOwned>(
    row: &Value,
    field: &str,
) -> Result<T, MemoryError> {
    let value = row
        .get(field)
        .cloned()
        .ok_or_else(|| MemoryError::Storage(format!("registry row has no {field}")))?;
    serde_json::from_value(value)
        .map_err(|error| MemoryError::Storage(format!("decode registry {field}: {error}")))
}

fn required_u64(row: &Value, field: &str) -> Result<u64, MemoryError> {
    row.get(field)
        .and_then(Value::as_u64)
        .ok_or_else(|| MemoryError::Storage(format!("registry row has invalid {field}")))
}

fn required_u32(row: &Value, field: &str) -> Result<u32, MemoryError> {
    let value = required_u64(row, field)?;
    u32::try_from(value)
        .map_err(|_| MemoryError::Storage(format!("registry row has out-of-range {field}")))
}

fn encoded_status<T: serde::Serialize>(status: T, field: &str) -> Result<String, MemoryError> {
    let value = serde_json::to_value(status)
        .map_err(|error| MemoryError::Storage(format!("encode {field}: {error}")))?;
    value
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| MemoryError::Storage(format!("encode {field} did not produce a string")))
}

fn returned_version(rows: &[Value], operation: &str) -> Result<u64, MemoryError> {
    rows.first()
        .and_then(|row| row.get("version"))
        .and_then(Value::as_u64)
        .ok_or_else(|| MemoryError::Storage(format!("{operation} returned an invalid version")))
}

fn decode_account(row: &Value) -> Result<Account, MemoryError> {
    Ok(Account {
        id: row_id(row, ""),
        status: status_from_row(row, "status")?,
        tenant_id: row
            .get("tenant_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        created_at: required_datetime(row, "created_at")?,
    })
}

fn decode_tenant(row: &Value) -> Result<Tenant, MemoryError> {
    let binding = row
        .get("namespace_binding")
        .and_then(Value::as_object)
        .ok_or_else(|| MemoryError::Storage("tenant row has no namespace_binding".into()))?;
    let lease = row.get("provisioning_lease").and_then(|value| {
        let object = value.as_object()?;
        Some(ProvisioningLeaseState {
            owner_id: object.get("owner_id")?.as_str()?.to_owned(),
            lease_id: object.get("lease_id")?.as_str()?.to_owned(),
            expires_at: datetime_value(object.get("expires_at")?)?,
            fencing_generation: object.get("fencing_generation")?.as_u64()?,
            heartbeat_at: datetime_value(object.get("heartbeat_at")?)?,
        })
    });
    Ok(Tenant {
        id: row_id(row, ""),
        status: status_from_row(row, "status")?,
        namespace_binding: NamespaceBinding {
            namespace: binding
                .get("namespace")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            database: binding
                .get("database")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        },
        plan_version: row.get("plan_version").and_then(Value::as_u64).unwrap_or(0) as u32,
        schema_version: row
            .get("schema_version")
            .and_then(Value::as_u64)
            .unwrap_or(0) as u32,
        retry_stage: row
            .get("retry_stage")
            .and_then(Value::as_str)
            .map(|value| serde_json::from_value(Value::String(value.to_owned())))
            .transpose()
            .map_err(|error| MemoryError::Storage(format!("decode retry_stage: {error}")))?,
        provisioning_lease: lease,
        created_at: required_datetime(row, "created_at")?,
        version: row.get("version").and_then(Value::as_u64).unwrap_or(0),
    })
}

fn decode_api_key(row: &Value) -> Result<ApiKey, MemoryError> {
    let verifier = bytes_value(row.get("verifier").unwrap_or(&Value::Null))
        .ok_or_else(|| MemoryError::Storage("api key verifier is invalid".into()))?;
    Ok(ApiKey {
        id: row_id(row, ""),
        account_id: row
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        name: row
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        verifier: KeyedVerifier(verifier),
        status: status_from_row(row, "status")?,
        created_at: required_datetime(row, "created_at")?,
        expires_at: row.get("expires_at").and_then(datetime_value),
        last_used_at: row.get("last_used_at").and_then(datetime_value),
        version: row.get("version").and_then(Value::as_u64).unwrap_or(0),
    })
}

fn decode_identity(row: &Value) -> Result<ExternalIdentity, MemoryError> {
    let verifier = bytes_value(row.get("subject_verifier").unwrap_or(&Value::Null))
        .ok_or_else(|| MemoryError::Storage("identity subject verifier is invalid".into()))?;
    Ok(ExternalIdentity {
        id: row_id(row, ""),
        issuer: row
            .get("issuer")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        subject_verifier: SubjectVerifier(verifier),
        account_id: row
            .get("account_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        created_at: required_datetime(row, "created_at")?,
    })
}

impl SurrealRegistryStore {
    /// Build against an already-constructed engine. The caller
    /// is responsible for `use_ns`/`use_db` if needed; the store
    /// rebinds on every call so concurrent use is safe.
    pub async fn connect(target: &crate::config::SurrealTargetConfig) -> Result<Self, MemoryError> {
        let store = Self::connect_unmigrated(target).await?;
        store.apply_migrations().await?;
        Ok(store)
    }

    /// Connect a privileged engine for tenant namespace creation and binding.
    /// This deliberately does not apply control-plane migrations: the caller
    /// uses it for the tenant target, which may point at a different database.
    pub async fn connect_engine(
        target: &crate::config::SurrealTargetConfig,
    ) -> Result<crate::http::registry::PrivilegedEngine, MemoryError> {
        let store = Self::connect_unmigrated(target).await?;
        Ok(store.privileged_engine())
    }

    async fn connect_unmigrated(
        target: &crate::config::SurrealTargetConfig,
    ) -> Result<Self, MemoryError> {
        let url = target.url.trim();
        let db = if url == "mem://" || url == "mem" {
            let db = Surreal::new::<surrealdb::engine::local::Mem>(())
                .await
                .map_err(|err| map_storage_error("mem engine init", err))?;
            RegistryDb::Local(Arc::new(db))
        } else if let Some(rest) = url.strip_prefix("rocksdb://") {
            use surrealdb::opt::Config as SurrealOptConfig;
            use surrealdb::opt::auth::Root;
            use surrealdb::opt::capabilities::Capabilities;
            if rest.trim().is_empty() {
                return Err(MemoryError::Validation(
                    "rocksdb registry path is empty".into(),
                ));
            }
            let root = Root {
                username: target.username.clone(),
                password: target.password.clone(),
            };
            let cfg = SurrealOptConfig::default()
                .user(root.clone())
                .capabilities(Capabilities::default());
            let db = Surreal::new::<surrealdb::engine::local::RocksDb>((rest, cfg))
                .await
                .map_err(|err| map_storage_error("rocksdb init", err))?;
            db.signin(root)
                .await
                .map_err(|err| map_storage_error("rocksdb auth", err))?;
            RegistryDb::Local(Arc::new(db))
        } else if let Some(rest) = url.strip_prefix("ws://") {
            let db = Surreal::new::<Ws>(rest)
                .await
                .map_err(|err| map_storage_error("remote init", err))?;
            Self::signin_remote(&db, target).await?;
            RegistryDb::Remote(Arc::new(db))
        } else if let Some(rest) = url.strip_prefix("wss://") {
            let db = Surreal::new::<Wss>(rest)
                .await
                .map_err(|err| map_storage_error("remote init", err))?;
            Self::signin_remote(&db, target).await?;
            RegistryDb::Remote(Arc::new(db))
        } else {
            return Err(MemoryError::Validation(format!(
                "unsupported registry url scheme: {url}"
            )));
        };
        let namespace = target.namespace.trim().to_owned();
        let database = target.database.trim().to_owned();
        if !is_safe_identifier(&namespace) {
            return Err(MemoryError::Validation(format!(
                "registry namespace '{namespace}' is not a safe identifier"
            )));
        }
        if !is_safe_identifier(&database) {
            return Err(MemoryError::Validation(format!(
                "registry database '{database}' is not a safe identifier"
            )));
        }
        db.as_dyn().use_ns_db(&namespace, &database).await?;
        Ok(Self {
            db,
            namespace,
            database,
        })
    }

    async fn signin_remote<C: surrealdb::Connection>(
        db: &Surreal<C>,
        target: &crate::config::SurrealTargetConfig,
    ) -> Result<(), MemoryError> {
        if target.username.is_empty() || target.password.is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "remote SurrealDB registry credentials are required".into(),
            ));
        }
        db.signin(surrealdb_auth::Root {
            username: target.username.clone(),
            password: target.password.clone(),
        })
        .await
        .map_err(|err| map_storage_error("remote auth", err))?;
        Ok(())
    }

    /// Build against an in-memory engine for tests and apply the same durable
    /// registry migrations as production.
    pub async fn connect_in_memory(namespace: &str, database: &str) -> Result<Self, MemoryError> {
        let db = Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .map_err(|err| map_storage_error("mem engine init", err))?;
        let store = Self::from_local_db(Arc::new(db), namespace, database).await?;
        store.apply_migrations().await?;
        Ok(store)
    }

    /// Bind a caller-owned local engine. Tests use this to create two store
    /// instances over one shared Mem engine and prove persistence across handles.
    pub async fn from_local_db(
        db: Arc<Surreal<Db>>,
        namespace: &str,
        database: &str,
    ) -> Result<Self, MemoryError> {
        if !is_safe_identifier(namespace) || !is_safe_identifier(database) {
            return Err(MemoryError::Validation(
                "registry namespace/database must be safe identifiers".into(),
            ));
        }
        let store = Self {
            db: RegistryDb::Local(db),
            namespace: namespace.to_owned(),
            database: database.to_owned(),
        };
        store.handle().use_ns_db(namespace, database).await?;
        Ok(store)
    }

    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    pub fn database(&self) -> &str {
        &self.database
    }

    fn handle(&self) -> &dyn SurrealHandle {
        self.db.as_dyn()
    }

    /// Convert the connected registry handle into the privileged engine seam.
    pub fn privileged_engine(&self) -> crate::http::registry::PrivilegedEngine {
        match &self.db {
            RegistryDb::Remote(db) => crate::http::registry::PrivilegedEngine::Remote(db.clone()),
            RegistryDb::Local(db) => crate::http::registry::PrivilegedEngine::Local(db.clone()),
        }
    }

    /// Apply the control-plane migration catalog with a durable ledger.
    ///
    /// The ledger is bootstrapped with idempotent DDL, then each migration is
    /// claimed by a short datastore-time lease. An expired `applying` claim is
    /// recoverable; a completed row is never silently re-executed with a
    /// different checksum. Every statement error is surfaced by `query_json`,
    /// and the final table postconditions fail startup if the control schema is
    /// incomplete.
    pub async fn apply_migrations(&self) -> Result<Vec<String>, MemoryError> {
        self.handle()
            .query_json(
                "DEFINE TABLE IF NOT EXISTS migration_ledger SCHEMAFULL; \
                 DEFINE FIELD IF NOT EXISTS file_name ON migration_ledger TYPE string; \
                 DEFINE FIELD IF NOT EXISTS checksum ON migration_ledger TYPE string; \
                 DEFINE FIELD IF NOT EXISTS status ON migration_ledger TYPE string; \
                 DEFINE FIELD IF NOT EXISTS started_at ON migration_ledger TYPE datetime; \
                 DEFINE FIELD IF NOT EXISTS completed_at ON migration_ledger TYPE option<datetime>; \
                 DEFINE FIELD IF NOT EXISTS error ON migration_ledger TYPE option<string>; \
                 DEFINE FIELD IF NOT EXISTS owner ON migration_ledger TYPE option<string>; \
                 DEFINE FIELD IF NOT EXISTS lease_expires_at ON migration_ledger TYPE option<datetime>; \
                 DEFINE INDEX IF NOT EXISTS idx_migration_ledger_file ON migration_ledger FIELDS file_name UNIQUE;",
                None,
            )
            .await
            .map_err(|err| map_storage_error("bootstrap registry migration ledger", err))?;

        let mut verified = Vec::new();
        for name in super::migrations::REGISTRY_MIGRATIONS {
            let file_name = format!("{name}.surql");
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("migrations")
                .join(&file_name);
            let sql = std::fs::read_to_string(&path).map_err(|err| {
                MemoryError::Storage(format!(
                    "failed to read migration {}: {err}",
                    path.display()
                ))
            })?;
            let checksum = migration_checksum(&sql);
            let record_id = migration_ledger_id(&file_name);
            let owner = format!("pid:{}:{}", std::process::id(), uuid::Uuid::new_v4());
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);

            loop {
                let rows = self
                    .handle()
                    .query_json(
                        "SELECT * FROM type::table($table) WHERE file_name = $file LIMIT 1",
                        Some(json!({"table": "migration_ledger", "file": file_name})),
                    )
                    .await
                    .map_err(|err| map_storage_error("read registry migration ledger", err))?;

                if let Some(row) = rows.into_iter().next() {
                    let stored_checksum = row.get("checksum").and_then(Value::as_str);
                    if stored_checksum != Some(checksum.as_str()) {
                        return Err(MemoryError::ConfigInvalid(format!(
                            "registry migration {file_name} checksum differs from its durable ledger"
                        )));
                    }
                    match row.get("status").and_then(Value::as_str) {
                        Some("completed") => {
                            verified.push(file_name.clone());
                            break;
                        }
                        Some("failed") | Some("applying") => {
                            if migration_lease_active(&row)? {
                                if std::time::Instant::now() >= deadline {
                                    return Err(MemoryError::Unavailable(format!(
                                        "registry migration {file_name} is being applied by another replica"
                                    )));
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                                continue;
                            }
                            let claimed = self
                                .handle()
                                .query_json(
                                    "UPDATE type::record($table, $id) SET status = 'applying', owner = $owner, lease_expires_at = type::datetime($expiry), started_at = time::now(), completed_at = NONE, error = NONE WHERE (status = 'failed' OR status = 'applying') AND (lease_expires_at IS NONE OR lease_expires_at <= time::now()) RETURN AFTER",
                                    Some(json!({
                                        "table": "migration_ledger",
                                        "id": record_id,
                                        "owner": owner,
                                        "expiry": (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339(),
                                    })),
                                )
                                .await
                                .map_err(|err| map_storage_error("claim registry migration", err))?;
                            if claimed.is_empty() {
                                continue;
                            }
                            break;
                        }
                        Some(status) => {
                            return Err(MemoryError::Storage(format!(
                                "registry migration {file_name} has unsupported status {status}"
                            )));
                        }
                        None => {
                            return Err(MemoryError::Storage(format!(
                                "registry migration {file_name} ledger row has no status"
                            )));
                        }
                    }
                } else {
                    let created = self
                        .handle()
                        .query_json(
                            "CREATE type::record($table, $id) SET file_name = $file, checksum = $checksum, status = 'applying', owner = $owner, lease_expires_at = type::datetime($expiry), started_at = time::now() RETURN AFTER",
                            Some(json!({
                                "table": "migration_ledger",
                                "id": record_id,
                                "file": file_name,
                                "checksum": checksum,
                                "owner": owner,
                                "expiry": (Utc::now() + chrono::Duration::seconds(30)).to_rfc3339(),
                            })),
                        )
                        .await;
                    match created {
                        Ok(_) => break,
                        Err(error) if is_conflict_error(&error) => continue,
                        Err(error) => {
                            return Err(map_storage_error("reserve registry migration", error));
                        }
                    }
                }
            }

            let execution = self.handle().query_json(&sql, None).await;
            if let Err(error) = execution {
                let _ = self
                    .handle()
                    .query_json(
                        "UPDATE type::record($table, $id) SET status = 'failed', error = $error, owner = NONE, lease_expires_at = NONE WHERE owner = $owner RETURN AFTER",
                        Some(json!({
                            "table": "migration_ledger",
                            "id": record_id,
                            "owner": owner,
                            "error": error.to_string(),
                        })),
                    )
                    .await;
                return Err(map_storage_error("apply registry migration", error));
            }
            let completed = self
                .handle()
                .query_json(
                    "UPDATE type::record($table, $id) SET status = 'completed', completed_at = time::now(), owner = NONE, lease_expires_at = NONE, error = NONE WHERE status = 'applying' AND owner = $owner RETURN AFTER",
                    Some(json!({"table": "migration_ledger", "id": record_id, "owner": owner})),
                )
                .await
                .map_err(|err| map_storage_error("complete registry migration", err))?;
            if completed.is_empty() {
                return Err(MemoryError::Conflict(format!(
                    "registry migration {file_name} lease was lost before completion"
                )));
            }
            verified.push(file_name);
        }
        self.verify_registry_schema().await?;
        Ok(verified)
    }

    async fn verify_registry_schema(&self) -> Result<(), MemoryError> {
        const REQUIRED: &[(&str, &[&str], &[&str])] = &[
            (
                "account",
                &[
                    "id",
                    "status",
                    "tenant_id",
                    "created_at",
                    "deletion_challenge_id",
                    "deletion_started_at",
                    "deletion_completed_at",
                ],
                &["idx_account_tenant"],
            ),
            (
                "tenant",
                &[
                    "id",
                    "status",
                    "plan_version",
                    "schema_version",
                    "retry_stage",
                    "provisioning_lease",
                    "provisioning_lease.owner_id",
                    "provisioning_lease.lease_id",
                    "provisioning_lease.expires_at",
                    "provisioning_lease.fencing_generation",
                    "provisioning_lease.heartbeat_at",
                    "version",
                    "created_at",
                    "namespace_binding",
                    "namespace_binding.namespace",
                    "namespace_binding.database",
                    "deletion_started_at",
                    "deletion_completed_at",
                ],
                &["idx_tenant_namespace_binding"],
            ),
            (
                "api_key",
                &[
                    "id",
                    "account_id",
                    "name",
                    "verifier",
                    "status",
                    "created_at",
                    "expires_at",
                    "last_used_at",
                    "version",
                ],
                &[],
            ),
            (
                "external_identity",
                &[
                    "id",
                    "issuer",
                    "subject_verifier",
                    "account_id",
                    "created_at",
                ],
                &["idx_external_identity_issuer_subject"],
            ),
            (
                "provisioning_event",
                &["tenant_id", "stage", "created_at"],
                &[],
            ),
            (
                "plan",
                &[
                    "id",
                    "version",
                    "limits",
                    "limits.max_ingested_bytes",
                    "limits.max_episode_count",
                    "limits.ingest_per_minute",
                    "limits.max_open_app_sessions",
                    "limits.max_active_api_keys",
                    "limits.per_tenant_request_concurrency",
                    "limits.extraction_concurrency",
                ],
                &["idx_plan_version"],
            ),
            (
                "deletion_challenge",
                &[
                    "id",
                    "verifier",
                    "account_id",
                    "session_id",
                    "expires_at",
                    "consumed_at",
                    "created_at",
                ],
                &["idx_deletion_challenge_verifier"],
            ),
            (
                "usage",
                &[
                    "tenant_id",
                    "ingested_bytes",
                    "episode_count",
                    "open_app_sessions",
                    "active_api_keys",
                    "ingest_window_start",
                    "ingest_current_minute",
                    "updated_at",
                ],
                &["idx_usage_tenant"],
            ),
            (
                "control_plane_session",
                &[
                    "id",
                    "cookie_hash",
                    "account_id",
                    "auth_time",
                    "idle_expiry",
                    "absolute_expiry",
                ],
                &[
                    "idx_control_plane_session_cookie",
                    "idx_control_plane_session_account",
                ],
            ),
            (
                "oidc_request",
                &[
                    "state_hash",
                    "sealed_payload",
                    "aead_nonce",
                    "expires_at",
                    "created_at",
                ],
                &["idx_oidc_request_state"],
            ),
            (
                "audit_event",
                &[
                    "account_id",
                    "actor_kind",
                    "actor_principal",
                    "action",
                    "occurred_at",
                    "correlation_id",
                ],
                &[
                    "idx_audit_event_account",
                    "idx_audit_event_action_correlation",
                ],
            ),
            (
                "migration_ledger",
                &[
                    "file_name",
                    "checksum",
                    "status",
                    "started_at",
                    "completed_at",
                    "error",
                    "owner",
                    "lease_expires_at",
                ],
                &["idx_migration_ledger_file"],
            ),
        ];

        for (table, required_fields, required_indexes) in REQUIRED {
            let rows = self
                .handle()
                .query_json(&format!("INFO FOR TABLE {table}"), None)
                .await
                .map_err(|err| map_storage_error("verify registry schema", err))?;
            let info = rows.into_iter().next().ok_or_else(|| {
                MemoryError::Storage(format!("registry schema has no info for table {table}"))
            })?;
            let fields = info
                .get("fields")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    MemoryError::Storage(format!(
                        "registry schema has no field metadata for {table}"
                    ))
                })?;
            for field in *required_fields {
                if !fields.contains_key(*field) {
                    return Err(MemoryError::Storage(format!(
                        "registry schema is missing field {field} on table {table}"
                    )));
                }
            }
            let indexes = info
                .get("indexes")
                .and_then(Value::as_object)
                .ok_or_else(|| {
                    MemoryError::Storage(format!(
                        "registry schema has no index metadata for {table}"
                    ))
                })?;
            for index in *required_indexes {
                if !indexes.contains_key(*index) {
                    return Err(MemoryError::Storage(format!(
                        "registry schema is missing index {index} on table {table}"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl RegistryStore for SurrealRegistryStore {
    async fn ping(&self) -> bool {
        self.handle().ping().await
    }

    async fn find_account_by_id(&self, account_id: &str) -> Result<Option<Account>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT id, status, tenant_id, created_at FROM type::table($table) WHERE id = type::record($table, $id) LIMIT 1",
                Some(json!({"table": "account", "id": account_id})),
            )
            .await
            .map_err(|err| map_storage_error("find_account_by_id", err))?;
        rows.into_iter()
            .next()
            .map(|row| decode_account(&row))
            .transpose()
    }

    async fn find_account_by_identity(
        &self,
        issuer: &str,
        subject_verifier: &SubjectVerifier,
    ) -> Result<Option<Account>, MemoryError> {
        let verifier_hex = hex::encode(subject_verifier.0);
        let rows = self
            .handle()
            .query_json(
                "SELECT account_id FROM type::table($table) WHERE issuer = $issuer AND subject_verifier = $verifier LIMIT 1",
                Some(json!({"table": "external_identity", "issuer": issuer, "verifier": verifier_hex})),
            )
            .await
            .map_err(|err| map_storage_error("find_account_by_identity", err))?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let Some(account_id) = row.get("account_id").and_then(|v| v.as_str()) else {
            return Ok(None);
        };
        self.find_account_by_id(account_id).await
    }

    async fn create_account_bundle(
        &self,
        account: &Account,
        tenant: &Tenant,
        identity: Option<&ExternalIdentity>,
    ) -> Result<(), MemoryError> {
        if account.tenant_id != tenant.id {
            return Err(MemoryError::Validation(
                "account and tenant relationship does not match".into(),
            ));
        }
        if let Some(identity) = identity
            && identity.account_id != account.id
        {
            return Err(MemoryError::Validation(
                "identity.account_id must equal account.id".into(),
            ));
        }
        let tenant_status = serde_json::to_value(tenant.status)
            .map_err(|error| MemoryError::Storage(format!("encode tenant status: {error}")))?;
        let account_status = serde_json::to_value(account.status)
            .map_err(|error| MemoryError::Storage(format!("encode account status: {error}")))?;
        let retry_stage_assignment = if tenant.retry_stage.is_some() {
            "retry_stage = $retry_stage"
        } else {
            "retry_stage = NONE"
        };
        let lease_assignment = if tenant.provisioning_lease.is_some() {
            "provisioning_lease = $lease"
        } else {
            "provisioning_lease = NONE"
        };
        let mut script = format!(
            "BEGIN TRANSACTION; LET $existing_namespace = SELECT VALUE id FROM tenant WHERE namespace_binding.namespace = $namespace LIMIT 1; IF array::len($existing_namespace) > 0 {{ THROW 'namespace binding already exists'; }}; CREATE type::record('account', $account_id) SET id = $account_id, status = $account_status, tenant_id = $tenant_id, created_at = type::datetime($account_created_at); CREATE type::record('tenant', $tenant_record_id) SET id = $tenant_record_id, status = $tenant_status, namespace_binding = $binding, plan_version = $plan_version, schema_version = $schema_version, {retry_stage_assignment}, {lease_assignment}, created_at = type::datetime($tenant_created_at), version = $version;",
        );
        if identity.is_some() {
            script.push_str(" CREATE type::record('external_identity', $identity_id) SET id = $identity_id, issuer = $issuer, subject_verifier = $subject_verifier, account_id = $identity_account_id, created_at = type::datetime($identity_created_at);");
        }
        script.push_str(" COMMIT TRANSACTION;");
        let mut vars = json!({
            "account_id": account.id,
            "account_status": account_status,
            "tenant_id": account.tenant_id,
            "tenant_record_id": tenant.id,
            "tenant_status": tenant_status,
            "namespace": tenant.namespace_binding.namespace,
            "binding": tenant.namespace_binding,
            "plan_version": tenant.plan_version,
            "schema_version": tenant.schema_version,
            "retry_stage": tenant.retry_stage,
            "lease": tenant.provisioning_lease,
            "account_created_at": account.created_at.to_rfc3339(),
            "tenant_created_at": tenant.created_at.to_rfc3339(),
            "version": tenant.version,
        });
        if let Some(identity) = identity {
            let Some(object) = vars.as_object_mut() else {
                return Err(MemoryError::Storage(
                    "account bundle query variables are not an object".into(),
                ));
            };
            object.insert("identity_id".into(), json!(identity.id));
            object.insert("issuer".into(), json!(identity.issuer));
            object.insert(
                "subject_verifier".into(),
                json!(hex::encode(identity.subject_verifier.0)),
            );
            object.insert("identity_account_id".into(), json!(identity.account_id));
            object.insert(
                "identity_created_at".into(),
                json!(identity.created_at.to_rfc3339()),
            );
        }
        self.handle()
            .query_json(&script, Some(vars))
            .await
            .map_err(|error| map_storage_error("create account bundle", error))?;
        Ok(())
    }

    async fn find_external_identities(
        &self,
        account_id: &str,
    ) -> Result<Vec<ExternalIdentity>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT * FROM type::table($table) WHERE account_id = $account_id ORDER BY created_at",
                Some(json!({"table": "external_identity", "account_id": account_id})),
            )
            .await
            .map_err(|error| map_storage_error("find external identities", error))?;
        rows.iter().map(decode_identity).collect()
    }

    async fn link_external_identity(&self, identity: &ExternalIdentity) -> Result<(), MemoryError> {
        if self
            .find_account_by_id(&identity.account_id)
            .await?
            .is_none()
        {
            return Err(MemoryError::NotFound(format!(
                "account {}",
                identity.account_id
            )));
        }
        self.handle()
            .query_json(
                "CREATE type::record($table, $id) SET id = $id, issuer = $issuer, subject_verifier = $verifier, account_id = $account_id, created_at = type::datetime($created_at)",
                Some(json!({
                    "table": "external_identity",
                    "id": identity.id,
                    "issuer": identity.issuer,
                    "verifier": hex::encode(identity.subject_verifier.0),
                    "account_id": identity.account_id,
                    "created_at": identity.created_at.to_rfc3339(),
                })),
            )
            .await
            .map_err(|error| map_storage_error("link external identity", error))?;
        Ok(())
    }

    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
    ) -> Result<(), MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "DELETE type::record($table, $id) WHERE account_id = $account_id RETURN BEFORE",
                Some(json!({"table": "external_identity", "id": identity_id, "account_id": account_id})),
            )
            .await
            .map_err(|error| map_storage_error("unlink external identity", error))?;
        if rows.is_empty() {
            return Err(MemoryError::NotFound("external identity not found".into()));
        }
        Ok(())
    }

    async fn find_tenant_by_account(
        &self,
        account_id: &str,
    ) -> Result<Option<Tenant>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT tenant_id FROM type::table($table) WHERE id = type::record($table, $id) LIMIT 1",
                Some(json!({"table": "account", "id": account_id})),
            )
            .await
            .map_err(|err| map_storage_error("find_tenant_by_account", err))?;
        let tenant_id = rows.into_iter().next().and_then(|v| {
            v.get("tenant_id")
                .and_then(|t| t.as_str().map(|s| s.to_string()))
        });
        let Some(tenant_id) = tenant_id else {
            return Ok(None);
        };
        self.find_tenant_by_id(&tenant_id).await
    }

    async fn find_tenant_by_id(&self, tenant_id: &str) -> Result<Option<Tenant>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT * FROM type::table($table) WHERE id = type::record($table, $id) LIMIT 1",
                Some(json!({"table": "tenant", "id": tenant_id})),
            )
            .await
            .map_err(|err| map_storage_error("find_tenant_by_id", err))?;
        rows.into_iter()
            .next()
            .map(|row| decode_tenant(&row))
            .transpose()
    }

    async fn find_api_key(&self, key_id: &str) -> Result<Option<ApiKey>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT * FROM type::table($table) WHERE id = type::record($table, $id) LIMIT 1",
                Some(json!({"table": "api_key", "id": key_id})),
            )
            .await
            .map_err(|err| map_storage_error("find_api_key", err))?;
        rows.into_iter()
            .next()
            .map(|row| decode_api_key(&row))
            .transpose()
    }

    async fn write_api_key(&self, key: &ApiKey) -> Result<(), MemoryError> {
        let expires_assignment = if key.expires_at.is_some() {
            "expires_at = type::datetime($expires_at)"
        } else {
            "expires_at = NONE"
        };
        let used_assignment = if key.last_used_at.is_some() {
            "last_used_at = type::datetime($last_used_at)"
        } else {
            "last_used_at = NONE"
        };
        let sql = format!(
            "CREATE type::record($table, $id) SET id = $id, account_id = $account_id, name = $name, verifier = $verifier, status = $status, created_at = type::datetime($created_at), {expires_assignment}, {used_assignment}, version = $version"
        );
        self.handle()
            .query_json(
                &sql,
                Some(json!({
                    "table": "api_key",
                    "id": key.id,
                    "account_id": key.account_id,
                    "name": key.name,
                    "verifier": hex::encode(key.verifier.0),
                    "status": serde_json::to_value(key.status).map_err(|error| MemoryError::Storage(format!("encode key status: {error}")))?,
                    "created_at": key.created_at.to_rfc3339(),
                    "expires_at": key.expires_at.map(|value| value.to_rfc3339()),
                    "last_used_at": key.last_used_at.map(|value| value.to_rfc3339()),
                    "version": key.version,
                })),
            )
            .await
            .map_err(|err| map_storage_error("write_api_key", err))?;
        Ok(())
    }

    async fn list_api_keys(&self, account_id: &str) -> Result<Vec<ApiKeyMeta>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT id, name, status, created_at, expires_at, last_used_at \
                 FROM type::table($table) WHERE account_id = $account_id",
                Some(json!({"table": "api_key", "account_id": account_id})),
            )
            .await
            .map_err(|err| map_storage_error("list_api_keys", err))?;
        rows.into_iter()
            .map(|row| {
                let key = decode_api_key(&row)?;
                Ok(ApiKeyMeta {
                    id: key.id,
                    name: key.name,
                    status: key.status,
                    created_at: key.created_at,
                    expires_at: key.expires_at,
                    last_used_at: key.last_used_at,
                })
            })
            .collect()
    }

    async fn revoke_api_key(&self, account_id: &str, key_id: &str) -> Result<(), MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::record($table, $id) SET status = 'revoked', version = version + 1 WHERE account_id = $account_id AND status = 'active' RETURN AFTER",
                Some(json!({"table": "api_key", "id": key_id, "account_id": account_id})),
            )
            .await
            .map_err(|err| map_storage_error("revoke_api_key", err))?;
        if rows.is_empty() {
            return Err(MemoryError::NotFound("api key not found".into()));
        }
        Ok(())
    }

    async fn create_api_key_if_below_limit(
        &self,
        key: &ApiKey,
        max_active: u32,
    ) -> Result<(), MemoryError> {
        if max_active == 0 {
            return Err(MemoryError::Conflict("active API key limit is zero".into()));
        }
        let expires_assignment = if key.expires_at.is_some() {
            "expires_at = type::datetime($expires_at)"
        } else {
            "expires_at = NONE"
        };
        let used_assignment = if key.last_used_at.is_some() {
            "last_used_at = type::datetime($last_used_at)"
        } else {
            "last_used_at = NONE"
        };
        let status = serde_json::to_value(key.status)
            .map_err(|error| MemoryError::Storage(format!("encode key status: {error}")))?;
        let sql = format!(
            "BEGIN TRANSACTION; LET $active = SELECT count() AS count FROM api_key WHERE account_id = $account_id AND status = 'active' AND (expires_at IS NONE OR expires_at > time::now()) GROUP ALL; IF array::len($active) > 0 AND $active[0].count >= $max_active {{ THROW 'active API key limit reached'; }}; CREATE type::record('api_key', $id) SET id = $id, account_id = $account_id, name = $name, verifier = $verifier, status = $status, created_at = type::datetime($created_at), {expires_assignment}, {used_assignment}, version = $version; COMMIT TRANSACTION;"
        );
        self.handle()
            .query_json(
                &sql,
                Some(json!({
                    "id": key.id,
                    "account_id": key.account_id,
                    "name": key.name,
                    "verifier": hex::encode(key.verifier.0),
                    "status": status,
                    "created_at": key.created_at.to_rfc3339(),
                    "expires_at": key.expires_at.map(|value| value.to_rfc3339()),
                    "last_used_at": key.last_used_at.map(|value| value.to_rfc3339()),
                    "version": key.version,
                    "max_active": max_active,
                })),
            )
            .await
            .map_err(|err| match err {
                MemoryError::Storage(message)
                    if message.contains("active API key limit reached") =>
                {
                    MemoryError::Conflict("active API key limit reached".into())
                }
                error => map_storage_error("create API key", error),
            })?;
        Ok(())
    }

    async fn revoke_all_api_keys(&self, account_id: &str) -> Result<u64, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::table($table) SET status = 'revoked', version = version + 1 WHERE account_id = $account_id AND status = 'active' RETURN BEFORE",
                Some(json!({"table": "api_key", "account_id": account_id})),
            )
            .await
            .map_err(|err| map_storage_error("revoke all api keys", err))?;
        Ok(rows.len() as u64)
    }

    async fn touch_api_key(&self, key_id: &str, used_at: DateTime<Utc>) -> Result<(), MemoryError> {
        self.handle()
            .query_json(
                "UPDATE type::record($table, $id) SET last_used_at = type::datetime($used_at) WHERE status = 'active' AND (expires_at IS NONE OR expires_at > type::datetime($used_at))",
                Some(json!({"table": "api_key", "id": key_id, "used_at": used_at.to_rfc3339()})),
            )
            .await
            .map_err(|err| map_storage_error("touch_api_key", err))?;
        Ok(())
    }

    async fn write_account(&self, account: &Account) -> Result<(), MemoryError> {
        let status = serde_json::to_value(account.status)
            .map_err(|error| MemoryError::Storage(format!("encode account status: {error}")))?;
        self.handle()
            .query_json(
                "UPSERT type::record($table, $id) SET id = $id, status = IF status = 'deleting' AND $status != 'deleting' THEN status ELSE $status END, tenant_id = $tenant_id, created_at = type::datetime($created_at)",
                Some(json!({
                    "table": "account",
                    "id": account.id,
                    "status": status,
                    "tenant_id": account.tenant_id,
                    "created_at": account.created_at.to_rfc3339(),
                })),
            )
            .await
            .map_err(|err| map_storage_error("write_account", err))?;
        Ok(())
    }

    async fn transition_account_state(
        &self,
        account_id: &str,
        from: AccountStatus,
        to: AccountStatus,
    ) -> Result<(), MemoryError> {
        let from = serde_json::to_value(from)
            .map_err(|error| MemoryError::Storage(format!("encode account state: {error}")))?;
        let to = serde_json::to_value(to)
            .map_err(|error| MemoryError::Storage(format!("encode account state: {error}")))?;
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::record($table, $id) SET status = $to WHERE status = $from AND NOT (status = 'deleting' AND $to != 'deleting') RETURN AFTER",
                Some(json!({"table": "account", "id": account_id, "from": from, "to": to})),
            )
            .await
            .map_err(|err| map_storage_error("transition account state", err))?;
        if rows.is_empty() {
            return Err(MemoryError::Conflict("account state CAS failed".into()));
        }
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let script = "BEGIN TRANSACTION; \
            LET $challenge = SELECT * FROM deletion_challenge \
                WHERE verifier = $verifier AND account_id = $account_id \
                AND session_id = $session_id AND consumed_at IS NONE \
                AND expires_at > type::datetime($now) LIMIT 1; \
            IF array::len($challenge) = 0 { THROW 'deletion challenge is invalid or expired'; }; \
            LET $account = UPDATE type::record('account', $account_id) \
                SET status = 'deleting', \
                    deletion_challenge_id = <string> record::id($challenge[0].id), \
                    deletion_started_at = type::datetime($now) \
                WHERE status = 'active' RETURN AFTER; \
            IF array::len($account) = 0 { THROW 'account is not active'; }; \
            LET $tenant = UPDATE type::record('tenant', $account[0].tenant_id) \
                SET status = 'deleting', \
                    deletion_started_at = type::datetime($now), \
                    provisioning_lease = NONE, \
                    version = version + 1 \
                WHERE status IN ['reserved', 'namespace_creating', 'migrating', 'ready', 'suspended', 'failed', 'deleting'] \
                RETURN AFTER; \
            IF array::len($tenant) = 0 { THROW 'tenant deletion tombstone is already purged or missing'; }; \
            UPDATE api_key SET status = 'revoked', version = version + 1 \
                WHERE account_id = $account_id AND status = 'active'; \
            DELETE FROM control_plane_session WHERE account_id = $account_id; \
            LET $consumed = UPDATE deletion_challenge \
                SET consumed_at = type::datetime($now) \
                WHERE verifier = $verifier AND account_id = $account_id \
                AND session_id = $session_id AND consumed_at IS NONE \
                AND expires_at > type::datetime($now) RETURN AFTER; \
            IF array::len($consumed) = 0 { THROW 'deletion challenge was consumed concurrently'; }; \
            CREATE type::record('audit_event', $audit_id) SET \
                account_id = $account_id, actor_kind = 'account', \
                actor_principal = $account_id, action = 'account_deletion_started', \
                occurred_at = type::datetime($now), correlation_id = $audit_id; \
            COMMIT TRANSACTION;";
        let result = self
            .handle()
            .query_json(
                script,
                Some(json!({
                    "verifier": verifier,
                    "account_id": account_id,
                    "session_id": session_id,
                    "now": now.to_rfc3339(),
                    "audit_id": format!("deletion_start_{account_id}"),
                })),
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) => Err(classify_deletion_error(error)),
        }
    }

    #[cfg(feature = "control-plane")]
    async fn begin_operator_deletion(
        &self,
        tenant_id: &str,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let audit_id = format!("operator_deletion_{tenant_id}");
        let script = "BEGIN TRANSACTION; \
            LET $tenant = SELECT * FROM tenant WHERE id = type::record('tenant', $tenant_id) LIMIT 1; \
            IF array::len($tenant) = 0 { THROW 'tenant not found'; }; \
            IF $tenant[0].status = 'purged' { THROW 'tenant is already purged'; }; \
            LET $account_record = (SELECT VALUE id FROM account WHERE tenant_id = $tenant_id LIMIT 1)[0]; \
            IF $account_record IS NONE { THROW 'account not found'; }; \
            LET $account_id = <string> record::id($account_record); \
            UPDATE account SET status = 'deleting', deletion_started_at = type::datetime($now) \
                WHERE tenant_id = $tenant_id AND status IN ['active', 'deleting']; \
            UPDATE tenant SET status = 'deleting', deletion_started_at = type::datetime($now), provisioning_lease = NONE, version = version + 1 \
                WHERE id = type::record('tenant', $tenant_id) AND status IN ['reserved', 'namespace_creating', 'migrating', 'ready', 'suspended', 'failed', 'deleting']; \
            UPDATE api_key SET status = 'revoked', version = version + 1 WHERE account_id = $account_id AND status = 'active'; \
            DELETE FROM control_plane_session WHERE account_id = $account_id; \
            IF count(SELECT * FROM audit_event WHERE correlation_id = $audit_id) = 0 { CREATE type::record('audit_event', $audit_id) SET account_id = $account_id, actor_kind = 'operator', actor_principal = $actor, action = 'account_deletion_started_operator', occurred_at = type::datetime($now), correlation_id = $audit_id; }; \
            COMMIT TRANSACTION;";
        self.handle()
            .query_json(
                script,
                Some(json!({
                    "tenant_id": tenant_id,
                    "actor": actor,
                    "now": now.to_rfc3339(),
                    "audit_id": audit_id,
                })),
            )
            .await
            .map_err(classify_deletion_error)?;
        Ok(())
    }

    async fn write_tenant(&self, tenant: &Tenant) -> Result<(), MemoryError> {
        let status = serde_json::to_value(tenant.status)
            .map_err(|error| MemoryError::Storage(format!("encode tenant status: {error}")))?;
        let retry_stage_assignment = if tenant.retry_stage.is_some() {
            "retry_stage = $retry_stage"
        } else {
            "retry_stage = NONE"
        };
        let lease_assignment = if tenant.provisioning_lease.is_some() {
            "provisioning_lease = $lease"
        } else {
            "provisioning_lease = NONE"
        };
        let sql = format!(
            "UPSERT type::record($table, $id) SET id = $id, status = IF status = 'purged' AND $status != 'purged' THEN status ELSE $status END, namespace_binding = IF namespace_binding IS NONE THEN $binding ELSE namespace_binding END, plan_version = $plan_version, schema_version = $schema_version, {retry_stage_assignment}, {lease_assignment}, created_at = type::datetime($created_at), version = $version"
        );
        self.handle()
            .query_json(
                &sql,
                Some(json!({
                    "table": "tenant",
                    "id": tenant.id,
                    "status": status,
                    "binding": tenant.namespace_binding,
                    "plan_version": tenant.plan_version,
                    "schema_version": tenant.schema_version,
                    "retry_stage": tenant.retry_stage,
                    "lease": tenant.provisioning_lease,
                    "created_at": tenant.created_at.to_rfc3339(),
                    "version": tenant.version,
                })),
            )
            .await
            .map_err(|err| map_storage_error("write_tenant", err))?;
        Ok(())
    }

    async fn update_tenant_state(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
    ) -> Result<u64, MemoryError> {
        let from_str = encoded_status(from, "tenant state")?;
        let to_str = encoded_status(to, "tenant state")?;
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::record($table, $id) SET status = $to, version = version + 1 \
                 WHERE version = $expected AND status = $from RETURN AFTER",
                Some(json!({
                    "table": "tenant",
                    "id": tenant_id,
                    "to": to_str,
                    "expected": expected_version,
                    "from": from_str,
                })),
            )
            .await
            .map_err(|err| map_storage_error("update_tenant_state", err))?;
        if rows.is_empty() {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} state CAS failed"
            )));
        }
        let new_version = returned_version(&rows, "update_tenant_state")?;
        Ok(new_version)
    }

    async fn update_tenant_state_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
        lease: &LeaseFence<'_>,
    ) -> Result<u64, MemoryError> {
        let from_str = encoded_status(from, "tenant state")?;
        let to_str = encoded_status(to, "tenant state")?;
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::record($table, $id) SET status = $to, version = version + 1 \
                 WHERE version = $expected AND status = $from \
                 AND provisioning_lease.owner_id = $owner \
                 AND provisioning_lease.lease_id = $lease \
                 AND provisioning_lease.fencing_generation = $gen \
                 AND provisioning_lease.expires_at > time::now() \
                 RETURN AFTER",
                Some(json!({
                    "table": "tenant",
                    "id": tenant_id,
                    "to": to_str,
                    "expected": expected_version,
                    "from": from_str,
                    "owner": lease.owner_id,
                    "lease": lease.lease_id,
                    "gen": lease.fencing_generation,
                })),
            )
            .await
            .map_err(|err| map_storage_error("update_tenant_state_fenced", err))?;
        if rows.is_empty() {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} fenced CAS failed"
            )));
        }
        let new_version = returned_version(&rows, "update_tenant_state_fenced")?;
        Ok(new_version)
    }

    async fn update_tenant_schema_version_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        new_schema_version: u32,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<u64, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::record($table, $id) SET schema_version = $new, version = version + 1 \
                 WHERE version = $expected \
                 AND provisioning_lease.owner_id = $owner \
                 AND provisioning_lease.lease_id = $lease \
                 AND provisioning_lease.fencing_generation = $gen \
                 AND provisioning_lease.expires_at > time::now() \
                 RETURN AFTER",
                Some(json!({
                    "table": "tenant",
                    "id": tenant_id,
                    "new": new_schema_version,
                    "expected": expected_version,
                    "owner": lease_owner_id,
                    "lease": lease_id,
                    "gen": fencing_generation,
                })),
            )
            .await
            .map_err(|err| map_storage_error("update_tenant_schema_version_fenced", err))?;
        if rows.is_empty() {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} schema-version fenced CAS failed"
            )));
        }
        let new_version = returned_version(&rows, "update_tenant_schema_version_fenced")?;
        Ok(new_version)
    }

    async fn claim_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        lease_ttl_secs: i64,
    ) -> Result<Option<crate::http::leases::ProvisioningLease>, MemoryError> {
        if lease_ttl_secs <= 0 {
            return Err(MemoryError::Validation(
                "provisioning lease TTL must be positive".into(),
            ));
        }
        let now_str = Utc::now().to_rfc3339();
        // Use a server-side function via $let, but Surreal's query
        // engine allows arithmetic on datetimes; we pass the
        // current time as ISO and let the engine compute expires.
        let expires_at = (Utc::now() + chrono::Duration::seconds(lease_ttl_secs)).to_rfc3339();
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::record($table, $id) SET \
                 provisioning_lease = { owner_id: $owner, lease_id: $lease, \
                 fencing_generation: (IF provisioning_lease IS NONE OR \
                 provisioning_lease.expires_at < time::now() THEN \
                 (IF provisioning_lease IS NONE THEN 1 ELSE provisioning_lease.fencing_generation + 1 END) \
                 ELSE provisioning_lease.fencing_generation END), \
                 expires_at: type::datetime($exp), heartbeat_at: type::datetime($now) }, \
                 version = version + 1 \
                 WHERE status IN ['reserved', 'namespace_creating', 'migrating', 'suspended', 'failed', 'deleting'] \
                 AND (provisioning_lease IS NONE OR provisioning_lease.expires_at <= time::now()) \
                 RETURN AFTER",
                Some(json!({
                    "table": "tenant",
                    "id": tenant_id,
                    "owner": owner_id,
                    "lease": lease_id,
                    "now": now_str,
                    "exp": expires_at,
                })),
            )
            .await
            .map_err(|err| map_storage_error("claim_provisioning", err))?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let lease_json = row
            .get("provisioning_lease")
            .cloned()
            .unwrap_or(Value::Null);
        let lease: crate::http::leases::ProvisioningLease = serde_json::from_value(lease_json)
            .map_err(|err| MemoryError::Storage(format!("decode lease: {err}")))?;
        Ok(Some(lease))
    }

    async fn release_provisioning_lease(
        &self,
        tenant_id: &str,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<(), MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::record($table, $id) SET provisioning_lease = NONE, version = version + 1 \
                 WHERE provisioning_lease.owner_id = $owner \
                 AND provisioning_lease.lease_id = $lease \
                 AND provisioning_lease.fencing_generation = $gen RETURN AFTER",
                Some(json!({
                    "table": "tenant",
                    "id": tenant_id,
                    "owner": lease_owner_id,
                    "lease": lease_id,
                    "gen": fencing_generation,
                })),
            )
            .await
            .map_err(|err| map_storage_error("release_provisioning_lease", err))?;
        if rows.is_empty() {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} release failed: lease mismatch"
            )));
        }
        Ok(())
    }

    async fn heartbeat_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        heartbeat_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::record($table, $id) SET \
                 provisioning_lease.heartbeat_at = $hb, \
                 provisioning_lease.expires_at = $exp, \
                 version = version + 1 \
                 WHERE provisioning_lease.owner_id = $owner \
                 AND provisioning_lease.lease_id = $lease \
                 AND provisioning_lease.fencing_generation = $gen \
                 AND provisioning_lease.expires_at > time::now() \
                 RETURN AFTER",
                Some(json!({
                    "table": "tenant",
                    "id": tenant_id,
                    "owner": owner_id,
                    "lease": lease_id,
                    "gen": fencing_generation,
                    "hb": heartbeat_at.to_rfc3339(),
                    "exp": expires_at.to_rfc3339(),
                })),
            )
            .await
            .map_err(|err| map_storage_error("heartbeat_provisioning", err))?;
        if rows.is_empty() {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} heartbeat failed: lease mismatch"
            )));
        }
        Ok(())
    }

    async fn list_due_provisioning(
        &self,
        limit: usize,
        _now: DateTime<Utc>,
    ) -> Result<Vec<Tenant>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT * FROM type::table($table) \
                 WHERE status IN ['reserved', 'migrating', 'suspended', 'failed'] \
                 AND (provisioning_lease IS NONE OR provisioning_lease.expires_at <= time::now()) \
                 LIMIT $limit",
                Some(json!({"table": "tenant", "limit": limit})),
            )
            .await
            .map_err(|err| map_storage_error("list_due_provisioning", err))?;
        rows.into_iter().map(|row| decode_tenant(&row)).collect()
    }

    async fn list_ready_tenants(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Vec<Tenant>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT * FROM tenant WHERE status = 'ready' AND ($cursor IS NONE OR id > $cursor) ORDER BY id LIMIT $limit",
                Some(json!({"cursor": cursor, "limit": limit})),
            )
            .await
            .map_err(|error| map_storage_error("list ready tenants", error))?;
        rows.into_iter().map(|row| decode_tenant(&row)).collect()
    }

    async fn list_deleting_tenants(
        &self,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<Vec<Tenant>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT * FROM tenant WHERE status = 'deleting' \
                 AND (provisioning_lease IS NONE OR provisioning_lease.expires_at <= type::datetime($now)) \
                 ORDER BY id LIMIT $limit",
                Some(json!({"limit": limit, "now": now.to_rfc3339()})),
            )
            .await
            .map_err(|error| map_storage_error("list deleting tenants", error))?;
        rows.into_iter().map(|row| decode_tenant(&row)).collect()
    }

    async fn list_tenants(&self, limit: usize) -> Result<Vec<Tenant>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT * FROM tenant ORDER BY id LIMIT $limit",
                Some(json!({"limit": limit})),
            )
            .await
            .map_err(|error| map_storage_error("list tenants", error))?;
        rows.into_iter().map(|row| decode_tenant(&row)).collect()
    }

    #[cfg(feature = "control-plane")]
    async fn finalize_account_deletion(
        &self,
        tenant_id: &str,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        completed_at: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let Some(tenant) = self.find_tenant_by_id(tenant_id).await? else {
            return Err(MemoryError::NotFound(format!("tenant {tenant_id}")));
        };
        if tenant.status == TenantStatus::Purged {
            return Ok(());
        }
        if tenant.status != TenantStatus::Deleting {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} is not deleting"
            )));
        }
        let script = "BEGIN TRANSACTION; \
            LET $tenant = UPDATE type::record('tenant', $tenant_id) \
                SET status = 'purged', \
                    deletion_completed_at = type::datetime($completed_at), \
                    provisioning_lease = NONE, \
                    version = version + 1 \
                WHERE status = 'deleting' \
                AND provisioning_lease.owner_id = $owner_id \
                AND provisioning_lease.lease_id = $lease_id \
                AND provisioning_lease.fencing_generation = $fencing_generation \
                AND provisioning_lease.expires_at > time::now() RETURN AFTER; \
            IF array::len($tenant) = 0 { THROW 'deletion lease is stale or tenant is no longer deleting'; }; \
            LET $account = UPDATE account SET deletion_completed_at = type::datetime($completed_at) \
                WHERE tenant_id = $tenant_id AND status = 'deleting' RETURN AFTER; \
            IF array::len($account) = 0 { THROW 'deleting account tombstone is missing'; }; \
            CREATE type::record('audit_event', $audit_id) SET \
                account_id = <string> record::id($account[0].id), actor_kind = 'system', \
                actor_principal = $owner_id, action = 'account_deletion_completed', \
                occurred_at = type::datetime($completed_at), correlation_id = $audit_id; \
            COMMIT TRANSACTION;";
        let result = self
            .handle()
            .query_json(
                script,
                Some(json!({
                    "tenant_id": tenant_id,
                    "owner_id": lease_owner_id,
                    "lease_id": lease_id,
                    "fencing_generation": fencing_generation,
                    "completed_at": completed_at.to_rfc3339(),
                    "audit_id": format!("deletion_complete_{tenant_id}"),
                })),
            )
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) => {
                let classified = classify_deletion_error(error);
                if self
                    .find_tenant_by_id(tenant_id)
                    .await?
                    .is_some_and(|current| current.status == TenantStatus::Purged)
                {
                    Ok(())
                } else {
                    Err(classified)
                }
            }
        }
    }

    async fn load_plan(&self, version: u32) -> Result<super::models::Plan, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT * FROM plan WHERE version = $version LIMIT 1",
                Some(json!({"version": version})),
            )
            .await
            .map_err(|error| map_storage_error("load plan", error))?;
        let Some(row) = rows.into_iter().next() else {
            return Err(MemoryError::NotFound(format!(
                "registry plan version {version} is not provisioned"
            )));
        };
        let id = row
            .get("id")
            .and_then(record_id_value)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| MemoryError::Storage("plan row has no valid id".into()))?;
        let stored_version = required_u32(&row, "version")?;
        if stored_version != version {
            return Err(MemoryError::Storage(format!(
                "plan row version {stored_version} does not match requested version {version}"
            )));
        }
        let limits_value = row
            .get("limits")
            .cloned()
            .ok_or_else(|| MemoryError::Storage("plan row has no limits".into()))?;
        let limits = serde_json::from_value(limits_value)
            .map_err(|error| MemoryError::Storage(format!("decode plan limits: {error}")))?;
        Ok(super::models::Plan {
            id,
            version: stored_version,
            limits,
        })
    }

    async fn ensure_plan(&self, plan: &super::models::Plan) -> Result<(), MemoryError> {
        self.handle()
            .query_json(
                "IF count(SELECT * FROM plan WHERE version = $version) = 0 { CREATE type::record($table, $id) SET id = $id, version = $version, limits = $limits; }",
                Some(json!({
                    "table": "plan",
                    "id": plan.id,
                    "version": plan.version,
                    "limits": plan.limits,
                })),
            )
            .await
            .map_err(|error| map_storage_error("ensure plan", error))?;
        Ok(())
    }

    async fn load_usage(
        &self,
        tenant_id: &str,
    ) -> Result<crate::http::registry::plan::UsageCounter, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT ingest_window_start, ingest_current_minute, ingested_bytes, episode_count FROM usage WHERE tenant_id = $tenant_id LIMIT 1",
                Some(json!({"tenant_id": tenant_id})),
            )
            .await
            .map_err(|error| map_storage_error("load usage", error))?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(crate::http::registry::plan::UsageCounter::default());
        };
        Ok(crate::http::registry::plan::UsageCounter {
            ingest_current_minute: required_u32(&row, "ingest_current_minute")?,
            window_start: required_datetime(&row, "ingest_window_start")?,
            ingested_bytes: required_u64(&row, "ingested_bytes")?,
            episode_count: required_u64(&row, "episode_count")?,
        })
    }

    async fn reserve_ingest_usage(
        &self,
        tenant_id: &str,
        source_bytes: u64,
        plan: &crate::http::registry::plan::Plan,
        now: DateTime<Utc>,
    ) -> Result<crate::http::registry::plan::QuotaDecision, MemoryError> {
        for _attempt in 0..3 {
            // Create the usage row once. The conditional UPDATE below is the
            // admission operation; it increments counters only when every
            // quota predicate still holds at the datastore write point.
            self.handle()
                .query_json(
                    "UPSERT type::record($table, $tenant_id) SET tenant_id = $tenant_id, ingest_window_start = IF ingest_window_start IS NONE THEN type::datetime($now) ELSE ingest_window_start END, ingest_current_minute = IF ingest_current_minute IS NONE THEN 0 ELSE ingest_current_minute END, ingested_bytes = IF ingested_bytes IS NONE THEN 0 ELSE ingested_bytes END, episode_count = IF episode_count IS NONE THEN 0 ELSE episode_count END, updated_at = time::now()",
                    Some(json!({"table": "usage", "tenant_id": tenant_id, "now": now.to_rfc3339()})),
                )
                .await
                .map_err(|error| map_storage_error("initialize ingest usage", error))?;
            let rows = self
                .handle()
                .query_json(
                    "UPDATE type::record($table, $tenant_id) SET ingest_window_start = IF ingest_window_start <= type::datetime($cutoff) THEN type::datetime($now) ELSE ingest_window_start END, ingest_current_minute = IF ingest_window_start <= type::datetime($cutoff) THEN 1 ELSE ingest_current_minute + 1 END, ingested_bytes = ingested_bytes + $bytes, episode_count = episode_count + 1, updated_at = time::now() WHERE tenant_id = $tenant_id AND ingested_bytes + $bytes <= $max_bytes AND episode_count < $max_episodes AND (ingest_current_minute < $per_minute OR ingest_window_start <= type::datetime($cutoff)) RETURN AFTER",
                    Some(json!({
                        "table": "usage",
                        "tenant_id": tenant_id,
                        "now": now.to_rfc3339(),
                        "cutoff": (now - chrono::Duration::seconds(60)).to_rfc3339(),
                        "bytes": source_bytes,
                        "max_bytes": plan.max_ingested_bytes,
                        "max_episodes": plan.max_episode_count,
                        "per_minute": plan.ingest_per_minute,
                    })),
                )
                .await
                .map_err(|error| map_storage_error("reserve ingest usage", error))?;
            if !rows.is_empty() {
                return Ok(crate::http::registry::plan::QuotaDecision::Allow);
            }
            let current = self.load_usage(tenant_id).await?;
            let mut probe = current;
            let decision =
                crate::http::registry::plan::enforce_ingest(plan, &mut probe, source_bytes, now);
            if decision.is_deny() {
                return Ok(decision);
            }
        }
        Err(MemoryError::Unavailable(
            "ingest quota update contention; retry the request".into(),
        ))
    }

    async fn reconcile_usage(
        &self,
        tenant_id: &str,
        expected: crate::http::registry::plan::UsageCounter,
    ) -> Result<(), MemoryError> {
        self.handle()
            .query_json(
                "UPSERT type::record($table, $tenant_id) SET tenant_id = $tenant_id, ingest_window_start = type::datetime($window), ingest_current_minute = $count, ingested_bytes = $bytes, episode_count = $episodes, updated_at = time::now()",
                Some(json!({"table": "usage", "tenant_id": tenant_id, "window": expected.window_start.to_rfc3339(), "count": expected.ingest_current_minute, "bytes": expected.ingested_bytes, "episodes": expected.episode_count})),
            )
            .await
            .map_err(|error| map_storage_error("reconcile usage", error))?;
        Ok(())
    }

    async fn append_provisioning_event(
        &self,
        tenant_id: &str,
        stage: &str,
    ) -> Result<(), MemoryError> {
        self.handle()
            .query_json(
                "CREATE provisioning_event SET tenant_id = $tenant_id, stage = $stage",
                Some(json!({"tenant_id": tenant_id, "stage": stage})),
            )
            .await
            .map_err(|err| map_storage_error("append_provisioning_event", err))?;
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn store_oidc_request(
        &self,
        state_hash: &str,
        sealed_payload: &[u8],
        aead_nonce: &[u8; 12],
    ) -> Result<(), MemoryError> {
        let payload_b64 = base64_encode(sealed_payload);
        let nonce_arr: Vec<u8> = aead_nonce.to_vec();
        self.handle()
            .query_json(
                "CREATE type::record($table, $id) SET state_hash = $state, sealed_payload = $payload, aead_nonce = $nonce, expires_at = type::datetime($expires_at), created_at = time::now()",
                Some(json!({
                    "table": "oidc_request",
                    "id": state_hash,
                    "state": state_hash,
                    "payload": payload_b64,
                    "nonce": nonce_arr,
                    "expires_at": (Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
                })),
            )
            .await
            .map_err(|err| map_storage_error("store_oidc_request", err))?;
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn take_oidc_request(
        &self,
        state_hash: &str,
    ) -> Result<Option<(Vec<u8>, [u8; 12])>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "DELETE type::record($table, $id) WHERE state_hash = $state AND expires_at > time::now() RETURN BEFORE",
                Some(json!({"table": "oidc_request", "id": state_hash, "state": state_hash})),
            )
            .await
            .map_err(|err| map_storage_error("take_oidc_request", err))?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let payload_b64 = row
            .get("sealed_payload")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let nonce_arr = row.get("aead_nonce").and_then(|v| v.as_array()).cloned();
        let payload = payload_b64
            .ok_or_else(|| MemoryError::Storage("oidc_request payload missing".into()))?;
        let nonce =
            nonce_arr.ok_or_else(|| MemoryError::Storage("oidc_request nonce missing".into()))?;
        let nonce_bytes = nonce
            .iter()
            .map(|value| value.as_u64().and_then(|number| u8::try_from(number).ok()))
            .collect::<Option<Vec<_>>>()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| MemoryError::Storage("oidc_request nonce is invalid".into()))?;
        let payload = base64_decode(&payload).map_err(|error| {
            MemoryError::Storage(format!("oidc_request payload is invalid: {error}"))
        })?;
        Ok(Some((payload, nonce_bytes)))
    }

    #[cfg(feature = "control-plane")]
    async fn store_session(
        &self,
        session: &crate::control::session::ControlPlaneSession,
    ) -> Result<(), MemoryError> {
        self.handle()
            .query_json(
                "CREATE type::record($table, $id) SET id = $id, cookie_hash = $cookie_hash, account_id = $account_id, auth_time = type::datetime($auth_time), idle_expiry = type::datetime($idle_expiry), absolute_expiry = type::datetime($absolute_expiry)",
                Some(json!({
                    "table": "control_plane_session",
                    "id": session.id,
                    "cookie_hash": session.cookie_hash,
                    "account_id": session.account_id,
                    "auth_time": session.auth_time.to_rfc3339(),
                    "idle_expiry": session.idle_expiry.to_rfc3339(),
                    "absolute_expiry": session.absolute_expiry.to_rfc3339(),
                })),
            )
            .await
            .map_err(|err| map_storage_error("store_session", err))?;
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn find_session(
        &self,
        cookie_hash: &str,
    ) -> Result<Option<crate::control::session::ControlPlaneSession>, MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "SELECT * FROM type::table($table) WHERE cookie_hash = $cookie AND idle_expiry > time::now() AND absolute_expiry > time::now() LIMIT 1",
                Some(json!({"table": "control_plane_session", "cookie": cookie_hash})),
            )
            .await
            .map_err(|err| map_storage_error("find_session", err))?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let cookie_hash = row
            .get("cookie_hash")
            .and_then(Value::as_str)
            .ok_or_else(|| MemoryError::Storage("session cookie hash missing".into()))?;
        let session = crate::control::session::ControlPlaneSession {
            id: row_id(&row, ""),
            cookie_hash: cookie_hash.to_owned(),
            account_id: row
                .get("account_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            auth_time: required_datetime(&row, "auth_time")?,
            idle_expiry: required_datetime(&row, "idle_expiry")?,
            absolute_expiry: required_datetime(&row, "absolute_expiry")?,
        };
        if session.idle_expiry <= Utc::now() || session.absolute_expiry <= Utc::now() {
            return Ok(None);
        }
        Ok(Some(session))
    }

    #[cfg(feature = "control-plane")]
    async fn touch_session(
        &self,
        session_id: &str,
        idle_expiry: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::record($table, $id) SET idle_expiry = IF type::datetime($idle_expiry) < absolute_expiry THEN type::datetime($idle_expiry) ELSE absolute_expiry END WHERE absolute_expiry > time::now() RETURN AFTER",
                Some(json!({"table": "control_plane_session", "id": session_id, "idle_expiry": idle_expiry.to_rfc3339()})),
            )
            .await
            .map_err(|error| map_storage_error("touch session", error))?;
        if rows.is_empty() {
            return Err(MemoryError::NotFound("session not found".into()));
        }
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn delete_session(&self, cookie_hash: &str) -> Result<(), MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "DELETE FROM control_plane_session WHERE cookie_hash = $cookie_hash RETURN BEFORE",
                Some(json!({"cookie_hash": cookie_hash})),
            )
            .await
            .map_err(|error| map_storage_error("delete session", error))?;
        if rows.is_empty() {
            return Err(MemoryError::NotFound("session not found".into()));
        }
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn create_deletion_challenge(
        &self,
        challenge: &DeletionChallengeRecord,
    ) -> Result<(), MemoryError> {
        let consumed_assignment = if challenge.consumed_at.is_some() {
            "consumed_at = type::datetime($consumed_at)"
        } else {
            "consumed_at = NONE"
        };
        let sql = format!(
            "CREATE type::record($table, $id) SET id = $id, verifier = $verifier, account_id = $account_id, session_id = $session_id, expires_at = type::datetime($expires_at), {consumed_assignment}, created_at = time::now()"
        );
        self.handle()
            .query_json(
                &sql,
                Some(json!({
                    "table": "deletion_challenge",
                    "id": challenge.id,
                    "verifier": challenge.verifier,
                    "account_id": challenge.account_id,
                    "session_id": challenge.session_id,
                    "expires_at": challenge.expires_at.to_rfc3339(),
                    "consumed_at": challenge.consumed_at.map(|value| value.to_rfc3339()),
                })),
            )
            .await
            .map_err(|error| map_storage_error("create deletion challenge", error))?;
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn consume_deletion_challenge(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let rows = self
            .handle()
            .query_json(
                "UPDATE type::table($table) SET consumed_at = type::datetime($now) WHERE verifier = $verifier AND account_id = $account_id AND session_id = $session_id AND consumed_at IS NONE AND expires_at > type::datetime($now) RETURN AFTER",
                Some(json!({"table": "deletion_challenge", "verifier": verifier, "account_id": account_id, "session_id": session_id, "now": now.to_rfc3339()})),
            )
            .await
            .map_err(|error| map_storage_error("consume deletion challenge", error))?;
        if rows.is_empty() {
            return Err(MemoryError::Conflict(
                "deletion challenge is invalid or expired".into(),
            ));
        }
        Ok(())
    }
}

#[allow(dead_code)]
fn base64_encode(bytes: &[u8]) -> String {
    let mut buf = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    // Minimal RFC 4648 base64 encoder.
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        buf.push(ALPHABET[(b0 >> 2) as usize]);
        buf.push(ALPHABET[(((b0 & 0b11) << 4) | (b1 >> 4)) as usize]);
        if chunk.len() > 1 {
            buf.push(ALPHABET[(((b1 & 0b1111) << 2) | (b2 >> 6)) as usize]);
        } else {
            buf.push(b'=');
        }
        if chunk.len() > 2 {
            buf.push(ALPHABET[(b2 & 0b111111) as usize]);
        } else {
            buf.push(b'=');
        }
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[allow(dead_code)]
fn base64_decode(input: &str) -> Result<Vec<u8>, &'static str> {
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }

    let bytes = input.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return Err("length is not a multiple of four");
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for (chunk_index, chunk) in bytes.chunks_exact(4).enumerate() {
        let a = val(chunk[0]).ok_or("invalid character")?;
        let b = val(chunk[1]).ok_or("invalid character")?;
        let padding_at_two = chunk[2] == b'=';
        let padding_at_three = chunk[3] == b'=';
        if padding_at_two && !padding_at_three {
            return Err("invalid padding");
        }
        if (padding_at_two || padding_at_three) && chunk_index + 1 != bytes.len() / 4 {
            return Err("padding must be at the end");
        }
        let c = if padding_at_two {
            0
        } else {
            val(chunk[2]).ok_or("invalid character")?
        };
        let d = if padding_at_three {
            0
        } else {
            val(chunk[3]).ok_or("invalid character")?
        };
        out.push((a << 2) | (b >> 4));
        if !padding_at_two {
            out.push((b << 4) | (c >> 2));
        }
        if !padding_at_three {
            out.push((c << 6) | d);
        }
    }
    Ok(out)
}

// ─── ensure_namespace re-export for callers that need DDL ───────

pub use super::storage::ensure_namespace as ensure_registry_namespace;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::RegistryStore;
    use crate::http::registry::models::{AccountStatus, NamespaceBinding, TenantStatus};
    use surrealdb::engine::local::Mem;

    fn account() -> Account {
        Account {
            id: "acct_shared".into(),
            status: AccountStatus::Active,
            tenant_id: "ten_shared".into(),
            created_at: Utc::now(),
        }
    }

    fn tenant() -> Tenant {
        Tenant {
            id: "ten_shared".into(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: "tns_shared".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 0,
        }
    }

    #[tokio::test]
    async fn durable_registry_persists_across_store_handles() {
        let db = Arc::new(Surreal::new::<Mem>(()).await.expect("mem engine"));
        let first = SurrealRegistryStore::from_local_db(db.clone(), "control", "registry")
            .await
            .expect("bind first store");
        first
            .apply_migrations()
            .await
            .expect("apply registry schema");
        first
            .ensure_plan(&crate::http::registry::models::Plan::default())
            .await
            .expect("ensure default plan");
        let loaded_plan = first.load_plan(1).await.expect("load default plan");
        assert_eq!(loaded_plan.version, 1);
        assert_eq!(loaded_plan.limits.ingest_per_minute, 60);
        let identity = ExternalIdentity {
            id: "idn_shared".into(),
            issuer: "https://issuer.example".into(),
            subject_verifier: SubjectVerifier([7; 32]),
            account_id: "acct_shared".into(),
            created_at: Utc::now(),
        };
        first
            .create_account_bundle(&account(), &tenant(), Some(&identity))
            .await
            .expect("create account bundle");

        let second = SurrealRegistryStore::from_local_db(db, "control", "registry")
            .await
            .expect("bind second store");
        let loaded_account = second
            .find_account_by_id("acct_shared")
            .await
            .expect("read account")
            .expect("account exists");
        assert_eq!(loaded_account.tenant_id, "ten_shared");
        let loaded_tenant = second
            .find_tenant_by_account("acct_shared")
            .await
            .expect("read tenant")
            .expect("tenant exists");
        assert_eq!(loaded_tenant.namespace_binding.namespace, "tns_shared");
        let loaded_identity = second
            .find_account_by_identity("https://issuer.example", &SubjectVerifier([7; 32]))
            .await
            .expect("read identity")
            .expect("identity resolves");
        assert_eq!(loaded_identity.id, "acct_shared");

        let key = ApiKey {
            id: "ak_shared".into(),
            account_id: "acct_shared".into(),
            name: "integration".into(),
            verifier: KeyedVerifier([9; 32]),
            status: ApiKeyStatus::Active,
            created_at: Utc::now(),
            expires_at: Some(Utc::now() + chrono::Duration::days(1)),
            last_used_at: None,
            version: 0,
        };
        second.write_api_key(&key).await.expect("write API key");
        let second_key = ApiKey {
            id: "ak_shared_2".into(),
            account_id: "acct_shared".into(),
            name: "second".into(),
            verifier: KeyedVerifier([8; 32]),
            status: ApiKeyStatus::Active,
            created_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
            version: 0,
        };
        let cap_result = second.create_api_key_if_below_limit(&second_key, 1).await;
        assert!(
            matches!(cap_result, Err(MemoryError::Conflict(message)) if message.contains("limit"))
        );
        let loaded_key = second
            .find_api_key("ak_shared")
            .await
            .expect("read API key")
            .expect("API key exists");
        assert_eq!(loaded_key.account_id, "acct_shared");
        assert_eq!(loaded_key.verifier.0, [9; 32]);

        second
            .begin_operator_deletion("ten_shared", "operator", Utc::now())
            .await
            .expect("operator deletion start");
        assert_eq!(
            second
                .find_account_by_id("acct_shared")
                .await
                .expect("read deleting account")
                .expect("account")
                .status,
            AccountStatus::Deleting
        );
        assert_eq!(
            second
                .find_api_key("ak_shared")
                .await
                .expect("read revoked key")
                .expect("key")
                .status,
            ApiKeyStatus::Revoked
        );

        let now = Utc::now();
        let account_delete = Account {
            id: "acct_delete".into(),
            status: AccountStatus::Active,
            tenant_id: "ten_delete".into(),
            created_at: now,
        };
        let tenant_delete = Tenant {
            id: "ten_delete".into(),
            status: TenantStatus::Ready,
            namespace_binding: NamespaceBinding {
                namespace: "tns_delete".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 44,
            retry_stage: None,
            provisioning_lease: None,
            created_at: now,
            version: 0,
        };
        second
            .create_account_bundle(&account_delete, &tenant_delete, None)
            .await
            .expect("create deletion account");
        second
            .create_deletion_challenge(&DeletionChallengeRecord {
                id: "challenge_delete".into(),
                verifier: "verifier_delete".into(),
                account_id: account_delete.id.clone(),
                session_id: "session_delete".into(),
                expires_at: now + chrono::Duration::minutes(5),
                consumed_at: None,
            })
            .await
            .expect("create deletion challenge");
        second
            .begin_account_deletion("verifier_delete", "acct_delete", "session_delete", now)
            .await
            .expect("durable deletion start");
        assert_eq!(
            second
                .find_account_by_id("acct_delete")
                .await
                .expect("read deleting account")
                .expect("account")
                .status,
            AccountStatus::Deleting
        );
        let replay = second
            .begin_account_deletion("verifier_delete", "acct_delete", "session_delete", now)
            .await;
        assert!(matches!(replay, Err(MemoryError::Conflict(_))));
    }
}
