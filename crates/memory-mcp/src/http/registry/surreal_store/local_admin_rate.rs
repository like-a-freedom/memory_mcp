//! Durable rate bucket operations for local admin authentication.
//!
//! Manages throttling and denied aggregate counts using the
//! `local_admin_rate_bucket` table.

use serde_json::json;

use crate::error::MemoryError;
use crate::service::local_admin::contracts::{LocalAdminError, LocalResult};

use super::SurrealRegistryStore;

/// Convert a `MemoryError` to `LocalAdminError::Infrastructure`.
fn infra(e: MemoryError) -> LocalAdminError {
    LocalAdminError::Infrastructure(e)
}

/// Rate bucket operations for the durable store.
impl SurrealRegistryStore {
    /// Reserve a rate bucket entry atomically. Returns the current count
    /// within the window for the given source/action combination.
    pub async fn reserve_rate_bucket(
        &self,
        source_bucket: u16,
        action: &str,
        window_secs: u64,
    ) -> LocalResult<u64> {
        let sql = "
            BEGIN TRANSACTION;
            INSERT INTO local_admin_rate_bucket SET
                source_bucket = $source_bucket,
                action = $action,
                created_at = time::now();
            LET $count = (
                SELECT count() AS cnt FROM local_admin_rate_bucket
                WHERE source_bucket = $source_bucket
                AND action = $action
                AND created_at > time::now() - ${window}s
            );
            COMMIT TRANSACTION;
            RETURN $count;
        ";

        let result = self
            .db
            .as_dyn()
            .query_json(
                sql,
                Some(json!({
                    "source_bucket": source_bucket,
                    "action": action,
                    "window": window_secs,
                })),
            )
            .await
            .map_err(infra)?;

        let count = result
            .first()
            .and_then(|v| v.as_array()?.first())
            .and_then(|v| v.get("cnt")?.as_u64())
            .unwrap_or(0);

        Ok(count)
    }

    /// Increment a denied aggregate slot. These are saturating counters
    /// for fixed action/reason combinations, separate from the per-attempt
    /// rate bucket rows.
    pub async fn increment_denied_aggregate(
        &self,
        source_bucket: u16,
        action: &str,
        reason: &str,
    ) -> LocalResult<()> {
        let sql = "
            UPSERT local_admin_rate_bucket SET
                source_bucket = $source_bucket,
                action = $action,
                reason = $reason,
                denied_count = math::max(0, denied_count + 1),
                updated_at = time::now();
        ";

        self.db
            .as_dyn()
            .query_json(
                sql,
                Some(json!({
                    "source_bucket": source_bucket,
                    "action": action,
                    "reason": reason,
                })),
            )
            .await
            .map_err(infra)?;

        Ok(())
    }

    /// Clean up expired rate bucket entries.
    pub async fn cleanup_rate_buckets(&self, older_than_secs: u64) -> LocalResult<()> {
        let sql = "DELETE local_admin_rate_bucket WHERE created_at < time::now() - ${window}s;";

        self.db
            .as_dyn()
            .query_json(sql, Some(json!({"window": older_than_secs})))
            .await
            .map_err(infra)?;

        Ok(())
    }
}
