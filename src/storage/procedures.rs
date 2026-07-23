//! Narrow storage for procedure candidate records (gated).
//!
//! Candidates derive only from accepted lesson evidence linked to trusted
//! outcomes. This store wraps `Arc<dyn DbClient>` without modifying the trait.
//! Promotion is disabled until the procedure gate passes.

use std::sync::Arc;

use crate::models::ProcedureCandidateRecord;
use crate::service::MemoryError;
use crate::storage::DbClient;

/// Narrow store for procedure candidates.
pub struct ProcedureStore {
    client: Arc<dyn DbClient>,
}

impl ProcedureStore {
    /// Create a new store over an existing client.
    #[must_use]
    pub fn new(client: Arc<dyn DbClient>) -> Self {
        Self { client }
    }

    /// Load a candidate by its deterministic ID.
    pub async fn load_candidate(
        &self,
        candidate_id: &str,
        namespace: &str,
    ) -> Result<Option<ProcedureCandidateRecord>, MemoryError> {
        let record_id = format!("procedure_candidate:{candidate_id}");
        let existing = self.client.select_one(&record_id, namespace).await?;
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
        namespace: &str,
    ) -> Result<(), MemoryError> {
        let record_id = format!("procedure_candidate:{}", record.candidate_id);
        let value = serde_json::to_value(record).map_err(|e| {
            MemoryError::Storage(format!("failed to serialize procedure_candidate: {e}"))
        })?;
        self.client
            .create(&record_id, value, namespace)
            .await
            .map(|_| ())
    }

    /// Update an existing candidate (e.g., append evidence, change status).
    pub async fn update_candidate(
        &self,
        record: &ProcedureCandidateRecord,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        let record_id = format!("procedure_candidate:{}", record.candidate_id);
        let value = serde_json::to_value(record).map_err(|e| {
            MemoryError::Storage(format!("failed to serialize procedure_candidate: {e}"))
        })?;
        self.client
            .update(&record_id, value, namespace)
            .await
            .map(|_| ())
    }

    /// List candidates filtered by namespace, scope, project, and status.
    pub async fn list_candidates(
        &self,
        namespace: &str,
        scope: &str,
        project: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<ProcedureCandidateRecord>, MemoryError> {
        let sql = if project.is_some() && status.is_some() {
            "SELECT * FROM procedure_candidate WHERE scope = $scope AND project = $project AND status = $status ORDER BY updated_at DESC"
        } else if project.is_some() {
            "SELECT * FROM procedure_candidate WHERE scope = $scope AND project = $project ORDER BY updated_at DESC"
        } else if status.is_some() {
            "SELECT * FROM procedure_candidate WHERE scope = $scope AND status = $status ORDER BY updated_at DESC"
        } else {
            "SELECT * FROM procedure_candidate WHERE scope = $scope ORDER BY updated_at DESC"
        };
        let mut vars = serde_json::Map::new();
        vars.insert(
            "scope".to_string(),
            serde_json::Value::String(scope.to_string()),
        );
        if let Some(project) = project {
            vars.insert(
                "project".to_string(),
                serde_json::Value::String(project.to_string()),
            );
        }
        if let Some(status) = status {
            vars.insert(
                "status".to_string(),
                serde_json::Value::String(status.to_string()),
            );
        }
        let rows = self
            .client
            .query(sql, Some(serde_json::Value::Object(vars)), namespace)
            .await?;

        let mut candidates = Vec::new();
        if let Some(arr) = rows.as_array() {
            for row in arr {
                if let Ok(record) = serde_json::from_value::<ProcedureCandidateRecord>(row.clone())
                {
                    candidates.push(record);
                }
            }
        }
        Ok(candidates)
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
            scope: scope.to_string(),
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
