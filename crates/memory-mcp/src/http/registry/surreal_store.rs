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
#[cfg(feature = "control-plane")]
use crate::http::config::BrowserAuthMethod;
#[cfg(feature = "control-plane")]
use crate::service::local_admin::contracts::LocalKeyFingerprints;

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
    /// Like `query_json` but extracts the result at the explicit
    /// `result_index` instead of 0. Every statement error is checked
    /// before extraction; a missing or malformed result at the given
    /// index is an error, not an empty success.
    async fn query_json_at(
        &self,
        sql: &str,
        vars: Option<Value>,
        result_index: usize,
    ) -> Result<Vec<Value>, MemoryError>;
    async fn ping(&self) -> bool;
}

/// Read one statement's result as a list of rows.
///
/// `Response::take::<serde_json::Value>` refuses a statement that yields
/// more than one record: it reports "Tried to take only a single result
/// from a query that contains multiple". That silently broke every
/// `SELECT` returning two or more rows, including the provisioning
/// scheduler's tenant listing. Reading the untyped value first and
/// converting afterwards keeps the array shape, so both a multi-row read
/// and a scalar read behave correctly.
///
/// Returns `Ok(None)` when the statement index produced no value at all,
/// so callers can distinguish "empty result" from "missing statement".
fn take_statement_rows(
    response: &mut surrealdb::IndexedResults,
    index: usize,
) -> Result<Option<Vec<Value>>, MemoryError> {
    // Take the untyped value directly. `Option<T>` is deliberately avoided:
    // SurrealDB defines `Option<T>` as "at most one row" and errors on a
    // multi-element array, which is exactly what a `SELECT` returns.
    let raw: surrealdb::types::Value = response
        .take::<surrealdb::types::Value>(index)
        .map_err(|err| MemoryError::Storage(format!("take failed: {err}")))?;
    if matches!(raw, surrealdb::types::Value::None) {
        return Ok(None);
    }
    // `into_json_value` is the same flattening `Response::take::<serde_json::Value>`
    // applies; serialising the typed value directly would instead produce the
    // externally-tagged form (`{"Datetime": "..."}`).
    let json: Value = raw.into_json_value();
    Ok(Some(match json {
        Value::Null => Vec::new(),
        Value::Array(values) => values,
        value => vec![value],
    }))
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
        let rows = take_statement_rows(&mut response, 0)
            .map_err(|err| MemoryError::Storage(format!("take failed: {err}")))?;
        Ok(rows.unwrap_or_default())
    }
    async fn query_json_at(
        &self,
        sql: &str,
        vars: Option<Value>,
        result_index: usize,
    ) -> Result<Vec<Value>, MemoryError> {
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
        let rows = take_statement_rows(&mut response, result_index).map_err(|err| {
            MemoryError::Storage(format!("take index {result_index} failed: {err}"))
        })?;
        rows.ok_or_else(|| {
            MemoryError::Storage(format!("query returned no result at index {result_index}"))
        })
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
        let rows = take_statement_rows(&mut response, 0)
            .map_err(|err| MemoryError::Storage(format!("take failed: {err}")))?;
        Ok(rows.unwrap_or_default())
    }
    async fn query_json_at(
        &self,
        sql: &str,
        vars: Option<Value>,
        result_index: usize,
    ) -> Result<Vec<Value>, MemoryError> {
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
        let rows = take_statement_rows(&mut response, result_index).map_err(|err| {
            MemoryError::Storage(format!("take index {result_index} failed: {err}"))
        })?;
        rows.ok_or_else(|| {
            MemoryError::Storage(format!("query returned no result at index {result_index}"))
        })
    }
    async fn ping(&self) -> bool {
        match self.query("INFO FOR DB").await {
            Ok(mut response) => response.take_errors().is_empty(),
            Err(_) => false,
        }
    }
}

/// Whether a storage message says that a uniqueness constraint was violated.
///
/// SurrealDB spells the same condition two ways: inserting a record that exists
/// says "already exists", while a unique index says "Database index `x` already
/// *contains* …". Both have to be recognized **here**, in one place, because
/// [`map_storage_error`] classifies with this predicate and [`is_conflict_error`]
/// re-detects the result: a message one of them recognizes and the other does not
/// would be classified as a conflict and then treated as something else.
fn is_unique_violation_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("already exists")
        || lower.contains("already contains")
        || lower.contains("duplicate")
        || lower.contains("unique")
}

/// Convert a SurrealDB error string into a typed MemoryError.
fn map_storage_error(context: &str, err: impl std::fmt::Display) -> MemoryError {
    let msg = err.to_string();
    let lower = msg.to_ascii_lowercase();
    if is_unique_violation_message(&lower) {
        MemoryError::Conflict(format!("{context}: {msg}"))
    } else if lower.contains("not found") || lower.contains("no record") {
        MemoryError::NotFound(format!("{context}: {msg}"))
    } else {
        MemoryError::Storage(format!("{context}: {msg}"))
    }
}

/// Whether an error [`map_storage_error`] produced came from a uniqueness
/// violation, so a caller that lost the original message can retry or continue.
fn is_conflict_error(error: &MemoryError) -> bool {
    matches!(error, MemoryError::Conflict(message) if is_unique_violation_message(message))
}

/// The first of `sentinels` that a `THROW`-ed statement surfaced in the
/// storage error's text, or `None`.
///
/// A `THROW` inside a guarded transaction reaches the adapter as an opaque
/// storage failure carrying only its own sentinel, so a caller that needs a
/// typed error matches on the token it threw rather than on message prose.
#[cfg(feature = "control-plane")]
fn thrown_token<'a>(error: &MemoryError, sentinels: &[&'a str]) -> Option<&'a str> {
    let MemoryError::Storage(message) = error else {
        return None;
    };
    sentinels
        .iter()
        .copied()
        .find(|token| message.contains(token))
}

