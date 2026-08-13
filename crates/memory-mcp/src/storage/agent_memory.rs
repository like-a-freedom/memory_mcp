//! Narrow storage for agent-memory lifecycle events and projection jobs.
//!
//! This wraps `Arc<dyn DbClient>` without modifying the trait. Accepted content
//! is stored once in the episode table; the event references the episode
//! without copying content. Ignored and duplicate events create zero durable
//! rows.

use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::{
    CaptureDisposition, CaptureReasonCode, InvocationOrigin, SourceKind, TrustClass,
};
use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// A persisted lifecycle event record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryEventRecord {
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_event: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_event_id: Option<String>,
    pub event_kind: String,
    pub task_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub normalized_task: Option<String>,
    #[serde(default)]
    pub policy_tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_signal: Option<String>,
    pub disposition: String,
    pub trust_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_byte_len: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_uri_count: Option<i64>,
    #[serde(default)]
    pub reason_codes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_retrieval_fingerprint: Option<String>,
    #[serde(default)]
    pub trace_selected_fact_ids: Vec<String>,
    #[serde(default)]
    pub trace_selected_experience_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_policy_fingerprint: Option<String>,
    pub origin_kind: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// A persisted projection job record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventProjectionJobRecord {
    pub job_id: String,
    pub event_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub episode_id: Option<String>,
    pub status: String,
    #[serde(default)]
    pub attempts: i64,
    #[serde(default)]
    pub max_attempts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leased_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_expires_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dead_lettered_at: Option<String>,
    pub origin_kind: String,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// A rejection audit record (hashes and reason codes only).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryCaptureAuditRecord {
    pub audit_id: String,
    pub event_id: String,
    pub content_hash: String,
    pub content_byte_len: i64,
    pub disposition: String,
    #[serde(default)]
    pub reason_codes: Vec<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// Parse a scope-free lifecycle record while accepting only the two known
/// legacy partition fields for read-old compatibility.
fn parse_scope_free_record<T: DeserializeOwned>(
    mut value: Value,
    record_type: &str,
) -> Result<T, MemoryError> {
    let object = value.as_object_mut().ok_or_else(|| {
        MemoryError::Storage(format!(
            "failed to parse {record_type}: durable row is not an object"
        ))
    })?;

    for field in ["scope", "project"] {
        if let Some(legacy_value) = object.remove(field)
            && !legacy_value.is_string()
            && !legacy_value.is_null()
        {
            return Err(MemoryError::Storage(format!(
                "failed to parse {record_type}: legacy {field} must be a string"
            )));
        }
    }

    serde_json::from_value(value)
        .map_err(|error| MemoryError::Storage(format!("failed to parse {record_type}: {error}")))
}

pub(crate) fn parse_memory_event(value: Value) -> Result<MemoryEventRecord, MemoryError> {
    parse_scope_free_record(value, "memory_event")
}

pub(crate) fn parse_event_projection_job(
    value: Value,
) -> Result<EventProjectionJobRecord, MemoryError> {
    parse_scope_free_record(value, "event_projection_job")
}

pub(crate) fn parse_memory_capture_audit(
    value: Value,
) -> Result<MemoryCaptureAuditRecord, MemoryError> {
    parse_scope_free_record(value, "memory_capture_audit")
}

/// Narrow store for agent-memory lifecycle records.
///
/// Wraps `Arc<dyn DbClient>` without modifying the trait.
pub struct AgentMemoryStore {
    client: BoundDbClient,
}

