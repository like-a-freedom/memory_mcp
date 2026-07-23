//! Procedural memory candidate model (gated).
//!
//! Candidates derive only from accepted lesson evidence linked to trusted
//! outcomes. They group deterministically, append evidence, derive a Beta
//! posterior from counts, and never auto-promote. The procedure gate must
//! pass before promotion is enabled.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Status of a procedure candidate.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProcedureStatus {
    /// Shadow-only: candidate exists but is not promoted.
    Shadow,
    /// Promoted to experience retrieval.
    Promoted,
    /// Deprecated by an operator.
    Deprecated,
}

impl ProcedureStatus {
    /// Serialize to a string for storage.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Shadow => "shadow",
            Self::Promoted => "promoted",
            Self::Deprecated => "deprecated",
        }
    }
}

/// A procedure candidate record.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProcedureCandidateRecord {
    pub candidate_id: String,
    pub namespace: String,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub task_fingerprint: String,
    pub normalized_task: String,
    pub status: String,
    pub trust_floor: String,
    #[serde(default)]
    pub success_count: i64,
    #[serde(default)]
    pub failure_count: i64,
    #[serde(default)]
    pub evidence_count: i64,
    pub origin_kind: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
}

/// Derive a Beta posterior mean from success and failure counts.
///
/// Uses a Beta(1, 1) prior. Do not persist redundant alpha/beta fields.
#[must_use]
pub fn beta_posterior_mean(success_count: i64, failure_count: i64) -> f64 {
    let alpha = 1.0_f64 + success_count as f64;
    let beta = 1.0_f64 + failure_count as f64;
    alpha / (alpha + beta)
}

/// Compute a deterministic candidate ID from namespace, scope, project, and
/// task fingerprint.
#[must_use]
pub fn deterministic_candidate_id(
    namespace: &str,
    scope: &str,
    project: Option<&str>,
    task_fingerprint: &str,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    hasher.update(b":");
    hasher.update(scope.as_bytes());
    hasher.update(b":");
    hasher.update(project.unwrap_or("").as_bytes());
    hasher.update(b":");
    hasher.update(task_fingerprint.as_bytes());
    let hash = hex::encode(&hasher.finalize()[..]);
    format!("procedure_candidate:{hash}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_posterior_mean_with_no_evidence_is_prior() {
        // Beta(1,1) prior → mean 0.5
        let mean = beta_posterior_mean(0, 0);
        assert!((mean - 0.5).abs() < 1e-9);
    }

    #[test]
    fn beta_posterior_mean_favors_success() {
        let mean = beta_posterior_mean(9, 1);
        assert!(
            mean > 0.8,
            "9 successes and 1 failure should give mean > 0.8, got {mean}"
        );
    }

    #[test]
    fn beta_posterior_mean_favors_failure() {
        let mean = beta_posterior_mean(1, 9);
        assert!(
            mean < 0.2,
            "1 success and 9 failures should give mean < 0.2, got {mean}"
        );
    }

    #[test]
    fn deterministic_candidate_id_is_stable() {
        let id1 = deterministic_candidate_id("test", "org", Some("p"), "task:1");
        let id2 = deterministic_candidate_id("test", "org", Some("p"), "task:1");
        assert_eq!(id1, id2);
        assert!(id1.starts_with("procedure_candidate:"));
    }

    #[test]
    fn deterministic_candidate_id_differs_for_different_tasks() {
        let id1 = deterministic_candidate_id("test", "org", Some("p"), "task:1");
        let id2 = deterministic_candidate_id("test", "org", Some("p"), "task:2");
        assert_ne!(id1, id2);
    }

    #[test]
    fn procedure_status_as_str_is_stable() {
        assert_eq!(ProcedureStatus::Shadow.as_str(), "shadow");
        assert_eq!(ProcedureStatus::Promoted.as_str(), "promoted");
        assert_eq!(ProcedureStatus::Deprecated.as_str(), "deprecated");
    }
}
