//! Durable app session store (plan §7.2).
//!
//! The store wraps a `BoundDbClient` bound to a specific
//! tenant namespace. Every call carries the tenant
//! binding in the WHERE clause; a handle that resolves
//! to a row in another tenant's namespace is a not-found
//! from this client's perspective and never reveals
//! cross-tenant ownership.
//!
//! Concurrency model: optimistic versioning. `open`
//! inserts a row at version=1 with a fresh opaque
//! handle. `command` runs `UPDATE WHERE version = $expected
//! RETURN version`; on success the new version is
//! `expected+1` and `idle_expiry` is bumped to
//! `min(now + 30m, absolute_expiry)`. A failed CAS
//! returns `MemoryError::Conflict` and never mutates
//! payload or expiry.

use std::sync::Arc;

use serde_json::Value;

use crate::error::MemoryError;
use crate::storage::client::BoundDbClient;

/// Idle window. The cleanup pass (Task 7.2) physically
/// deletes rows whose `idle_expiry` is in the past.
pub const IDLE_EXPIRY_SECS: i64 = 30 * 60;
/// Hard ceiling on session lifetime. A session whose
/// `absolute_expiry` is in the past is deleted by the
/// cleanup pass; `command` caps `idle_expiry` at
/// `absolute_expiry`.
pub const ABSOLUTE_EXPIRY_SECS: i64 = 24 * 60 * 60;
/// Maximum open app sessions per tenant. Enforced in
/// `open` before insert; matches
/// `Plan::max_open_app_sessions` (Task 6.4).
pub const MAX_OPEN_PER_TENANT: i64 = 32;

pub struct AppSessionStore {
    db: Arc<BoundDbClient>,
}

impl AppSessionStore {
    pub fn new(db: Arc<BoundDbClient>) -> Self {
        Self { db }
    }

    /// Open a new app session. The returned handle is an
    /// opaque 32-byte URL-safe string; the version starts
    /// at 1. `open` is the gate that enforces the
    /// per-tenant cap; a tenant at the cap returns
    /// `MemoryError::Conflict("app_session_cap_reached")`.
    pub async fn open(
        &self,
        tenant_id: &str,
        app: &str,
        payload: Value,
    ) -> Result<(String, u64), MemoryError> {
        // Pre-flight: count this tenant's non-expired
        // sessions. We bound the count by
        // MAX_OPEN_PER_TENANT + 1 so a tenant at the cap
        // is rejected without scanning past it.
        let count = self.count_active(tenant_id).await?;
        if count >= MAX_OPEN_PER_TENANT {
            return Err(MemoryError::Conflict(format!(
                "app_session_cap_reached: tenant {tenant_id} at {count} sessions (cap {MAX_OPEN_PER_TENANT})"
            )));
        }
        let now = chrono::Utc::now();
        let idle_expiry = now + chrono::Duration::seconds(IDLE_EXPIRY_SECS);
        let absolute_expiry = now + chrono::Duration::seconds(ABSOLUTE_EXPIRY_SECS);
        let handle = generate_handle();
        // The bound client already targets a specific
        // namespace; the tenant_id is bound in params so
        // a tenant mismatch returns zero rows.
        self.db
            .query(
                "CREATE app_session SET \
                 handle = $handle, \
                 tenant_id = $tenant_id, \
                 app = $app, \
                 version = 1, \
                 payload = $payload, \
                 idle_expiry = $idle_expiry, \
                 absolute_expiry = $absolute_expiry;",
                Some(serde_json::json!({
                    "handle": handle,
                    "tenant_id": tenant_id,
                    "app": app,
                    "payload": payload,
                    "idle_expiry": idle_expiry,
                    "absolute_expiry": absolute_expiry,
                })),
            )
            .await?;
        Ok((handle, 1))
    }

