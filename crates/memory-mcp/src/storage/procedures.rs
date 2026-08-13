//! Narrow storage for procedure candidate records (gated).
//!
//! Candidates derive only from accepted lesson evidence linked to trusted
//! outcomes. This store wraps `Arc<dyn DbClient>` without modifying the trait.
//! Promotion is disabled until the procedure gate passes.

use std::sync::Arc;

use crate::models::ProcedureCandidateRecord;
use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// Narrow store for procedure candidates.
pub struct ProcedureStore {
    client: BoundDbClient,
}

impl ProcedureStore {
    /// Create a new store bound to the process Active Namespace.
    #[must_use]
    pub fn new(client: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            client: BoundDbClient::new(client, namespace),
        }
    }

    /// Load a candidate by its deterministic ID.
    pub async fn load_candidate(
        &self,
        candidate_id: &str,
    ) -> Result<Option<ProcedureCandidateRecord>, MemoryError> {
        let record_id = format!("procedure_candidate:{candidate_id}");
        let existing = self.client.select_one(&record_id).await?;
        match existing {
            Some(value) => {
                let record: ProcedureCandidateRecord =
                    serde_json::from_value(value).map_err(|e| {
                        MemoryError::Storage(format!("failed to parse procedure_candidate: {e}"))
                    })?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Create a new candidate. Returns `Conflict` if the ID already exists.
    pub async fn create_candidate(
        &self,
        record: &ProcedureCandidateRecord,
    ) -> Result<(), MemoryError> {
        let record_id = format!("procedure_candidate:{}", record.candidate_id);
        let value = serde_json::to_value(record).map_err(|e| {
            MemoryError::Storage(format!("failed to serialize procedure_candidate: {e}"))
        })?;
        self.client.create(&record_id, value).await.map(|_| ())
    }

    /// Update an existing candidate (e.g., append evidence, change status).
    pub async fn update_candidate(
        &self,
        record: &ProcedureCandidateRecord,
    ) -> Result<(), MemoryError> {
        let record_id = format!("procedure_candidate:{}", record.candidate_id);
        let value = serde_json::to_value(record).map_err(|e| {
            MemoryError::Storage(format!("failed to serialize procedure_candidate: {e}"))
        })?;
        self.client.update(&record_id, value).await.map(|_| ())
    }

    /// List candidates in the active namespace, optionally filtered by status.
    ///
    /// The database namespace is the only partition boundary. Legacy `scope`
    /// and `project` fields are returned as metadata but never used for filtering.
    pub async fn list_candidates(
        &self,
        status: Option<&str>,
    ) -> Result<Vec<ProcedureCandidateRecord>, MemoryError> {
        let (sql, vars) = match status {
            Some(status) => (
                "SELECT * FROM procedure_candidate WHERE status = $status ORDER BY updated_at DESC",
                serde_json::json!({"status": status}),
            ),
            None => (
                "SELECT * FROM procedure_candidate ORDER BY updated_at DESC",
                serde_json::json!({}),
            ),
        };
        let rows = self.client.query(sql, Some(vars)).await?;

        let Some(arr) = rows.as_array() else {
            return Ok(Vec::new());
        };

        arr.iter()
            .cloned()
            .map(serde_json::from_value::<ProcedureCandidateRecord>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| MemoryError::Storage(format!("failed to parse procedure_candidate: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::ProcedureCandidateRecord;

    fn make_candidate(id: &str, scope: &str, project: Option<&str>) -> ProcedureCandidateRecord {
        ProcedureCandidateRecord {
            candidate_id: id.to_string(),
            namespace: "test".to_string(),
            identity_version: 2,
            scope: Some(scope.to_string()),
            project: project.map(str::to_string),
            task_fingerprint: "task:1".to_string(),
            normalized_task: "do work".to_string(),
            status: "shadow".to_string(),
            trust_floor: "lifecycle_evidence".to_string(),
            success_count: 0,
            failure_count: 0,
            evidence_count: 0,
            origin_kind: "lifecycle_adapter".to_string(),
            created_at: "2026-07-23T00:00:00Z".to_string(),
            updated_at: "2026-07-23T00:00:00Z".to_string(),
            promoted_at: None,
            deprecated_at: None,
            expires_at: None,
        }
    }

    #[test]
    fn procedure_store_is_send_sync() {
        // Compile-time assertion: ProcedureStore must be Send + Sync for use
        // in async contexts.
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ProcedureStore>();
    }

    #[test]
    fn make_candidate_helper_produces_shadow_status() {
        let candidate = make_candidate("c1", "org", Some("p"));
        assert_eq!(candidate.status, "shadow");
        assert_eq!(candidate.trust_floor, "lifecycle_evidence");
    }
}