impl AgentMemoryStore {
    /// Create a new narrow store bound to the process Active Namespace.
    #[must_use]
    pub fn new(client: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            client: BoundDbClient::new(client, namespace),
        }
    }

    /// Load a memory event by its stable event ID.
    pub async fn load_event(
        &self,
        event_id: &str,
    ) -> Result<Option<MemoryEventRecord>, MemoryError> {
        let record_id = format!("memory_event:{event_id}");
        let existing = self.client.select_one(&record_id).await?;
        match existing {
            Some(value) => {
                let record = parse_memory_event(value)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Persist a memory event.
    pub async fn create_event(&self, record: &MemoryEventRecord) -> Result<(), MemoryError> {
        let record_id = format!("memory_event:{}", record.event_id);
        let value = serde_json::to_value(record)
            .map_err(|e| MemoryError::Storage(format!("failed to serialize memory_event: {e}")))?;
        self.client.create(&record_id, value).await.map(|_| ())
    }

    /// Load pending jobs or jobs whose lease has expired.
    pub async fn load_pending_jobs(
        &self,
        now: &str,
        limit: i32,
    ) -> Result<Vec<EventProjectionJobRecord>, MemoryError> {
        let sql = "SELECT * FROM event_projection_job WHERE status = 'pending' OR (status = 'leased' AND lease_expires_at <= type::datetime($now)) LIMIT $limit";
        let rows = self
            .client
            .query(sql, Some(serde_json::json!({"now": now, "limit": limit})))
            .await?;
        rows.as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .cloned()
            .map(parse_event_projection_job)
            .collect()
    }

    /// Update a projection job through the process-bound namespace.
    pub async fn update_job(&self, job_id: &str, payload: Value) -> Result<(), MemoryError> {
        let record_id = format!("event_projection_job:{job_id}");
        self.client.update(&record_id, payload).await.map(|_| ())
    }

    /// Load a projection job by its job ID.
    pub async fn load_job(
        &self,
        job_id: &str,
    ) -> Result<Option<EventProjectionJobRecord>, MemoryError> {
        let record_id = format!("event_projection_job:{job_id}");
        let existing = self.client.select_one(&record_id).await?;
        match existing {
            Some(value) => {
                let record = parse_event_projection_job(value)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// Persist a projection job.
    pub async fn create_job(&self, record: &EventProjectionJobRecord) -> Result<(), MemoryError> {
        let record_id = format!("event_projection_job:{}", record.job_id);
        let value = serde_json::to_value(record).map_err(|e| {
            MemoryError::Storage(format!("failed to serialize event_projection_job: {e}"))
        })?;
        self.client.create(&record_id, value).await.map(|_| ())
    }

    /// Persist a rejection audit.
    pub async fn create_audit(&self, record: &MemoryCaptureAuditRecord) -> Result<(), MemoryError> {
        let record_id = format!("memory_capture_audit:{}", record.audit_id);
        let value = serde_json::to_value(record).map_err(|e| {
            MemoryError::Storage(format!("failed to serialize memory_capture_audit: {e}"))
        })?;
        self.client.create(&record_id, value).await.map(|_| ())
    }

    /// Load a rejection audit by event ID.
    pub async fn load_audit_by_event(
        &self,
        event_id: &str,
    ) -> Result<Option<MemoryCaptureAuditRecord>, MemoryError> {
        let sql = "SELECT * FROM memory_capture_audit WHERE event_id = $event_id LIMIT 1";
        let vars = serde_json::json!({"event_id": event_id});
        let rows = self.client.query(sql, Some(vars)).await?;
        // query returns a Value; for a SELECT it is typically an array.
        let row = rows.as_array().and_then(|arr| arr.first()).cloned();
        match row {
            Some(value) => {
                let record = parse_memory_capture_audit(value)?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }
}

/// Serialize a disposition for storage.
#[must_use]
pub fn disposition_str(d: &CaptureDisposition) -> &'static str {
    match d {
        CaptureDisposition::Accepted => "accepted",
        CaptureDisposition::Duplicate => "duplicate",
        CaptureDisposition::Ignored => "ignored",
        CaptureDisposition::Quarantined => "quarantined",
        CaptureDisposition::Rejected => "rejected",
        CaptureDisposition::Degraded => "degraded",
    }
}

/// Serialize a trust class for storage.
#[must_use]
pub fn trust_class_str(t: &TrustClass) -> &'static str {
    match t {
        TrustClass::AgentInference => "agent_inference",
        TrustClass::LifecycleEvidence => "lifecycle_evidence",
        TrustClass::OperatorApproved => "operator_approved",
        TrustClass::LegacyUnknown => "legacy_unknown",
        TrustClass::UntrustedExternal => "untrusted_external",
    }
}

/// Serialize a source kind for storage.
#[must_use]
pub fn source_kind_str(s: &SourceKind) -> &'static str {
    match s {
        SourceKind::AgentOutput => "agent_output",
        SourceKind::ToolResult => "tool_result",
        SourceKind::UserMessage => "user_message",
        SourceKind::Operator => "operator",
        SourceKind::External => "external",
        SourceKind::LegacyUnknown => "legacy_unknown",
    }
}

/// Serialize reason codes for storage.
#[must_use]
pub fn reason_codes_str(codes: &[CaptureReasonCode]) -> Vec<&'static str> {
    codes
        .iter()
        .map(|code| match code {
            CaptureReasonCode::EmptyTask => "empty_task",
            CaptureReasonCode::UnchangedTask => "unchanged_task",
            CaptureReasonCode::StatusPolling => "status_polling",
            CaptureReasonCode::ReadOnlyNoise => "read_only_noise",
            CaptureReasonCode::DuplicateIdentity => "duplicate_identity",
            CaptureReasonCode::SecretLikeContent => "secret_like_content",
            CaptureReasonCode::ExternalSelfPromotion => "external_self_promotion",
            CaptureReasonCode::BudgetExhausted => "budget_exhausted",
            CaptureReasonCode::QuarantineTtl => "quarantine_ttl",
            CaptureReasonCode::AcceptedPreference => "accepted_preference",
            CaptureReasonCode::AcceptedConstraint => "accepted_constraint",
            CaptureReasonCode::AcceptedDecision => "accepted_decision",
            CaptureReasonCode::AcceptedCommitment => "accepted_commitment",
            CaptureReasonCode::AcceptedCorrection => "accepted_correction",
            CaptureReasonCode::AcceptedOutcome => "accepted_outcome",
            CaptureReasonCode::AcceptedCheckpoint => "accepted_checkpoint",
            CaptureReasonCode::DegradedListenerUnavailable => "degraded_listener_unavailable",
        })
        .collect()
}

/// Serialize the invocation origin kind for storage.
#[must_use]
pub fn origin_kind_str(origin: &InvocationOrigin) -> &'static str {
    match origin {
        InvocationOrigin::AgentSelected => "agent_selected",
        InvocationOrigin::LifecycleAdapter { .. } => "lifecycle_adapter",
        InvocationOrigin::VerifiedConnector { .. } => "verified_connector",
        InvocationOrigin::Operator { .. } => "operator",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_partition_fields_are_read_compatibility_only() {
        let event = parse_memory_event(serde_json::json!({
            "event_id": "evt:legacy",
            "event_kind": "post_tool_result",
            "task_fingerprint": "task:legacy",
            "scope": "org",
            "project": "legacy-project",
            "disposition": "accepted",
            "trust_class": "lifecycle_evidence",
            "origin_kind": "lifecycle_adapter",
            "created_at": "2026-08-13T00:00:00Z"
        }))
        .expect("legacy event row remains readable");
        let encoded = serde_json::to_value(event).expect("encode scope-free event");
        assert!(
            !encoded
                .as_object()
                .expect("event object")
                .contains_key("scope")
        );
        assert!(
            !encoded
                .as_object()
                .expect("event object")
                .contains_key("project")
        );

        let job = parse_event_projection_job(serde_json::json!({
            "job_id": "job:legacy",
            "event_id": "evt:legacy",
            "scope": "org",
            "project": "legacy-project",
            "status": "pending",
            "origin_kind": "lifecycle_adapter",
            "created_at": "2026-08-13T00:00:00Z"
        }))
        .expect("legacy job row remains readable");
        let encoded = serde_json::to_value(job).expect("encode scope-free job");
        assert!(
            !encoded
                .as_object()
                .expect("job object")
                .contains_key("scope")
        );
        assert!(
            !encoded
                .as_object()
                .expect("job object")
                .contains_key("project")
        );

        let audit = parse_memory_capture_audit(serde_json::json!({
            "audit_id": "audit:legacy",
            "event_id": "evt:legacy",
            "content_hash": "sha256:legacy",
            "content_byte_len": 0,
            "disposition": "rejected",
            "scope": "org",
            "project": "legacy-project",
            "created_at": "2026-08-13T00:00:00Z"
        }))
        .expect("legacy audit row remains readable");
        let encoded = serde_json::to_value(audit).expect("encode scope-free audit");
        assert!(
            !encoded
                .as_object()
                .expect("audit object")
                .contains_key("scope")
        );
        assert!(
            !encoded
                .as_object()
                .expect("audit object")
                .contains_key("project")
        );
    }

    #[test]
    fn malformed_legacy_partition_field_is_observable() {
        let error = parse_event_projection_job(serde_json::json!({
            "job_id": "job:malformed",
            "event_id": "evt:malformed",
            "scope": {"unexpected": true},
            "status": "pending",
            "origin_kind": "lifecycle_adapter",
            "created_at": "2026-08-13T00:00:00Z"
        }))
        .expect_err("malformed legacy field must not be silently accepted");
        assert!(error.to_string().contains("legacy scope"));
    }

    #[test]
    fn disposition_str_is_stable() {
        assert_eq!(disposition_str(&CaptureDisposition::Accepted), "accepted");
        assert_eq!(disposition_str(&CaptureDisposition::Ignored), "ignored");
        assert_eq!(disposition_str(&CaptureDisposition::Rejected), "rejected");
    }

    #[test]
    fn trust_class_str_is_stable() {
        assert_eq!(
            trust_class_str(&TrustClass::AgentInference),
            "agent_inference"
        );
        assert_eq!(
            trust_class_str(&TrustClass::UntrustedExternal),
            "untrusted_external"
        );
    }

    #[test]
    fn origin_kind_str_is_stable() {
        assert_eq!(
            origin_kind_str(&InvocationOrigin::AgentSelected),
            "agent_selected"
        );
        assert_eq!(
            origin_kind_str(&InvocationOrigin::LifecycleAdapter {
                adapter_id: "x".into(),
                adapter_version: "1".into(),
                host_event: "y".into()
            }),
            "lifecycle_adapter"
        );
    }

    #[test]
    fn reason_codes_str_maps_all_variants() {
        let codes = vec![
            CaptureReasonCode::BudgetExhausted,
            CaptureReasonCode::SecretLikeContent,
            CaptureReasonCode::AcceptedPreference,
        ];
        let strs = reason_codes_str(&codes);
        assert_eq!(
            strs,
            vec![
                "budget_exhausted",
                "secret_like_content",
                "accepted_preference"
            ]
        );
    }
}