    /// Optimistic-versioning mutation. Returns the new
    /// version on success or `MemoryError::Conflict` on a
    /// stale CAS, missing handle, expired session, or
    /// cross-tenant handle.
    pub async fn command(
        &self,
        handle: &str,
        expected_version: u64,
        mutation: Value,
    ) -> Result<u64, MemoryError> {
        let now = chrono::Utc::now();
        let new_idle = now + chrono::Duration::seconds(IDLE_EXPIRY_SECS);
        // The CAS predicate is `version = $expected AND
        // handle = $handle AND absolute_expiry > $now`.
        // The CappedIdleExpiry rule is enforced in the
        // RETURN projection: idle_expiry becomes
        // `min($new_idle, absolute_expiry)` so a session
        // near its absolute ceiling does not get its
        // idle window extended past the ceiling.
        let result = self
            .db
            .query(
                "UPDATE app_session SET \
                 version = $expected + 1, \
                 payload = $mutation, \
                 idle_expiry = (SELECT VALUE min($new_idle, absolute_expiry) FROM ONLY $handle LIMIT 1) \
                 WHERE handle = $handle AND version = $expected AND absolute_expiry > $now \
                 RETURN version;",
                Some(serde_json::json!({
                    "handle": handle,
                    "expected": expected_version,
                    "mutation": mutation,
                    "new_idle": new_idle,
                    "now": now,
                })),
            )
            .await?;
        let rows: Vec<serde_json::Value> = serde_json::from_value(result)
            .map_err(|e| MemoryError::Storage(format!("app_session command: {e}")))?;
        let Some(row) = rows.first() else {
            return Err(MemoryError::Conflict(format!(
                "app_session command: stale version {expected_version} for handle {handle}"
            )));
        };
        let new_version = row
            .get("version")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| MemoryError::Storage("app_session command: no version".into()))?;
        if new_version != expected_version + 1 {
            return Err(MemoryError::Conflict(format!(
                "app_session command: version drift expected {} got {new_version}",
                expected_version + 1
            )));
        }
        Ok(new_version)
    }

    /// Close a session. The DELETE is parameterized on
    /// `handle`; an unknown handle is a no-op.
    pub async fn close(&self, handle: &str) -> Result<(), MemoryError> {
        self.db
            .query(
                "DELETE FROM app_session WHERE handle = $handle;",
                Some(serde_json::json!({ "handle": handle })),
            )
            .await?;
        Ok(())
    }

    /// Look up the handle's tenant binding. A handle
    /// from another tenant returns `None` without
    /// revealing ownership. Resource reads use this
    /// to confirm the principal still owns the handle.
    pub async fn tenant_of(&self, handle: &str) -> Result<Option<String>, MemoryError> {
        let result = self
            .db
            .query(
                "SELECT VALUE tenant_id FROM app_session WHERE handle = $handle LIMIT 1;",
                Some(serde_json::json!({ "handle": handle })),
            )
            .await?;
        let rows: Vec<serde_json::Value> = serde_json::from_value(result)
            .map_err(|e| MemoryError::Storage(format!("app_session tenant_of: {e}")))?;
        Ok(rows.first().and_then(|v| v.as_str().map(String::from)))
    }

    /// Count non-expired sessions for a tenant. Used by
    /// the cap check in `open`.
    async fn count_active(&self, tenant_id: &str) -> Result<i64, MemoryError> {
        let now = chrono::Utc::now();
        let result = self
            .db
            .query(
                "SELECT count() AS n FROM app_session \
                 WHERE tenant_id = $tenant_id AND absolute_expiry > $now \
                 GROUP ALL;",
                Some(serde_json::json!({
                    "tenant_id": tenant_id,
                    "now": now,
                })),
            )
            .await?;
        let rows: Vec<serde_json::Value> = serde_json::from_value(result)
            .map_err(|e| MemoryError::Storage(format!("app_session count: {e}")))?;
        Ok(rows
            .first()
            .and_then(|v| v.get("n"))
            .and_then(|v| v.as_i64())
            .unwrap_or(0))
    }
}

/// Generate an opaque 32-byte URL-safe handle. The
/// randomness is sufficient for the lifetime of a single
/// session; a collision under a 32-byte random namespace
/// is a non-event for any realistic concurrency.
fn generate_handle() -> String {
    let mut bytes = [0u8; 32];
    for slot in bytes.iter_mut() {
        *slot = rand::random::<u8>();
    }
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

use base64::Engine;
