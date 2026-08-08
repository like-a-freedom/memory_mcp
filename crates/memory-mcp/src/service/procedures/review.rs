//! Operator review workflow for procedure candidates.
//!
//! `open_app` and `app_command` expose candidate evidence and operator actions.
//! Every mutation returns a change ID and is verified by persisted readback.
//!
//! Only current, promoted, scope-authorized versions become `FactType::Experience`
//! records. Existing `assemble_context` retrieves them under the existing shared
//! budget.

use crate::models::{ProcedureCandidateRecord, ProcedureStatus};
use crate::service::MemoryError;

/// The action an operator wishes to take on a candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewAction {
    /// Promote the candidate from Shadow to Promoted.
    Promote,
    /// Deprecate the candidate (retire it).
    Deprecate,
    /// Keep as shadow (no status change, just record the review).
    KeepShadow,
    /// Reject and close (mark deprecated with a rejection reason).
    Reject { reason: String },
}

/// The decision returned after applying a review action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewDecision {
    /// The change ID for audit traceability.
    pub change_id: String,
    /// The new status after the review.
    pub new_status: ProcedureStatus,
    /// Whether the candidate is now retrievable as Experience.
    pub promoted_to_experience: bool,
}

/// Review a candidate and apply the operator action.
///
/// Returns a `ReviewDecision` with a change ID. The caller must persist the
/// updated record and verify by readback.
///
/// # Errors
///
/// Returns `MemoryError::Validation` if:
/// - attempting to promote a deprecated candidate;
/// - attempting to promote a candidate with zero evidence;
/// - attempting to deprecate an already-deprecated candidate.
pub fn review_candidate(
    candidate: &ProcedureCandidateRecord,
    action: &ReviewAction,
) -> Result<ReviewDecision, MemoryError> {
    let current_status = parse_status(&candidate.status);

    match action {
        ReviewAction::Promote => {
            if current_status == ProcedureStatus::Deprecated {
                return Err(MemoryError::Validation(
                    "cannot promote a deprecated candidate".to_string(),
                ));
            }
            if current_status == ProcedureStatus::Promoted {
                return Err(MemoryError::Validation(
                    "candidate is already promoted".to_string(),
                ));
            }
            if candidate.evidence_count == 0 {
                return Err(MemoryError::Validation(
                    "cannot promote a candidate with zero evidence".to_string(),
                ));
            }
            let change_id = format!(
                "change:{}:promote:{}",
                candidate.candidate_id,
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            );
            Ok(ReviewDecision {
                change_id,
                new_status: ProcedureStatus::Promoted,
                promoted_to_experience: true,
            })
        }
        ReviewAction::Deprecate => {
            if current_status == ProcedureStatus::Deprecated {
                return Err(MemoryError::Validation(
                    "candidate is already deprecated".to_string(),
                ));
            }
            let change_id = format!(
                "change:{}:deprecate:{}",
                candidate.candidate_id,
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            );
            Ok(ReviewDecision {
                change_id,
                new_status: ProcedureStatus::Deprecated,
                promoted_to_experience: false,
            })
        }
        ReviewAction::KeepShadow => {
            let change_id = format!(
                "change:{}:keep:{}",
                candidate.candidate_id,
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            );
            Ok(ReviewDecision {
                change_id,
                new_status: ProcedureStatus::Shadow,
                promoted_to_experience: false,
            })
        }
        ReviewAction::Reject { reason } => {
            if reason.trim().is_empty() {
                return Err(MemoryError::Validation(
                    "rejection reason must not be empty".to_string(),
                ));
            }
            let change_id = format!(
                "change:{}:reject:{}",
                candidate.candidate_id,
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            );
            Ok(ReviewDecision {
                change_id,
                new_status: ProcedureStatus::Deprecated,
                promoted_to_experience: false,
            })
        }
    }
}

