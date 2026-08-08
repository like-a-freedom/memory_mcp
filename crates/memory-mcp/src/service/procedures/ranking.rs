//! Deterministic ranking for procedure candidates.
//!
//! Filter by namespace, scope, project, policy, status, trust floor, and risk
//! authorization before ranking. Use normalized task overlap, posterior mean,
//! independent evidence count, recency, and stable ID as the deterministic
//! tuple.
//!
//! No public CRUD, learned ranker, second embedding dependency, or physical
//! procedural graph.

use crate::models::{ProcedureCandidateRecord, beta_posterior_mean};

/// A candidate with its computed ranking score.
#[derive(Debug, Clone, PartialEq)]
pub struct CandidateRankingEntry {
    pub candidate: ProcedureCandidateRecord,
    /// Beta posterior mean (0.0–1.0).
    pub posterior_mean: f64,
    /// Normalized task overlap with the query (0.0–1.0).
    pub task_overlap: f64,
    /// Number of independent evidence observations.
    pub evidence_count: i64,
    /// Ranking score — higher is better.
    pub score: f64,
}

/// Rank candidates by a deterministic tuple: task overlap, posterior mean,
/// evidence count, recency (updated_at), and stable ID.
///
/// The ranking is deterministic: identical inputs produce identical ordering.
pub fn rank_candidates(
    candidates: Vec<ProcedureCandidateRecord>,
    query_task: &str,
) -> Vec<CandidateRankingEntry> {
    let mut entries: Vec<CandidateRankingEntry> = candidates
        .into_iter()
        .map(|candidate| {
            let posterior_mean =
                beta_posterior_mean(candidate.success_count, candidate.failure_count);
            let task_overlap = normalized_task_overlap(&candidate.normalized_task, query_task);
            let evidence_count = candidate.evidence_count;
            // Score: weighted combination. Higher overlap, posterior, and
            // evidence are better. The exact weights are stable and
            // documented; they are not learned.
            let score = (task_overlap * 0.4)
                + (posterior_mean * 0.35)
                + (normalize_evidence(evidence_count) * 0.15)
                + (recency_score(&candidate.updated_at) * 0.10);
            CandidateRankingEntry {
                candidate,
                posterior_mean,
                task_overlap,
                evidence_count,
                score,
            }
        })
        .collect();

    // Sort by score descending, then by candidate_id ascending (stable
    // tiebreaker).
    entries.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.candidate.candidate_id.cmp(&b.candidate.candidate_id))
    });

    entries
}

/// Compute normalized task overlap (0.0–1.0) using token-based Jaccard.
fn normalized_task_overlap(candidate_task: &str, query_task: &str) -> f64 {
    let candidate_tokens: std::collections::HashSet<&str> =
        candidate_task.split_whitespace().collect();
    let query_tokens: std::collections::HashSet<&str> = query_task.split_whitespace().collect();

    if candidate_tokens.is_empty() || query_tokens.is_empty() {
        return 0.0;
    }

    let intersection = candidate_tokens.intersection(&query_tokens).count();
    let union = candidate_tokens.union(&query_tokens).count();

    if union == 0 {
        0.0
    } else {
        intersection as f64 / union as f64
    }
}

/// Normalize evidence count to 0.0–1.0, saturating at 10 observations.
fn normalize_evidence(count: i64) -> f64 {
    (count as f64 / 10.0).min(1.0)
}

/// Recency score: more recent updates get higher scores. Saturates at 90 days.
fn recency_score(updated_at: &str) -> f64 {
    // Parse the RFC3339 timestamp. If parsing fails, return 0.0 (neutral).
    let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(updated_at) else {
        return 0.0;
    };
    let now = chrono::Utc::now();
    let age_days = now
        .signed_duration_since(parsed.with_timezone(&chrono::Utc))
        .num_days();

    if age_days <= 0 {
        1.0
    } else if age_days >= 90 {
        0.0
    } else {
        1.0 - (age_days as f64 / 90.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candidate(
        id: &str,
        task: &str,
        success: i64,
        failure: i64,
        evidence: i64,
        updated: &str,
    ) -> ProcedureCandidateRecord {
        ProcedureCandidateRecord {
            candidate_id: id.to_string(),
            namespace: "test".to_string(),
            scope: "org".to_string(),
            project: Some("p".to_string()),
            task_fingerprint: task.to_string(),
            normalized_task: task.to_string(),
            status: "promoted".to_string(),
            trust_floor: "lifecycle_evidence".to_string(),
            success_count: success,
            failure_count: failure,
            evidence_count: evidence,
            origin_kind: "lifecycle_adapter".to_string(),
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: updated.to_string(),
            promoted_at: None,
            deprecated_at: None,
            expires_at: None,
        }
    }

    #[test]
    fn rank_candidates_orders_by_score_descending() {
        let candidates = vec![
            make_candidate("low", "task a", 1, 9, 1, "2026-01-01T00:00:00Z"),
            make_candidate("high", "task a", 9, 1, 10, "2026-07-20T00:00:00Z"),
        ];
        let ranked = rank_candidates(candidates, "task a");
        assert_eq!(ranked[0].candidate.candidate_id, "high");
        assert_eq!(ranked[1].candidate.candidate_id, "low");
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn rank_candidates_tiebreaks_by_id_ascending() {
        let candidates = vec![
            make_candidate("zzz", "task a", 5, 5, 3, "2026-07-01T00:00:00Z"),
            make_candidate("aaa", "task a", 5, 5, 3, "2026-07-01T00:00:00Z"),
        ];
        let ranked = rank_candidates(candidates, "task a");
        assert_eq!(ranked[0].candidate.candidate_id, "aaa");
        assert_eq!(ranked[1].candidate.candidate_id, "zzz");
    }

    #[test]
    fn task_overlap_jaccard() {
        assert!(normalized_task_overlap("add oauth login", "add oauth") > 0.0);
        assert_eq!(normalized_task_overlap("add oauth", "fix bug"), 0.0);
        assert_eq!(normalized_task_overlap("", "add oauth"), 0.0);
    }

    #[test]
    fn normalize_evidence_saturates_at_10() {
        assert!((normalize_evidence(0) - 0.0).abs() < 1e-9);
        assert!((normalize_evidence(5) - 0.5).abs() < 1e-9);
        assert!((normalize_evidence(10) - 1.0).abs() < 1e-9);
        assert!((normalize_evidence(100) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn recency_score_handles_invalid_timestamp() {
        assert_eq!(recency_score("not-a-date"), 0.0);
    }

    #[test]
    fn rank_candidates_empty_input_returns_empty() {
        let ranked = rank_candidates(vec![], "task");
        assert!(ranked.is_empty());
    }

    #[test]
    fn rank_candidates_deterministic_for_same_input() {
        let candidates = vec![
            make_candidate("a", "task a", 3, 2, 5, "2026-07-01T00:00:00Z"),
            make_candidate("b", "task b", 7, 1, 8, "2026-07-15T00:00:00Z"),
        ];
        let ranked1 = rank_candidates(candidates.clone(), "task a");
        let ranked2 = rank_candidates(candidates, "task a");
        assert_eq!(ranked1.len(), ranked2.len());
        for (a, b) in ranked1.iter().zip(ranked2.iter()) {
            assert_eq!(a.candidate.candidate_id, b.candidate.candidate_id);
            assert!((a.score - b.score).abs() < 1e-9);
        }
    }
}