/// Engine-level MVCC write conflicts are transient: the datastore
/// explicitly marks the transaction retryable. Hot usage rows see
/// them under concurrent ingest, so quota admission retries them
/// with a short backoff instead of surfacing a 5xx.
fn is_write_conflict(error: &MemoryError) -> bool {
    let message = match error {
        MemoryError::Storage(message) | MemoryError::Conflict(message) => message.as_str(),
        _ => return false,
    };
    let lower = message.to_ascii_lowercase();
    lower.contains("write conflict") || lower.contains("retry the transaction")
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

/// Sentinel the unlink transaction throws when the removal would leave an
/// Account with no identity at all. A refused removal has to be told apart from
/// an infrastructure failure, and a `THROW` reaches the adapter as storage text.
const LAST_IDENTITY_SENTINEL: &str = "last identity cannot be unlinked";

/// Query variables shared by both identity-change transactions, so the two call
/// sites cannot drift in which columns they write.
///
/// The identity id is bound once and used both as the `external_identity` record
/// id and as the `target_identity_id` column: an unbound id would silently write
/// a single `external_identity:NONE` row that the next link then collides with.
fn identity_audit_vars(event: &ControlAuditEvent) -> Value {
    json!({
        "account_id": event.account_id,
        "actor_kind": event.actor_kind.as_str(),
        "actor_principal": event.actor_principal,
        "action": event.action,
        "identity_id": event.target_identity_id.as_deref().unwrap_or_default(),
        "correlation_id": event.correlation_id,
        "occurred_at": event.occurred_at.to_rfc3339(),
    })
}

/// Classify the failure of a guarded identity-change transaction. The guard's
/// own sentinel is the refusal; everything else keeps the adapter's mapping, so a
/// unique-tuple violation still reads as a `Conflict` and a missing Account or
/// identity still reads as `NotFound`.
/// Token the replace transaction `THROW`s when the Account does not hold
/// exactly one identity. It is a token rather than message prose (see
/// `thrown_token`), and the script assembles it from split literals so a
/// parse-error echo of the transaction text can never carry the assembled
/// token and be misclassified as a refusal — that false match once cost a
/// full debugging round. The user-facing message lives in
/// `classify_identity_change_error`.
const REPLACE_GUARD_TOKEN: &str = "replace_guard";

fn classify_identity_change_error(context: &str, error: MemoryError) -> MemoryError {
    let message = error.to_string();
    if message.contains(LAST_IDENTITY_SENTINEL) {
        return MemoryError::Conflict("the account's last identity cannot be unlinked".into());
    }
    // Token match on storage text only, never message prose: a parse error
    // echoes the transaction source, so prose sentinels would false-match it.
    if matches!(&error, MemoryError::Storage(_)) && message.contains(REPLACE_GUARD_TOKEN) {
        return MemoryError::Conflict("replace requires exactly one existing identity".into());
    }
    map_storage_error(context, error)
}

fn migration_checksum(sql: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    hex::encode(hasher.finalize())
}

/// SurrealQL `SET` assignment plus query variables for writing a
/// tenant's `provisioning_lease`. Nested datetimes inside a FLEXIBLE
/// object are not coerced from bound strings by the schema, so every
/// datetime uses an explicit `type::datetime` cast, matching every
/// other datetime write in this store.
fn lease_write_assignment(lease: Option<&ProvisioningLeaseState>) -> (String, Value) {
    match lease {
        None => ("provisioning_lease = NONE".to_owned(), json!({})),
        Some(lease) => (
            "provisioning_lease = { owner_id: $lease_owner_id, lease_id: $lease_id, \
             fencing_generation: $lease_fencing_generation, \
             expires_at: type::datetime($lease_expires_at), \
             heartbeat_at: type::datetime($lease_heartbeat_at) }"
                .to_owned(),
            json!({
                "lease_owner_id": lease.owner_id,
                "lease_id": lease.lease_id,
                "lease_fencing_generation": lease.fencing_generation,
                "lease_expires_at": lease.expires_at.to_rfc3339(),
                "lease_heartbeat_at": lease.heartbeat_at.to_rfc3339(),
            }),
        ),
    }
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
    /// SQL fault seam (plan §5). Production never arms it, so it is inert
    /// for every real deployment; tests arm a needle to fail a named
    /// local-admin statement and prove the adapter rolls back cleanly.
    /// Only the `control-plane` local-admin statements consult it, so it
    /// is compiled with them.
    #[cfg(feature = "control-plane")]
    sql_faults: crate::http::fault_injection::SqlFaultHook,
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
        // Surreal's textual record id looks like
        // `<table>:<key>`. The `<key>` may be a UUID, a number, or
        // a backtick-quoted string for arbitrary identifiers; strip
        // both the table prefix and any surrounding backticks so
        // downstream `type::record($table, $id)` calls receive a
        // bare key. The textual id is sufficient for the InMemory
        // bootstrap path; the Surreal side uses the same key.
        let stripped = value
            .rsplit_once(':')
            .map_or_else(|| value.to_owned(), |(_, key)| key.to_owned());
        stripped
            .strip_prefix('`')
            .and_then(|s| s.strip_suffix('`'))
            .map(str::to_owned)
            .unwrap_or(stripped)
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
            #[cfg(feature = "control-plane")]
            sql_faults: crate::http::fault_injection::SqlFaultHook::new(),
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
            #[cfg(feature = "control-plane")]
            sql_faults: crate::http::fault_injection::SqlFaultHook::new(),
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
        'catalog: for name in super::migrations::REGISTRY_MIGRATIONS {
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
                            // Already applied by an earlier connect: skip both the
                            // DDL and the fenced completion update. This replica
                            // does not own the row, so re-running the completion
                            // would always fail its owner fence.
                            continue 'catalog;
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

/// OIDC policy guard fragment shared by every OIDC-only session and
/// flow operation.
///
/// It is a constant 4-statement fragment, so within any guarded
/// transaction it occupies result indices 1..=4 (index 0 is the opening
/// `BEGIN TRANSACTION`) and the method-specific statement is index 5.
/// A `THROW` aborts the transaction and is surfaced by `query_json_at`
/// before any result is decoded. The caller binds `$expected_epoch` to
/// the fence joined at startup; a missing or ambiguous singleton, a
/// deployment that does not enable `oidc`, or a stale epoch all fail the
/// transaction.
///
/// The row is read with `LIMIT 2` so "exactly one row" is a checked
/// precondition rather than an assumption about which row the engine
/// returns: a second row now fails loudly instead of being silently
/// ignored. `methods` is authoritative and `mode` only supplies the value
/// for a row created before migration 048.
#[cfg(feature = "control-plane")]
const OIDC_POLICY_GUARD: &str = r#"
LET $policy = (SELECT mode, epoch, methods FROM browser_auth_policy LIMIT 2);
IF array::len($policy) != 1 { THROW 'no_policy'; };
IF NOT ('oidc' IN $policy[0].methods ?? [$policy[0].mode]) { THROW 'mode_mismatch'; };
IF $policy[0].epoch != $expected_epoch { THROW 'epoch_mismatch'; };
"#;

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
        let (lease_assignment, lease_vars) =
            lease_write_assignment(tenant.provisioning_lease.as_ref());
        let retry_stage_assignment = if tenant.retry_stage.is_some() {
            "retry_stage = $retry_stage"
        } else {
            "retry_stage = NONE"
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
            "account_created_at": account.created_at.to_rfc3339(),
            "tenant_created_at": tenant.created_at.to_rfc3339(),
            "version": tenant.version,
        });
        if let (Some(vars), Some(lease_vars)) = (vars.as_object_mut(), lease_vars.as_object()) {
            vars.extend(lease_vars.clone());
        }
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

    async fn link_external_identity(
        &self,
        identity: &ExternalIdentity,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        // One transaction: the identity and the `identity_linked` row describing
        // it are written together, or neither is (ADR-0057). The row cannot be
        // derived later, because the identity it names is deletable.
        let event = ControlAuditEvent::identity_change(
            IdentityAuditAction::Linked,
            &identity.id,
            &identity.account_id,
            audit,
        );
        let mut vars = identity_audit_vars(&event);
        vars["issuer"] = json!(identity.issuer);
        vars["verifier"] = json!(hex::encode(identity.subject_verifier.0));
        vars["created_at"] = json!(identity.created_at.to_rfc3339());
        let script = "BEGIN TRANSACTION; \
            LET $account = SELECT id FROM type::record('account', $account_id) LIMIT 1; \
            IF array::len($account) = 0 { THROW 'account not found'; }; \
            CREATE type::record('external_identity', $identity_id) SET \
                id = $identity_id, issuer = $issuer, \
                subject_verifier = $verifier, account_id = $account_id, \
                created_at = type::datetime($created_at); \
            CREATE type::record('audit_event', $correlation_id) SET \
                account_id = $account_id, \
                actor_kind = $actor_kind, \
                actor_principal = $actor_principal, \
                action = $action, \
                target_identity_id = IF $identity_id = '' { NONE } ELSE { $identity_id }, \
                occurred_at = type::datetime($occurred_at), \
                correlation_id = $correlation_id; \
            COMMIT TRANSACTION;";
        self.handle()
            .query_json(script, Some(vars))
            .await
            .map_err(|error| classify_identity_change_error("link external identity", error))?;
        Ok(())
    }

    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        // The last-identity rule is enforced here rather than by the caller so
        // that the count and the delete share one transaction: two concurrent
        // removals can no longer both read two identities and both delete one
        // (ADR-0057).
        let event = ControlAuditEvent::identity_change(
            IdentityAuditAction::Unlinked,
            identity_id,
            account_id,
            audit,
        );
        let script = "BEGIN TRANSACTION; \
            LET $held = SELECT id FROM external_identity WHERE account_id = $account_id; \
            IF array::len($held) < 2 { THROW 'last identity cannot be unlinked'; }; \
            LET $removed = DELETE type::record('external_identity', $identity_id) \
                WHERE account_id = $account_id RETURN BEFORE; \
            IF array::len($removed) = 0 { THROW 'identity not found'; }; \
            CREATE type::record('audit_event', $correlation_id) SET \
                account_id = $account_id, \
                actor_kind = $actor_kind, \
                actor_principal = $actor_principal, \
                action = $action, \
                target_identity_id = IF $identity_id = '' { NONE } ELSE { $identity_id }, \
                occurred_at = type::datetime($occurred_at), \
                correlation_id = $correlation_id; \
            COMMIT TRANSACTION;";
        self.handle()
            .query_json(script, Some(identity_audit_vars(&event)))
            .await
            .map_err(|error| classify_identity_change_error("unlink external identity", error))?;
        Ok(())
    }

    async fn replace_external_identity(
        &self,
        new_identity: &ExternalIdentity,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        // One transaction: the removal, the addition and both audit rows
        // commit together, so the Account never has zero identities and two
        // racing replacements cannot both swap (ADR-0057 invitation
        // remediation). The exactly-one guard is the refusal; the unique-tuple
        // index stays the backstop the classifier reads as `Conflict`. A
        // replay of the same tuple is a guarded no-op.
        let linked = ControlAuditEvent::identity_change(
            IdentityAuditAction::Linked,
            &new_identity.id,
            &new_identity.account_id,
            audit,
        );
        let mut vars = identity_audit_vars(&linked);
        vars["issuer"] = json!(new_identity.issuer);
        vars["verifier"] = json!(hex::encode(new_identity.subject_verifier.0));
        vars["created_at"] = json!(new_identity.created_at.to_rfc3339());
        let script = "BEGIN TRANSACTION; \
            LET $account = SELECT id FROM type::record('account', $account_id) LIMIT 1; \
            IF array::len($account) = 0 { THROW 'account not found'; }; \
            LET $held = SELECT id, issuer, subject_verifier FROM external_identity \
                WHERE account_id = $account_id; \
            IF array::len($held) != 1 { THROW string::concat('replace_', 'guard'); }; \
            IF $held[0].issuer != $issuer OR $held[0].subject_verifier != $verifier { \
                DELETE type::record('external_identity', $held[0].id); \
                CREATE type::record('external_identity', $identity_id) SET \
                    id = $identity_id, issuer = $issuer, \
                    subject_verifier = $verifier, account_id = $account_id, \
                    created_at = type::datetime($created_at); \
                CREATE type::record('audit_event', \
                    string::concat('identity_unlinked_', record::id($held[0].id))) SET \
                    account_id = $account_id, \
                    actor_kind = $actor_kind, \
                    actor_principal = $actor_principal, \
                    action = 'identity_unlinked', \
                    target_identity_id = record::id($held[0].id), \
                    occurred_at = type::datetime($occurred_at), \
                    correlation_id = string::concat('identity_unlinked_', record::id($held[0].id)); \
                CREATE type::record('audit_event', $correlation_id) SET \
                    account_id = $account_id, \
                    actor_kind = $actor_kind, \
                    actor_principal = $actor_principal, \
                    action = $action, \
                    target_identity_id = $identity_id, \
                    occurred_at = type::datetime($occurred_at), \
                    correlation_id = $correlation_id; \
            }; \
            COMMIT TRANSACTION;";
        self.handle()
            .query_json(script, Some(vars))
            .await
            .map_err(|error| classify_identity_change_error("replace external identity", error))?;
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
                "SELECT id, name, status, created_at, expires_at, last_used_at, verifier \
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
        let (lease_assignment, lease_vars) =
            lease_write_assignment(tenant.provisioning_lease.as_ref());
        let sql = format!(
            "UPSERT type::record($table, $id) SET id = $id, status = IF status = 'purged' AND $status != 'purged' THEN status ELSE $status END, namespace_binding = IF namespace_binding IS NONE THEN $binding ELSE namespace_binding END, plan_version = $plan_version, schema_version = $schema_version, {retry_stage_assignment}, {lease_assignment}, created_at = type::datetime($created_at), version = $version"
        );
        self.handle()
            .query_json(&sql, {
                let mut vars = json!({
                    "table": "tenant",
                    "id": tenant.id,
                    "status": status,
                    "binding": tenant.namespace_binding,
                    "plan_version": tenant.plan_version,
                    "schema_version": tenant.schema_version,
                    "retry_stage": tenant.retry_stage,
                    "created_at": tenant.created_at.to_rfc3339(),
                    "version": tenant.version,
                });
                if let (Some(vars), Some(lease_vars)) =
                    (vars.as_object_mut(), lease_vars.as_object())
                {
                    vars.extend(lease_vars.clone());
                }
                Some(vars)
            })
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
        // `Suspended` is deliberately absent: a suspended tenant is a
        // terminal, operator-chosen state (see `provisioning::reconcile`),
        // so the worker must not claim a lease for it or attempt to advance
        // it back to `Ready`. Resume is an explicit operator action that
        // performs the `Suspended -> Ready` transition itself.
        //
        // `namespace_creating` is present because a worker that dies between
        // the `Reserved -> NamespaceCreating` and `NamespaceCreating ->
        // Migrating` writes must be resumed by the next tick; omitting it
        // stranded such a tenant forever.
        let rows = self
            .handle()
            .query_json(
                "SELECT * FROM type::table($table) \
                 WHERE status IN ['reserved', 'namespace_creating', 'migrating', 'failed'] \
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
                "SELECT * FROM tenant \
                 WHERE status = 'ready' \
                 AND ($cursor IS NONE OR id > type::record('tenant', $cursor)) \
                 ORDER BY id LIMIT $limit",
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

    async fn ensure_local_plan(
        &self,
        plan: &super::models::Plan,
    ) -> Result<super::models::Plan, MemoryError> {
        if plan.version == 0 {
            return Err(MemoryError::ConfigInvalid(
                "local plan version must be at least 1".into(),
            ));
        }
        let plan_id = format!("local_plan_v{}", plan.version);
        // SurrealDB requires braces around a compound `IF` body and does
        // not accept a nested `IF` directly inside `ELSE`; a single
        // `ELSE IF` keeps the drift check in one statement.
        //
        // The lookup is by **version**, not by id: `idx_plan_version` is
        // unique, and a version created by an earlier OIDC or bootstrap
        // deployment must be reused when its limits agree rather than
        // colliding with a second row.
        //
        // Statement order is BEGIN, LET, IF, LET, IF, SELECT, COMMIT, so
        // the explicit result index below is 5.
        let sql = "
            BEGIN TRANSACTION;
            LET $existing = (SELECT * FROM plan WHERE version = $version LIMIT 1);
            IF array::len($existing) = 0 {
                CREATE type::record('plan', $plan_id) SET
                    id = $plan_id,
                    version = $version,
                    limits = $limits;
            };
            LET $current = (SELECT * FROM plan WHERE version = $version LIMIT 1);
            IF array::len($current) = 0 {
                THROW 'plan_missing';
            } ELSE IF $current[0].limits.max_ingested_bytes != $max_ingested_bytes OR
                      $current[0].limits.max_episode_count != $max_episode_count OR
                      $current[0].limits.ingest_per_minute != $ingest_per_minute OR
                      $current[0].limits.max_open_app_sessions != $max_open_app_sessions OR
                      $current[0].limits.max_active_api_keys != $max_active_api_keys OR
                      $current[0].limits.per_tenant_request_concurrency != $per_tenant_request_concurrency OR
                      $current[0].limits.extraction_concurrency != $extraction_concurrency {
                THROW 'plan_limit_mismatch';
            };
            SELECT * FROM plan WHERE version = $version LIMIT 1;
            COMMIT TRANSACTION;";
        let vars = Some(json!({
            "plan_id": plan_id,
            "version": plan.version,
            "limits": plan.limits,
            "max_ingested_bytes": plan.limits.max_ingested_bytes,
            "max_episode_count": plan.limits.max_episode_count,
            "ingest_per_minute": plan.limits.ingest_per_minute,
            "max_open_app_sessions": plan.limits.max_open_app_sessions,
            "max_active_api_keys": plan.limits.max_active_api_keys,
            "per_tenant_request_concurrency": plan.limits.per_tenant_request_concurrency,
            "extraction_concurrency": plan.limits.extraction_concurrency,
        }));
        let attempt = self.handle().query_json_at(sql, vars.clone(), 5).await;
        let rows = match attempt {
            Ok(rows) => rows,
            Err(error) => {
                // A concurrent replica may have created the version while
                // this transaction was in flight; the unique version index
                // then aborts this attempt. Re-read once: a compatible row
                // means the race was harmless, anything else is real drift.
                self.handle()
                    .query_json_at(sql, vars, 5)
                    .await
                    .map_err(|_| map_storage_error("ensure local plan", error))?
            }
        };
        let Some(row) = rows.into_iter().next() else {
            return Err(MemoryError::Storage(
                "ensure_local_plan returned no rows".into(),
            ));
        };
        let id = row
            .get("id")
            .and_then(record_id_value)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| MemoryError::Storage("plan row has no valid id".into()))?;
        let stored_version = required_u32(&row, "version")?;
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

    #[cfg(feature = "control-plane")]
    async fn reconcile_browser_policy(
        &self,
        desired: &[BrowserAuthMethod],
        local: Option<LocalKeyFingerprints>,
    ) -> Result<BrowserPolicyFence, MemoryError> {
        if local.is_some() != desired.contains(&BrowserAuthMethod::Local) {
            return Err(MemoryError::Storage(
                "local fingerprints must be supplied exactly when the local method is enabled"
                    .into(),
            ));
        }
        // Statement order: `BEGIN`(0) `LET`(1) `IF`(2) `IF`(3) `LET`(4)
        // `IF`(5) `IF`(6) `IF`(7) `UPDATE`(8) `SELECT`(9) `COMMIT`(10). Result
        // index 9 is the policy readback. `array::includes` is not in this
        // build's function registry, so membership is the `IN` operator; `??`
        // reads a legacy row's `mode` as its one-method set.
        //
        // The fingerprint statements are guarded on `$writes_local` rather than
        // written unconditionally: a `NULL` parameter does not coerce into the
        // `option<string>` columns, and an absent method's parameters are never
        // bound at all.
        let canonical_desired = BrowserAuthMethod::canonical_set(desired);
        let sql = "
            BEGIN TRANSACTION;
            LET $existing = (SELECT mode, epoch, methods, local_session_fingerprint, local_csrf_fingerprint FROM browser_auth_policy LIMIT 2);
            IF array::len($existing) = 0 {
                CREATE browser_auth_policy SET
                    mode = $desired[0],
                    methods = $desired,
                    epoch = 1,
                    version = 1,
                    created_at = time::now(),
                    updated_at = time::now();
            };
            IF array::len($existing) > 1 { THROW 'policy_ambiguous'; };
            LET $methods = $existing[0].methods ?? [$existing[0].mode];
            IF ('local' IN $methods AND NOT ('local' IN $desired)) OR ('oidc' IN $methods AND NOT ('oidc' IN $desired)) { THROW 'policy_removal'; };
            IF $writes_local AND 'local' IN $methods AND ($existing[0].local_session_fingerprint != $session_fingerprint OR $existing[0].local_csrf_fingerprint != $csrf_fingerprint) { THROW 'policy_key_mismatch'; };
            IF $writes_local { UPDATE browser_auth_policy SET local_session_fingerprint = $session_fingerprint, local_csrf_fingerprint = $csrf_fingerprint; };
            UPDATE browser_auth_policy SET methods = $desired, updated_at = time::now();
            SELECT mode, epoch, methods FROM browser_auth_policy LIMIT 2;
            COMMIT TRANSACTION;";
        let mut params = json!({
            "desired": canonical_desired
                .iter()
                .map(|method| method.as_str())
                .collect::<Vec<_>>(),
            "writes_local": local.is_some(),
        });
        if let Some(fingerprints) = local {
            params["session_fingerprint"] = json!(hex::encode(fingerprints.session));
            params["csrf_fingerprint"] = json!(hex::encode(fingerprints.csrf));
        }
        let rows = self
            .handle()
            .query_json_at(sql, Some(params), 9)
            .await
            .map_err(|error| {
                match thrown_token(&error, &["policy_removal", "policy_key_mismatch"]) {
                Some("policy_removal") => MemoryError::Conflict(
                    "this deployment's durable browser-auth policy enables a method that the \
                     configuration omits; removing a method is an explicit operation, not a \
                     startup reconciliation"
                        .into(),
                ),
                Some(_) => MemoryError::Conflict(
                    "the local administrator's key fingerprints do not match the durable policy; \
                     MEMORY_MCP_HTTP_SESSION_KEY and MEMORY_MCP_HTTP_CSRF_KEY must be the keys \
                     the policy was created with"
                        .into(),
                ),
                None => map_storage_error("reconcile browser policy", error),
            }
            })?;
        let Some(row) = rows.into_iter().next() else {
            return Err(MemoryError::Storage(
                "reconcile_browser_policy returned no rows".into(),
            ));
        };
        let methods = policy_methods_from_row(&row).ok_or_else(|| {
            MemoryError::Storage(
                "policy row carries neither an enabled-method set nor a recognized mode".into(),
            )
        })?;
        for method in canonical_desired {
            if !methods.contains(&method) {
                return Err(MemoryError::Storage(format!(
                    "policy row does not enable '{}'",
                    method.as_str()
                )));
            }
        }
        let epoch = required_u64(&row, "epoch")?;
        Ok(BrowserPolicyFence { methods, epoch })
    }

    /// ADR-0057 removal, as one transaction: the narrowed set, the advanced
    /// epoch and the operator-action audit row commit together or not at all.
    ///
    /// Statement order: `BEGIN`(0) `LET`(1) `IF`(2) `IF`(3) `LET`(4) `IF`(5)
    /// `IF`(6) `UPDATE`(7) `CREATE`(8) `SELECT`(9) `COMMIT`(10). Result index 9
    /// is the policy readback.
    ///
    /// The set left behind is derived in Rust and passed in as `$remaining`,
    /// with the statement asserting that the same method is present in
    /// `$methods`. For a two-method universe the two conditions together are
    /// exactly "the stored set minus `$method`": the stored set holds `$method`
    /// and the other method, so what remains is the other method. Expressing it
    /// this way avoids `array::filter`/`array::difference`, which are not in this
    /// build's function registry.
    ///
    /// `mode` — the pre-048 column — is left as it was, exactly as
    /// `reconcile_browser_policy` leaves it: `methods` is authoritative and `mode`
    /// is read only as the fallback for a row that predates it.
    #[cfg(feature = "control-plane")]
    async fn remove_browser_auth_method(
        &self,
        method: BrowserAuthMethod,
    ) -> Result<BrowserPolicyFence, MemoryError> {
        let remaining = match method {
            BrowserAuthMethod::Local => BrowserAuthMethod::Oidc,
            BrowserAuthMethod::Oidc => BrowserAuthMethod::Local,
        };
        // One id names the operation, and it is the audit row's durable id: a
        // derived id would collide when a method is removed, restored by
        // configuration, and removed again.
        let operation_id = uuid::Uuid::new_v4().to_string();
        let sql = "
            BEGIN TRANSACTION;
            LET $existing = (SELECT mode, epoch, methods FROM browser_auth_policy LIMIT 2);
            IF array::len($existing) = 0 { THROW 'policy_absent'; };
            IF array::len($existing) > 1 { THROW 'policy_ambiguous'; };
            LET $methods = $existing[0].methods ?? [$existing[0].mode];
            IF NOT ($method IN $methods) { THROW 'method_not_enabled'; };
            IF NOT ($remaining[0] IN $methods) { THROW 'policy_would_be_empty'; };
            UPDATE browser_auth_policy SET methods = $remaining, epoch = epoch + 1, updated_at = time::now();
            CREATE type::record('local_admin_audit', $operation_id) SET
                id = $operation_id,
                event_time = time::now(),
                actor_kind = 'cli',
                actor_id = 'local_admin_cli',
                action = 'auth_method_removed',
                target_admin_id = NONE,
                target_account_id = NONE,
                target_tenant_id = NONE,
                target_key_id = NONE,
                target_method = $method,
                outcome = 'success',
                request_id = $operation_id;
            SELECT mode, epoch, methods FROM browser_auth_policy LIMIT 1;
            COMMIT TRANSACTION;";
        let rows = self
            .handle()
            .query_json_at(
                sql,
                Some(json!({
                    "method": method.as_str(),
                    "remaining": [remaining.as_str()],
                    "operation_id": operation_id,
                })),
                9,
            )
            .await
            .map_err(|error| {
                match thrown_token(
                    &error,
                    &[
                        "policy_absent",
                        "policy_ambiguous",
                        "method_not_enabled",
                        "policy_would_be_empty",
                    ],
                ) {
                    Some("method_not_enabled") => MemoryError::Conflict(format!(
                        "browser authentication method '{}' is not enabled by the durable policy",
                        method.as_str()
                    )),
                    Some("policy_would_be_empty") => MemoryError::Conflict(
                        "removing this method would leave no browser authentication method".into(),
                    ),
                    Some(_) => MemoryError::Storage(
                        "the durable browser-auth policy is absent or ambiguous; refusing to \
                         remove a method from a row that is not exactly one policy"
                            .into(),
                    ),
                    None => map_storage_error("remove browser auth method", error),
                }
            })?;
        let Some(row) = rows.into_iter().next() else {
            return Err(MemoryError::Storage(
                "remove_browser_auth_method returned no rows".into(),
            ));
        };
        let methods = policy_methods_from_row(&row).ok_or_else(|| {
            MemoryError::Storage(
                "policy row carries neither an enabled-method set nor a recognized mode".into(),
            )
        })?;
        let epoch = required_u64(&row, "epoch")?;
        Ok(BrowserPolicyFence { methods, epoch })
    }

    #[cfg(feature = "control-plane")]
    async fn create_oidc_account_bundle(
        &self,
        policy: &BrowserPolicyFence,
        account: &Account,
        tenant: &Tenant,
        identity: &ExternalIdentity,
    ) -> Result<(), MemoryError> {
        let account_status = serde_json::to_value(account.status)
            .map_err(|error| MemoryError::Storage(format!("encode account status: {error}")))?;
        let tenant_status = serde_json::to_value(tenant.status)
            .map_err(|error| MemoryError::Storage(format!("encode tenant status: {error}")))?;
        let (lease_assignment, lease_vars) =
            lease_write_assignment(tenant.provisioning_lease.as_ref());
        let retry_stage_assignment = if tenant.retry_stage.is_some() {
            "retry_stage = $retry_stage"
        } else {
            "retry_stage = NONE"
        };
        let script = format!(
            "BEGIN TRANSACTION;
            LET $policy = (SELECT mode, epoch, methods FROM browser_auth_policy LIMIT 2);
            IF array::len($policy) != 1 {{ THROW 'no_policy'; }};
            IF NOT ('oidc' IN $policy[0].methods ?? [$policy[0].mode]) {{ THROW 'mode_mismatch'; }};
            IF $policy[0].epoch != $expected_epoch {{ THROW 'epoch_mismatch'; }};
            CREATE type::record('account', $account_id) SET id = $account_id, status = $account_status, tenant_id = $tenant_id, created_at = type::datetime($account_created_at);
            CREATE type::record('tenant', $tenant_record_id) SET id = $tenant_record_id, status = $tenant_status, namespace_binding = $binding, plan_version = $plan_version, schema_version = $schema_version, {retry_stage_assignment}, {lease_assignment}, created_at = type::datetime($tenant_created_at), version = $version;
            CREATE type::record('external_identity', $identity_id) SET id = $identity_id, issuer = $issuer, subject_verifier = $subject_verifier, account_id = $identity_account_id, created_at = type::datetime($identity_created_at);
            COMMIT TRANSACTION;",
        );
        let mut vars = json!({
            "expected_epoch": policy.epoch,
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
            "account_created_at": account.created_at.to_rfc3339(),
            "tenant_created_at": tenant.created_at.to_rfc3339(),
            "version": tenant.version,
            "identity_id": identity.id,
            "issuer": identity.issuer,
            "subject_verifier": hex::encode(identity.subject_verifier.0),
            "identity_account_id": identity.account_id,
            "identity_created_at": identity.created_at.to_rfc3339(),
        });
        if let (Some(vars), Some(lease_vars)) = (vars.as_object_mut(), lease_vars.as_object()) {
            vars.extend(lease_vars.clone());
        }
        self.handle()
            .query_json(&script, Some(vars))
            .await
            .map_err(|error| map_storage_error("create OIDC account bundle", error))?;
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
        // One hot usage row per Tenant; allow enough attempts that
        // a burst of concurrent ingests each lands one clean write.
        for _attempt in 0..8 {
            // Create the usage row once. The conditional UPDATE below is the
            // admission operation; it increments counters only when every
            // quota predicate still holds at the datastore write point.
            let init = self.handle().query_json(
                    "UPSERT type::record($table, $tenant_id) SET tenant_id = $tenant_id, ingest_window_start = IF ingest_window_start IS NONE THEN type::datetime($now) ELSE ingest_window_start END, ingest_current_minute = IF ingest_current_minute IS NONE THEN 0 ELSE ingest_current_minute END, ingested_bytes = IF ingested_bytes IS NONE THEN 0 ELSE ingested_bytes END, episode_count = IF episode_count IS NONE THEN 0 ELSE episode_count END, open_app_sessions = IF open_app_sessions IS NONE THEN 0 ELSE open_app_sessions END, active_api_keys = IF active_api_keys IS NONE THEN 0 ELSE active_api_keys END, updated_at = time::now()",
                    Some(json!({"table": "usage", "tenant_id": tenant_id, "now": now.to_rfc3339()})),
                )
                .await
                .map_err(|error| map_storage_error("initialize ingest usage", error));
            if let Err(error) = init {
                if is_write_conflict(&error) {
                    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                    continue;
                }
                return Err(error);
            }
            let reserve = self
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
                .map_err(|error| map_storage_error("reserve ingest usage", error));
            let rows = match reserve {
                Ok(rows) => rows,
                Err(error) => {
                    if is_write_conflict(&error) {
                        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
                        continue;
                    }
                    return Err(error);
                }
            };
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
                "UPSERT type::record($table, $tenant_id) SET tenant_id = $tenant_id, ingest_window_start = type::datetime($window), ingest_current_minute = $count, ingested_bytes = $bytes, episode_count = $episodes, open_app_sessions = IF open_app_sessions IS NONE THEN 0 ELSE open_app_sessions END, active_api_keys = IF active_api_keys IS NONE THEN 0 ELSE active_api_keys END, updated_at = time::now()",
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
        policy: &BrowserPolicyFence,
        state_hash: &str,
        sealed_payload: &[u8],
        aead_nonce: &[u8; 12],
    ) -> Result<(), MemoryError> {
        let payload_b64 = base64_encode(sealed_payload);
        let nonce_arr: Vec<u8> = aead_nonce.to_vec();
        // Statement order: `BEGIN`(0) guard(1..=4) `CREATE`(5) `COMMIT`(6).
        let sql = format!(
            "BEGIN TRANSACTION;{OIDC_POLICY_GUARD}
            CREATE type::record($table, $id) SET state_hash = $state, sealed_payload = $payload, aead_nonce = $nonce, expires_at = type::datetime($expires_at), created_at = time::now();
            COMMIT TRANSACTION;",
        );
        self.handle()
            .query_json_at(
                &sql,
                Some(json!({
                    "expected_epoch": policy.epoch,
                    "table": "oidc_request",
                    "id": state_hash,
                    "state": state_hash,
                    "payload": payload_b64,
                    "nonce": nonce_arr,
                    "expires_at": (Utc::now() + chrono::Duration::minutes(10)).to_rfc3339(),
                })),
                5,
            )
            .await
            .map_err(|err| map_storage_error("store_oidc_request", err))?;
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn take_oidc_request(
        &self,
        policy: &BrowserPolicyFence,
        state_hash: &str,
    ) -> Result<Option<(Vec<u8>, [u8; 12])>, MemoryError> {
        // Statement order: `BEGIN`(0) guard(1..=4) `DELETE`(5) `COMMIT`(6).
        let sql = format!(
            "BEGIN TRANSACTION;{OIDC_POLICY_GUARD}
            DELETE type::record($table, $id) WHERE state_hash = $state AND expires_at > time::now() RETURN BEFORE;
            COMMIT TRANSACTION;",
        );
        let rows = self
            .handle()
            .query_json_at(
                &sql,
                Some(json!({
                    "expected_epoch": policy.epoch,
                    "table": "oidc_request",
                    "id": state_hash,
                    "state": state_hash,
                })),
                5,
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
        policy: &BrowserPolicyFence,
        session: &crate::control::session::ControlPlaneSession,
    ) -> Result<(), MemoryError> {
        if session.browser_policy_epoch != Some(policy.epoch) {
            return Err(MemoryError::Conflict(
                "session epoch does not match the durable policy".into(),
            ));
        }
        // Statement order: `BEGIN`(0) guard(1..=4) `CREATE`(5) `COMMIT`(6).
        let sql = format!(
            "BEGIN TRANSACTION;{OIDC_POLICY_GUARD}
            CREATE type::record($table, $id) SET id = $id, cookie_hash = $cookie_hash, account_id = $account_id, browser_policy_epoch = $expected_epoch, auth_time = type::datetime($auth_time), idle_expiry = type::datetime($idle_expiry), absolute_expiry = type::datetime($absolute_expiry);
            COMMIT TRANSACTION;",
        );
        self.handle()
            .query_json_at(
                &sql,
                Some(json!({
                    "expected_epoch": policy.epoch,
                    "table": "control_plane_session",
                    "id": session.id,
                    "cookie_hash": session.cookie_hash,
                    "account_id": session.account_id,
                    "auth_time": session.auth_time.to_rfc3339(),
                    "idle_expiry": session.idle_expiry.to_rfc3339(),
                    "absolute_expiry": session.absolute_expiry.to_rfc3339(),
                })),
                5,
            )
            .await
            .map_err(|err| map_storage_error("store_session", err))?;
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn find_session(
        &self,
        policy: &BrowserPolicyFence,
        cookie_hash: &str,
    ) -> Result<Option<crate::control::session::ControlPlaneSession>, MemoryError> {
        // Statement order: `BEGIN`(0) guard(1..=4) `SELECT`(5) `COMMIT`(6).
        // The `browser_policy_epoch` predicate excludes sessions created
        // before the epoch column existed (decode as `NONE`), so an
        // upgraded deployment requires a fresh login.
        let sql = format!(
            "BEGIN TRANSACTION;{OIDC_POLICY_GUARD}
            SELECT * FROM type::table($table) WHERE cookie_hash = $cookie AND browser_policy_epoch = $expected_epoch AND idle_expiry > time::now() AND absolute_expiry > time::now() LIMIT 1;
            COMMIT TRANSACTION;",
        );
        let rows = self
            .handle()
            .query_json_at(
                &sql,
                Some(json!({
                    "expected_epoch": policy.epoch,
                    "table": "control_plane_session",
                    "cookie": cookie_hash,
                })),
                5,
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
            browser_policy_epoch: row.get("browser_policy_epoch").and_then(Value::as_u64),
            auth_time: required_datetime(&row, "auth_time")?,
            idle_expiry: required_datetime(&row, "idle_expiry")?,
            absolute_expiry: required_datetime(&row, "absolute_expiry")?,
        };
        if session.idle_expiry <= Utc::now()
            || session.absolute_expiry <= Utc::now()
            || session.browser_policy_epoch != Some(policy.epoch)
        {
            return Ok(None);
        }
        Ok(Some(session))
    }

    #[cfg(feature = "control-plane")]
    async fn touch_session(
        &self,
        policy: &BrowserPolicyFence,
        session_id: &str,
        cookie_hash: &str,
    ) -> Result<(), MemoryError> {
        // Statement order: `BEGIN`(0) guard(1..=4) `UPDATE`(5) `COMMIT`(6).
        // The idle deadline is computed from database time; the `WHERE`
        // clause refuses to extend a missing, expired or epoch-stale row,
        // and `UPDATE` never recreates a row.
        let sql = format!(
            "BEGIN TRANSACTION;{OIDC_POLICY_GUARD}
            UPDATE type::record($table, $id) SET idle_expiry = IF time::now() + 1800s < absolute_expiry THEN time::now() + 1800s ELSE absolute_expiry END WHERE cookie_hash = $cookie AND browser_policy_epoch = $expected_epoch AND idle_expiry > time::now() AND absolute_expiry > time::now() RETURN AFTER;
            COMMIT TRANSACTION;",
        );
        let rows = self
            .handle()
            .query_json_at(
                &sql,
                Some(json!({
                    "expected_epoch": policy.epoch,
                    "table": "control_plane_session",
                    "id": session_id,
                    "cookie": cookie_hash,
                })),
                5,
            )
            .await
            .map_err(|error| map_storage_error("touch session", error))?;
        if rows.is_empty() {
            return Err(MemoryError::NotFound("session not found".into()));
        }
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn delete_session(
        &self,
        policy: &BrowserPolicyFence,
        cookie_hash: &str,
    ) -> Result<(), MemoryError> {
        // Statement order: `BEGIN`(0) guard(1..=4) `DELETE`(5) `COMMIT`(6).
        let sql = format!(
            "BEGIN TRANSACTION;{OIDC_POLICY_GUARD}
            DELETE FROM control_plane_session WHERE cookie_hash = $cookie_hash AND browser_policy_epoch = $expected_epoch RETURN BEFORE;
            COMMIT TRANSACTION;",
        );
        let rows = self
            .handle()
            .query_json_at(
                &sql,
                Some(json!({
                    "expected_epoch": policy.epoch,
                    "cookie_hash": cookie_hash,
                })),
                5,
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

#[allow(dead_code, clippy::all)]
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

#[cfg(feature = "control-plane")]
mod local_admin;
#[cfg(feature = "control-plane")]
pub mod local_admin_rate;
#[cfg(feature = "control-plane")]
pub use local_admin_rate::rate_bucket_cleanup_scheduler_job;
#[cfg(all(test, feature = "control-plane"))]
mod local_admin_remote;

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

    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn query_json_at_extracts_correct_index() {
        let db = Arc::new(Surreal::new::<Mem>(()).await.expect("create mem db"));
        db.use_ns("test_qja")
            .use_db("test_qja")
            .await
            .expect("use ns/db");
        let registry_db = RegistryDb::Local(db);
        let sql = "LET $a = 'first'; LET $b = 'second'; RETURN $b;";
        let result = registry_db
            .as_dyn()
            .query_json_at(sql, None, 2)
            .await
            .expect("query_json_at");
        assert!(!result.is_empty());
    }

    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn query_json_at_fails_on_bad_index() {
        let db = Arc::new(Surreal::new::<Mem>(()).await.expect("create mem db"));
        db.use_ns("test_qja2")
            .use_db("test_qja2")
            .await
            .expect("use ns/db");
        let registry_db = RegistryDb::Local(db);
        let sql = "LET $a = 'first'; RETURN $a;";
        let result = registry_db.as_dyn().query_json_at(sql, None, 5).await;
        assert!(result.is_err());
    }

    /// `reconcile_browser_policy` creates the singleton on first call,
    /// returns the same fence on every later call, and never narrows the set.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn reconcile_creates_singleton_extends_and_is_idempotent() {
        use crate::http::config::BrowserAuthMethod;
        let namespace = format!("join_oidc_{}", uuid::Uuid::new_v4().simple());
        let store = SurrealRegistryStore::connect_in_memory(&namespace, "registry")
            .await
            .expect("migrated in-memory registry");
        let oidc_only = [BrowserAuthMethod::Oidc];
        let first = store
            .reconcile_browser_policy(&oidc_only, None)
            .await
            .expect("reconcile browser policy");
        assert_eq!(first.methods, vec![BrowserAuthMethod::Oidc]);
        assert_eq!(first.epoch, 1);
        let second = store
            .reconcile_browser_policy(&oidc_only, None)
            .await
            .expect("idempotent reconcile");
        assert_eq!(second.methods, first.methods);
        assert_eq!(second.epoch, first.epoch);
        // Adding the local method extends the same row rather than replacing
        // it, which is what makes adding a provider an environment change.
        let extended = store
            .reconcile_browser_policy(
                &[BrowserAuthMethod::Oidc, BrowserAuthMethod::Local],
                Some(LocalKeyFingerprints {
                    session: [0x11; 32],
                    csrf: [0x22; 32],
                }),
            )
            .await
            .expect("extend the policy with local");
        assert_eq!(
            extended.methods,
            vec![BrowserAuthMethod::Local, BrowserAuthMethod::Oidc]
        );
        assert_eq!(extended.epoch, first.epoch, "extending keeps the epoch");
    }

    /// Removing a method is an explicit guarded operation, not a
    /// reconciliation: a configuration that omits a method the row enables
    /// must fail startup rather than narrow the row.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn reconcile_refuses_to_remove_a_durably_enabled_method() {
        use crate::http::config::BrowserAuthMethod;
        let namespace = format!("join_oidc_local_{}", uuid::Uuid::new_v4().simple());
        let store = SurrealRegistryStore::connect_in_memory(&namespace, "registry")
            .await
            .expect("migrated in-memory registry");
        store
            .reconcile_browser_policy(
                &[BrowserAuthMethod::Local],
                Some(LocalKeyFingerprints {
                    session: [0x11; 32],
                    csrf: [0x22; 32],
                }),
            )
            .await
            .expect("reconcile a local-only policy");
        let refused = store
            .reconcile_browser_policy(&[BrowserAuthMethod::Oidc], None)
            .await;
        assert!(
            matches!(refused, Err(MemoryError::Conflict(_))),
            "an OIDC-only configuration over a local policy must fail, got {refused:?}"
        );
    }

    #[cfg(test)]
    mod storage_error_classification {
        use super::*;

        /// SurrealDB spells a uniqueness violation two ways, and the adapter has
        /// to read both as a `Conflict`: a record insert says "already exists",
        /// while a unique index says "Database index `x` already contains …".
        /// Missing the second spelling reported a lost race to the caller as an
        /// infrastructure failure.
        #[test]
        fn both_unique_violation_spellings_are_conflicts() {
            let messages = [
                "Database index `idx_external_identity_issuer_subject` already contains \
                 ['https://issuer.example', 'aa'], with record `external_identity:idn_one`",
                "Database record `external_identity:idn_one` already exists",
            ];
            for message in messages {
                let mapped = map_storage_error("link external identity", message);
                assert!(
                    matches!(mapped, MemoryError::Conflict(_)),
                    "{message:?} must classify as a conflict, got {mapped:?}"
                );
                assert!(
                    is_conflict_error(&mapped),
                    "the classifier and the detector must agree on {message:?}"
                );
            }
        }

        /// The agreement above must not come from classifying everything as a
        /// conflict: a genuine transport failure keeps its own category.
        #[test]
        fn other_failures_keep_their_category() {
            let mapped = map_storage_error("read policy", "connection reset by peer");
            assert!(matches!(mapped, MemoryError::Storage(_)), "got {mapped:?}");
            assert!(!is_conflict_error(&mapped));

            let missing = map_storage_error("read policy", "no record found");
            assert!(
                matches!(missing, MemoryError::NotFound(_)),
                "got {missing:?}"
            );
            assert!(!is_conflict_error(&missing));
        }
    }

    /// A migrated registry whose durable policy already enables `methods`.
    async fn policy_store(
        methods: &[crate::http::config::BrowserAuthMethod],
    ) -> SurrealRegistryStore {
        let namespace = format!("auth_policy_{}", uuid::Uuid::new_v4().simple());
        let store = SurrealRegistryStore::connect_in_memory(&namespace, "registry")
            .await
            .expect("migrated in-memory registry");
        let local = methods
            .contains(&crate::http::config::BrowserAuthMethod::Local)
            .then_some(LocalKeyFingerprints {
                session: [0x11; 32],
                csrf: [0x22; 32],
            });
        store
            .reconcile_browser_policy(methods, local)
            .await
            .expect("reconcile browser policy");
        store
    }

    /// An operator-action audit row, read through the same handle the production
    /// statements use.
    async fn removal_audit_rows(store: &SurrealRegistryStore) -> Vec<Value> {
        store
            .handle()
            .query_json(
                "SELECT actor_kind, actor_id, action, outcome, target_method FROM local_admin_audit \
                 WHERE action = 'auth_method_removed'",
                None,
            )
            .await
            .expect("read removal audit rows")
    }

    /// ADR-0057: the guarded removal narrows the durable set, advances the epoch
    /// and records the operator action, all as one write — and afterwards the
    /// configuration the operator was told to deploy is accepted at startup.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn removing_a_method_narrows_the_policy_and_audits_it() {
        use crate::http::config::BrowserAuthMethod;
        let store = policy_store(&[BrowserAuthMethod::Local, BrowserAuthMethod::Oidc]).await;

        let fence = store
            .remove_browser_auth_method(BrowserAuthMethod::Local)
            .await
            .expect("remove the local method");
        assert_eq!(fence.methods, vec![BrowserAuthMethod::Oidc]);
        assert_eq!(fence.epoch, 2, "a policy change advances the epoch");

        let rows = removal_audit_rows(&store).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["actor_kind"], "cli");
        assert_eq!(rows[0]["actor_id"], "local_admin_cli");
        assert_eq!(rows[0]["outcome"], "success");
        assert_eq!(rows[0]["target_method"], "local");

        // This is the whole point of the operation: the narrowed configuration
        // now reconciles instead of failing startup.
        let after_restart = store
            .reconcile_browser_policy(&[BrowserAuthMethod::Oidc], None)
            .await
            .expect("an SSO-only configuration must be accepted once local is removed");
        assert_eq!(after_restart.methods, vec![BrowserAuthMethod::Oidc]);
        assert_eq!(
            after_restart.epoch, fence.epoch,
            "reconciling an unchanged set keeps the epoch"
        );
    }

    /// Removing the only enabled method would leave a control plane that
    /// authenticates nobody, and removing one that is not enabled is not the
    /// operator's intent. Neither may write anything.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn removal_refuses_to_empty_the_policy_or_remove_an_absent_method() {
        use crate::http::config::BrowserAuthMethod;
        let store = policy_store(&[BrowserAuthMethod::Local]).await;

        let refusal = store
            .remove_browser_auth_method(BrowserAuthMethod::Local)
            .await;
        assert!(
            matches!(refusal, Err(MemoryError::Conflict(_))),
            "the last method must not be removable, got {refusal:?}"
        );
        let refusal = store
            .remove_browser_auth_method(BrowserAuthMethod::Oidc)
            .await;
        assert!(
            matches!(refusal, Err(MemoryError::Conflict(_))),
            "an absent method must be refused, got {refusal:?}"
        );
        assert!(
            removal_audit_rows(&store).await.is_empty(),
            "a refusal must not be audited as a removal"
        );

        let policy = store
            .reconcile_browser_policy(
                &[BrowserAuthMethod::Local],
                Some(LocalKeyFingerprints {
                    session: [0x11; 32],
                    csrf: [0x22; 32],
                }),
            )
            .await
            .expect("the policy is unchanged");
        assert_eq!(policy.methods, vec![BrowserAuthMethod::Local]);
        assert_eq!(
            policy.epoch, 1,
            "a refused removal must not advance the epoch"
        );
    }

    /// The OIDC flow operations that the black-box suite bypasses
    /// (it seeds sessions directly) are exercised here on the durable
    /// store: the policy guard parses, the documented result indices
    /// decode, consumption is one-use, and a stale fence is rejected.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn oidc_flow_operations_are_policy_guarded() {
        use crate::http::config::BrowserAuthMethod;
        use crate::http::registry::models::BrowserPolicyFence;

        let namespace = format!("oidc_flow_{}", uuid::Uuid::new_v4().simple());
        let store = SurrealRegistryStore::connect_in_memory(&namespace, "registry")
            .await
            .expect("migrated in-memory registry");
        let policy = store
            .reconcile_browser_policy(&[BrowserAuthMethod::Oidc], None)
            .await
            .expect("reconcile browser policy");

        // store -> take is a one-use round-trip.
        store
            .store_oidc_request(&policy, "state_hash_1", b"sealed", &[7u8; 12])
            .await
            .expect("store OIDC request");
        let (payload, nonce) = store
            .take_oidc_request(&policy, "state_hash_1")
            .await
            .expect("take OIDC request")
            .expect("request present");
        assert_eq!(payload, b"sealed");
        assert_eq!(nonce, [7u8; 12]);
        assert!(
            store
                .take_oidc_request(&policy, "state_hash_1")
                .await
                .expect("second take")
                .is_none(),
            "an OIDC request is consumed exactly once"
        );

        // A stale or non-OIDC fence aborts the transaction.
        let stale = BrowserPolicyFence {
            methods: vec![BrowserAuthMethod::Oidc],
            epoch: policy.epoch + 1,
        };
        assert!(
            store
                .store_oidc_request(&stale, "state_hash_2", b"x", &[1u8; 12])
                .await
                .is_err(),
            "a stale fence must not write an OIDC request"
        );
        assert!(
            store
                .take_oidc_request(&stale, "state_hash_2")
                .await
                .is_err(),
            "a stale fence must not consume an OIDC request"
        );

        // Session delete is guarded by the current epoch.
        let now = Utc::now();
        let session = crate::control::session::ControlPlaneSession {
            id: "ses_flow".into(),
            cookie_hash: "cookie_flow".into(),
            account_id: "acct_flow".into(),
            browser_policy_epoch: Some(policy.epoch),
            auth_time: now,
            idle_expiry: now + chrono::Duration::minutes(30),
            absolute_expiry: now + chrono::Duration::hours(1),
        };
        store
            .store_session(&policy, &session)
            .await
            .expect("store session");
        assert!(
            store
                .find_session(&policy, "cookie_flow")
                .await
                .expect("find session")
                .is_some()
        );
        assert!(
            store.delete_session(&stale, "cookie_flow").await.is_err(),
            "a stale fence must not delete a session"
        );
        store
            .delete_session(&policy, "cookie_flow")
            .await
            .expect("delete session");
        assert!(
            store
                .find_session(&policy, "cookie_flow")
                .await
                .expect("find deleted session")
                .is_none(),
            "delete removes the session row"
        );
    }

    /// The deliberate OIDC compatibility break: a session written before
    /// the epoch column existed decodes as `None`, and a session carrying
    /// a stale epoch is excluded, so neither can authenticate. Both rows
    /// are planted with raw SQL because `store_session` refuses to write
    /// either shape.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn find_session_rejects_legacy_and_stale_epoch_rows() {
        let namespace = format!("session_epoch_{}", uuid::Uuid::new_v4().simple());
        let store = SurrealRegistryStore::connect_in_memory(&namespace, "registry")
            .await
            .expect("migrated in-memory registry");
        let policy = store
            .reconcile_browser_policy(&[crate::http::config::BrowserAuthMethod::Oidc], None)
            .await
            .expect("reconcile browser policy");
        store
            .handle()
            .query_json(
                "CREATE type::record('control_plane_session', $id) SET id = $id, cookie_hash = $cookie, account_id = 'acct_legacy', auth_time = time::now(), idle_expiry = time::now() + 1800s, absolute_expiry = time::now() + 86400s",
                Some(json!({"id": "ses_legacy", "cookie": "cookie_legacy"})),
            )
            .await
            .expect("create legacy session");
        store
            .handle()
            .query_json(
                "CREATE type::record('control_plane_session', $id) SET id = $id, cookie_hash = $cookie, account_id = 'acct_stale', browser_policy_epoch = 99, auth_time = time::now(), idle_expiry = time::now() + 1800s, absolute_expiry = time::now() + 86400s",
                Some(json!({"id": "ses_stale", "cookie": "cookie_stale"})),
            )
            .await
            .expect("create stale session");
        assert!(
            store
                .find_session(&policy, "cookie_legacy")
                .await
                .expect("legacy lookup")
                .is_none(),
            "a session without an epoch must not resolve"
        );
        assert!(
            store
                .find_session(&policy, "cookie_stale")
                .await
                .expect("stale lookup")
                .is_none(),
            "a session with a stale epoch must not resolve"
        );
    }

    /// A parse error echoes the transaction source, so it must never be
    /// mistaken for the guard's refusal: the script throws the token assembled
    /// from split literals, and only a real `THROW` carries it.
    #[test]
    fn a_parse_error_echo_is_not_a_replace_refusal() {
        let echoed = MemoryError::Storage(
            "query statement errors: statement 4: Parse error at `IF array::len($held) != 1 { \
             THROW string::concat('replace_', 'guard'); }`"
                .into(),
        );
        let classified = classify_identity_change_error("replace external identity", echoed);
        assert!(
            !matches!(classified, MemoryError::Conflict(_)),
            "an infrastructure failure must not read as a refusal"
        );
    }

    /// The invitation remediation against the real engine: the swap is one
    /// transaction, both audit rows land with it, and the Account never has
    /// zero identities.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn replace_external_identity_swaps_in_one_transaction() {
        let store = identity_change_store().await;
        let old = linked_identity("idn_old", 0x01);
        store
            .link_external_identity(&old, &IdentityAudit::by_operator("admin_root", Utc::now()))
            .await
            .expect("link the mis-bound identity");
        let new = linked_identity("idn_new", 0x02);
        store
            .replace_external_identity(&new, &IdentityAudit::by_operator("admin_root", Utc::now()))
            .await
            .expect("replace through the guarded transaction");

        let held = store
            .find_external_identities("acct_shared")
            .await
            .expect("list");
        assert_eq!(held.len(), 1, "the Account never has zero identities");
        assert_eq!(held[0].id, "idn_new");
        let unlinked = audit_rows(&store, "identity_unlinked").await;
        assert!(
            unlinked
                .iter()
                .any(|row| row["target_identity_id"] == "idn_old"),
            "the removal is audited: {unlinked:?}"
        );
        let linked = audit_rows(&store, "identity_linked").await;
        assert!(
            linked
                .iter()
                .any(|row| row["target_identity_id"] == "idn_new"),
            "the addition is audited: {linked:?}"
        );
    }

    /// The exactly-one guard against the real engine.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn replace_external_identity_requires_exactly_one_existing() {
        let store = identity_change_store().await;
        let new = linked_identity("idn_new", 0x03);
        let refused = store
            .replace_external_identity(&new, &IdentityAudit::by_operator("admin_root", Utc::now()))
            .await;
        assert!(matches!(refused, Err(MemoryError::Conflict(_))));
    }

    /// A migrated control namespace with one Active Account, which is all an
    /// identity change needs.
    async fn identity_change_store() -> SurrealRegistryStore {
        let namespace = format!("identity_audit_{}", uuid::Uuid::new_v4().simple());
        let store = SurrealRegistryStore::connect_in_memory(&namespace, "registry")
            .await
            .expect("migrated Mem registry");
        store
            .ensure_plan(&crate::http::registry::models::Plan::default())
            .await
            .expect("ensure default plan");
        store
            .create_account_bundle(&account(), &tenant(), None)
            .await
            .expect("create account bundle");
        store
    }

    fn linked_identity(id: &str, tag: u8) -> ExternalIdentity {
        ExternalIdentity {
            id: id.into(),
            issuer: "https://issuer.example".into(),
            subject_verifier: SubjectVerifier([tag; 32]),
            account_id: "acct_shared".into(),
            created_at: Utc::now(),
        }
    }

    /// Durable audit rows carrying `action`, read through the same handle the
    /// production statements use.
    async fn audit_rows(store: &SurrealRegistryStore, action: &str) -> Vec<Value> {
        store
            .handle()
            .query_json(
                "SELECT account_id, actor_kind, actor_principal, action, target_identity_id, correlation_id FROM audit_event WHERE action = $action",
                Some(json!({"action": action})),
            )
            .await
            .expect("read audit rows")
    }

    /// ADR-0057: an identity change and its audit row are one durable write, and
    /// the row names the Account, the actor and the identity — which is what
    /// keeps the change readable once the identity itself has been deleted.
    #[tokio::test]
    async fn identity_changes_append_audit_rows_durably() {
        let store = identity_change_store().await;
        let audit = IdentityAudit::by_account("acct_shared", Utc::now());

        store
            .link_external_identity(&linked_identity("idn_one", 0x11), &audit)
            .await
            .expect("link first identity");
        let rows = audit_rows(&store, "identity_linked").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["account_id"], "acct_shared");
        assert_eq!(rows[0]["actor_kind"], "account");
        assert_eq!(rows[0]["actor_principal"], "acct_shared");
        assert_eq!(rows[0]["target_identity_id"], "idn_one");
        assert_eq!(rows[0]["correlation_id"], "identity_linked_idn_one");

        store
            .link_external_identity(&linked_identity("idn_two", 0x22), &audit)
            .await
            .expect("link second identity");
        store
            .unlink_external_identity("acct_shared", "idn_one", &audit)
            .await
            .expect("unlink one of two");

        let remaining = store
            .find_external_identities("acct_shared")
            .await
            .expect("list identities");
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].id, "idn_two");
        let rows = audit_rows(&store, "identity_unlinked").await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["target_identity_id"], "idn_one");
        assert_eq!(rows[0]["account_id"], "acct_shared");
    }

    /// A refused identity change leaves nothing behind: the transaction that
    /// would have appended the row is the one that was rolled back, so neither a
    /// duplicate link nor a refused removal can produce a row that describes a
    /// change that did not happen.
    #[tokio::test]
    async fn a_refused_identity_change_appends_no_audit_row() {
        let store = identity_change_store().await;
        let audit = IdentityAudit::by_account("acct_shared", Utc::now());
        let first = linked_identity("idn_one", 0x11);
        store
            .link_external_identity(&first, &audit)
            .await
            .expect("link first identity");

        // Same (issuer, subject_verifier) under a different row id: the durable
        // unique index is what refuses it.
        let duplicate = linked_identity("idn_duplicate", 0x11);
        let refused = store.link_external_identity(&duplicate, &audit).await;
        assert!(
            matches!(refused, Err(MemoryError::Conflict(_))),
            "a duplicate identity tuple must be refused, got {refused:?}"
        );
        assert_eq!(audit_rows(&store, "identity_linked").await.len(), 1);

        let refused = store
            .unlink_external_identity("acct_shared", "idn_one", &audit)
            .await;
        assert!(
            matches!(refused, Err(MemoryError::Conflict(_))),
            "the last identity must not be removable, got {refused:?}"
        );
        assert!(audit_rows(&store, "identity_unlinked").await.is_empty());
        assert_eq!(
            store
                .find_external_identities("acct_shared")
                .await
                .expect("list identities")
                .len(),
            1,
            "the refusal must not have removed anything"
        );
    }
}