/// Parse a status string back into the enum.
fn parse_status(s: &str) -> ProcedureStatus {
    match s {
        "shadow" => ProcedureStatus::Shadow,
        "promoted" => ProcedureStatus::Promoted,
        "deprecated" => ProcedureStatus::Deprecated,
        _ => ProcedureStatus::Shadow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candidate(status: &str, evidence: i64) -> ProcedureCandidateRecord {
        ProcedureCandidateRecord {
            candidate_id: "c1".to_string(),
            namespace: "test".to_string(),
            scope: "org".to_string(),
            project: Some("p".to_string()),
            task_fingerprint: "task:1".to_string(),
            normalized_task: "do work".to_string(),
            status: status.to_string(),
            trust_floor: "lifecycle_evidence".to_string(),
            success_count: 3,
            failure_count: 1,
            evidence_count: evidence,
            origin_kind: "lifecycle_adapter".to_string(),
            created_at: "2026-07-01T00:00:00Z".to_string(),
            updated_at: "2026-07-01T00:00:00Z".to_string(),
            promoted_at: None,
            deprecated_at: None,
            expires_at: None,
        }
    }

    #[test]
    fn promote_shadow_with_evidence_succeeds() {
        let candidate = make_candidate("shadow", 5);
        let decision =
            review_candidate(&candidate, &ReviewAction::Promote).expect("promotion should succeed");
        assert_eq!(decision.new_status, ProcedureStatus::Promoted);
        assert!(decision.promoted_to_experience);
        assert!(decision.change_id.starts_with("change:c1:promote:"));
    }

    #[test]
    fn promote_with_zero_evidence_fails() {
        let candidate = make_candidate("shadow", 0);
        let result = review_candidate(&candidate, &ReviewAction::Promote);
        assert!(result.is_err());
    }

    #[test]
    fn promote_deprecated_fails() {
        let candidate = make_candidate("deprecated", 5);
        let result = review_candidate(&candidate, &ReviewAction::Promote);
        assert!(result.is_err());
    }

    #[test]
    fn promote_already_promoted_fails() {
        let candidate = make_candidate("promoted", 5);
        let result = review_candidate(&candidate, &ReviewAction::Promote);
        assert!(result.is_err());
    }

    #[test]
    fn deprecate_shadow_succeeds() {
        let candidate = make_candidate("shadow", 2);
        let decision = review_candidate(&candidate, &ReviewAction::Deprecate)
            .expect("deprecation should succeed");
        assert_eq!(decision.new_status, ProcedureStatus::Deprecated);
        assert!(!decision.promoted_to_experience);
    }

    #[test]
    fn deprecate_already_deprecated_fails() {
        let candidate = make_candidate("deprecated", 2);
        let result = review_candidate(&candidate, &ReviewAction::Deprecate);
        assert!(result.is_err());
    }

    #[test]
    fn keep_shadow_returns_shadow() {
        let candidate = make_candidate("shadow", 2);
        let decision = review_candidate(&candidate, &ReviewAction::KeepShadow)
            .expect("keep shadow should succeed");
        assert_eq!(decision.new_status, ProcedureStatus::Shadow);
        assert!(!decision.promoted_to_experience);
    }

    #[test]
    fn reject_with_reason_deprecates() {
        let candidate = make_candidate("shadow", 2);
        let decision = review_candidate(
            &candidate,
            &ReviewAction::Reject {
                reason: "poisoned".to_string(),
            },
        )
        .expect("reject should succeed");
        assert_eq!(decision.new_status, ProcedureStatus::Deprecated);
    }

    #[test]
    fn reject_with_empty_reason_fails() {
        let candidate = make_candidate("shadow", 2);
        let result = review_candidate(
            &candidate,
            &ReviewAction::Reject {
                reason: "".to_string(),
            },
        );
        assert!(result.is_err());
    }

    #[test]
    fn change_id_is_unique_per_action() {
        let candidate = make_candidate("shadow", 5);
        let d1 = review_candidate(&candidate, &ReviewAction::Promote).expect("first promotion");
        std::thread::sleep(std::time::Duration::from_millis(10));
        // Reset to shadow for the second review.
        let candidate2 = make_candidate("shadow", 5);
        let d2 = review_candidate(&candidate2, &ReviewAction::Promote).expect("second promotion");
        assert_ne!(d1.change_id, d2.change_id);
    }
}
