use std::collections::BTreeMap;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct RetrievalSuiteSummary {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub expected_hits: usize,
    pub matched_hits: usize,
    pub expected_tier_totals: BTreeMap<String, usize>,
    pub expected_tier_passed_cases: BTreeMap<String, usize>,
    pub actual_tier_totals: BTreeMap<String, usize>,
}

impl RetrievalSuiteSummary {
    pub fn recall_at_5(&self) -> f64 {
        if self.expected_hits == 0 {
            return 1.0;
        }

        self.matched_hits as f64 / self.expected_hits as f64
    }

    pub fn pass_rate(&self) -> f64 {
        if self.total_cases == 0 {
            return 1.0;
        }

        self.passed_cases as f64 / self.total_cases as f64
    }

    pub fn expected_tier_pass_rate(&self, tier: &str) -> Option<f64> {
        let total = self.expected_tier_totals.get(tier).copied()?;
        if total == 0 {
            return Some(1.0);
        }

        let passed = self
            .expected_tier_passed_cases
            .get(tier)
            .copied()
            .unwrap_or(0);
        Some(passed as f64 / total as f64)
    }
}

pub fn record_retrieval_case(
    summary: &mut RetrievalSuiteSummary,
    expected_tier: &str,
    matched_hits: usize,
    expected_hits: usize,
    actual_tiers: &[&str],
    min_recall_at_k: f64,
) -> bool {
    summary.total_cases += 1;
    summary.expected_hits += expected_hits;
    summary.matched_hits += matched_hits;
    *summary
        .expected_tier_totals
        .entry(expected_tier.to_string())
        .or_insert(0) += 1;

    for tier in actual_tiers {
        *summary
            .actual_tier_totals
            .entry((*tier).to_string())
            .or_insert(0) += 1;
    }

    let recall = if expected_hits == 0 {
        1.0
    } else {
        matched_hits as f64 / expected_hits as f64
    };
    let passed = recall >= min_recall_at_k;
    if passed {
        summary.passed_cases += 1;
        *summary
            .expected_tier_passed_cases
            .entry(expected_tier.to_string())
            .or_insert(0) += 1;
    }

    passed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_retrieval_case_tracks_expected_and_actual_tiers() {
        let mut summary = RetrievalSuiteSummary::default();

        let passed = record_retrieval_case(&mut summary, "direct", 1, 1, &["direct", "graph"], 1.0);

        assert!(passed);
        assert_eq!(summary.total_cases, 1);
        assert_eq!(summary.passed_cases, 1);
        assert_eq!(summary.expected_hits, 1);
        assert_eq!(summary.matched_hits, 1);
        assert_eq!(summary.expected_tier_totals.get("direct"), Some(&1));
        assert_eq!(summary.expected_tier_passed_cases.get("direct"), Some(&1));
        assert_eq!(summary.actual_tier_totals.get("direct"), Some(&1));
        assert_eq!(summary.actual_tier_totals.get("graph"), Some(&1));
    }

    #[test]
    fn expected_tier_pass_rate_uses_expected_tier_outcomes() {
        let mut summary = RetrievalSuiteSummary::default();

        let direct_passed = record_retrieval_case(&mut summary, "direct", 1, 1, &["direct"], 1.0);
        let graph_passed = record_retrieval_case(&mut summary, "graph", 0, 1, &["graph"], 1.0);

        assert!(direct_passed);
        assert!(!graph_passed);
        assert_eq!(summary.expected_tier_pass_rate("direct"), Some(1.0));
        assert_eq!(summary.expected_tier_pass_rate("graph"), Some(0.0));
        assert_eq!(summary.expected_tier_pass_rate("temporal"), None);
    }

    #[test]
    fn recall_and_pass_rate_default_to_one_for_empty_summary() {
        let summary = RetrievalSuiteSummary::default();

        assert_eq!(summary.recall_at_5(), 1.0);
        assert_eq!(summary.pass_rate(), 1.0);
    }
}
