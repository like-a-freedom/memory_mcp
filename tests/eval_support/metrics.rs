use std::collections::BTreeMap;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct RetrievalSuiteSummary {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub expected_hits: usize,
    pub matched_hits: usize,
    pub reciprocal_rank_sum: f64,
    pub top_1_hits: usize,
    pub diversity_expected_cases: usize,
    pub diversity_passed_cases: usize,
    pub unique_source_episode_ratio_sum: f64,
    pub max_source_episode_share_sum: f64,
    pub expected_tier_totals: BTreeMap<String, usize>,
    pub expected_tier_passed_cases: BTreeMap<String, usize>,
    pub actual_tier_totals: BTreeMap<String, usize>,
    pub expected_tag_totals: BTreeMap<String, usize>,
    pub expected_tag_passed_cases: BTreeMap<String, usize>,
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

    pub fn mrr(&self) -> f64 {
        if self.total_cases == 0 {
            return 1.0;
        }

        self.reciprocal_rank_sum / self.total_cases as f64
    }

    pub fn top_1_hit_rate(&self) -> f64 {
        if self.total_cases == 0 {
            return 1.0;
        }

        self.top_1_hits as f64 / self.total_cases as f64
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

    pub fn expected_tag_pass_rate(&self, tag: &str) -> Option<f64> {
        let total = self.expected_tag_totals.get(tag).copied()?;
        if total == 0 {
            return Some(1.0);
        }

        let passed = self
            .expected_tag_passed_cases
            .get(tag)
            .copied()
            .unwrap_or(0);
        Some(passed as f64 / total as f64)
    }

    pub fn diversity_pass_rate(&self) -> Option<f64> {
        if self.diversity_expected_cases == 0 {
            return None;
        }

        Some(self.diversity_passed_cases as f64 / self.diversity_expected_cases as f64)
    }

    pub fn average_unique_source_episode_ratio(&self) -> Option<f64> {
        if self.diversity_expected_cases == 0 {
            return None;
        }

        Some(self.unique_source_episode_ratio_sum / self.diversity_expected_cases as f64)
    }

    pub fn average_max_source_episode_share(&self) -> Option<f64> {
        if self.diversity_expected_cases == 0 {
            return None;
        }

        Some(self.max_source_episode_share_sum / self.diversity_expected_cases as f64)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RetrievalCaseDiagnostics<'a> {
    pub actual_tiers: &'a [&'a str],
    pub first_relevant_rank: Option<usize>,
    pub source_episodes: &'a [&'a str],
    pub min_unique_source_episodes: Option<usize>,
    pub max_source_episode_share: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceEpisodeDiversity {
    pub unique_source_episodes: usize,
    pub unique_source_episode_ratio: f64,
    pub max_source_episode_share: f64,
}

pub fn first_relevant_rank(
    retrieved_contents: &[&str],
    expected_needles: &[String],
) -> Option<usize> {
    if expected_needles.is_empty() {
        return None;
    }

    retrieved_contents
        .iter()
        .position(|content| {
            expected_needles
                .iter()
                .any(|needle| content.contains(needle.as_str()))
        })
        .map(|index| index + 1)
}

pub fn source_episode_diversity(source_episodes: &[&str]) -> Option<SourceEpisodeDiversity> {
    let mut counts = BTreeMap::<&str, usize>::new();

    for source_episode in source_episodes
        .iter()
        .copied()
        .map(str::trim)
        .filter(|source_episode| !source_episode.is_empty())
    {
        *counts.entry(source_episode).or_insert(0) += 1;
    }

    if counts.is_empty() {
        return None;
    }

    let total = counts.values().sum::<usize>();
    let max_bucket = counts.values().copied().max().unwrap_or(0);

    Some(SourceEpisodeDiversity {
        unique_source_episodes: counts.len(),
        unique_source_episode_ratio: counts.len() as f64 / total as f64,
        max_source_episode_share: max_bucket as f64 / total as f64,
    })
}

pub fn record_retrieval_case(
    summary: &mut RetrievalSuiteSummary,
    expected_tier: &str,
    expected_tags: &[String],
    matched_hits: usize,
    expected_hits: usize,
    min_recall_at_k: f64,
    diagnostics: RetrievalCaseDiagnostics<'_>,
) -> bool {
    summary.total_cases += 1;
    summary.expected_hits += expected_hits;
    summary.matched_hits += matched_hits;
    *summary
        .expected_tier_totals
        .entry(expected_tier.to_string())
        .or_insert(0) += 1;
    for tag in expected_tags {
        *summary.expected_tag_totals.entry(tag.clone()).or_insert(0) += 1;
    }

    for tier in diagnostics.actual_tiers {
        *summary
            .actual_tier_totals
            .entry((*tier).to_string())
            .or_insert(0) += 1;
    }

    if let Some(rank) = diagnostics.first_relevant_rank {
        summary.reciprocal_rank_sum += 1.0 / rank as f64;
        if rank == 1 {
            summary.top_1_hits += 1;
        }
    }

    let diversity_expected = diagnostics.min_unique_source_episodes.is_some()
        || diagnostics.max_source_episode_share.is_some();
    let diversity_passed = if diversity_expected {
        summary.diversity_expected_cases += 1;
        let diversity = source_episode_diversity(diagnostics.source_episodes).unwrap_or(
            SourceEpisodeDiversity {
                unique_source_episodes: 0,
                unique_source_episode_ratio: 0.0,
                max_source_episode_share: 1.0,
            },
        );
        summary.unique_source_episode_ratio_sum += diversity.unique_source_episode_ratio;
        summary.max_source_episode_share_sum += diversity.max_source_episode_share;

        let passed = diagnostics
            .min_unique_source_episodes
            .is_none_or(|minimum| diversity.unique_source_episodes >= minimum)
            && diagnostics
                .max_source_episode_share
                .is_none_or(|maximum| diversity.max_source_episode_share <= maximum);
        if passed {
            summary.diversity_passed_cases += 1;
        }
        passed
    } else {
        true
    };

    let recall = if expected_hits == 0 {
        1.0
    } else {
        matched_hits as f64 / expected_hits as f64
    };
    let passed = recall >= min_recall_at_k && diversity_passed;
    if passed {
        summary.passed_cases += 1;
        *summary
            .expected_tier_passed_cases
            .entry(expected_tier.to_string())
            .or_insert(0) += 1;
        for tag in expected_tags {
            *summary
                .expected_tag_passed_cases
                .entry(tag.clone())
                .or_insert(0) += 1;
        }
    }

    passed
}

pub fn revoke_retrieval_case_pass(
    summary: &mut RetrievalSuiteSummary,
    expected_tier: &str,
    expected_tags: &[String],
) {
    summary.passed_cases = summary.passed_cases.saturating_sub(1);
    if let Some(passed_cases) = summary.expected_tier_passed_cases.get_mut(expected_tier) {
        *passed_cases = passed_cases.saturating_sub(1);
    }
    for tag in expected_tags {
        if let Some(passed_cases) = summary.expected_tag_passed_cases.get_mut(tag) {
            *passed_cases = passed_cases.saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_retrieval_case_tracks_expected_and_actual_tiers() {
        let mut summary = RetrievalSuiteSummary::default();

        let passed = record_retrieval_case(
            &mut summary,
            "direct",
            &["timeline_auto".to_string(), "graph_anchor".to_string()],
            1,
            1,
            1.0,
            RetrievalCaseDiagnostics {
                actual_tiers: &["direct", "graph"],
                first_relevant_rank: Some(1),
                source_episodes: &[],
                min_unique_source_episodes: None,
                max_source_episode_share: None,
            },
        );

        assert!(passed);
        assert_eq!(summary.total_cases, 1);
        assert_eq!(summary.passed_cases, 1);
        assert_eq!(summary.expected_hits, 1);
        assert_eq!(summary.matched_hits, 1);
        assert_eq!(summary.mrr(), 1.0);
        assert_eq!(summary.top_1_hit_rate(), 1.0);
        assert_eq!(summary.expected_tier_totals.get("direct"), Some(&1));
        assert_eq!(summary.expected_tier_passed_cases.get("direct"), Some(&1));
        assert_eq!(summary.actual_tier_totals.get("direct"), Some(&1));
        assert_eq!(summary.actual_tier_totals.get("graph"), Some(&1));
        assert_eq!(summary.expected_tag_totals.get("timeline_auto"), Some(&1));
        assert_eq!(
            summary.expected_tag_passed_cases.get("graph_anchor"),
            Some(&1)
        );
    }

    #[test]
    fn expected_tier_pass_rate_uses_expected_tier_outcomes() {
        let mut summary = RetrievalSuiteSummary::default();

        let direct_passed = record_retrieval_case(
            &mut summary,
            "direct",
            &[],
            1,
            1,
            1.0,
            RetrievalCaseDiagnostics {
                actual_tiers: &["direct"],
                first_relevant_rank: Some(1),
                ..RetrievalCaseDiagnostics::default()
            },
        );
        let graph_passed = record_retrieval_case(
            &mut summary,
            "graph",
            &[],
            0,
            1,
            1.0,
            RetrievalCaseDiagnostics {
                actual_tiers: &["graph"],
                first_relevant_rank: None,
                ..RetrievalCaseDiagnostics::default()
            },
        );

        assert!(direct_passed);
        assert!(!graph_passed);
        assert_eq!(summary.expected_tier_pass_rate("direct"), Some(1.0));
        assert_eq!(summary.expected_tier_pass_rate("graph"), Some(0.0));
        assert_eq!(summary.expected_tier_pass_rate("temporal"), None);
    }

    #[test]
    fn record_retrieval_case_tracks_ranking_and_source_diversity() {
        let mut summary = RetrievalSuiteSummary::default();

        let passed = record_retrieval_case(
            &mut summary,
            "direct",
            &["timeline_auto".to_string()],
            2,
            2,
            1.0,
            RetrievalCaseDiagnostics {
                actual_tiers: &["direct"],
                first_relevant_rank: Some(2),
                source_episodes: &["episode:alpha", "episode:beta", "episode:beta"],
                min_unique_source_episodes: Some(2),
                max_source_episode_share: Some(0.80),
            },
        );

        assert!(passed);
        assert!((summary.mrr() - 0.5).abs() < f64::EPSILON);
        assert!((summary.top_1_hit_rate() - 0.0).abs() < f64::EPSILON);
        assert_eq!(summary.diversity_pass_rate(), Some(1.0));
        assert_eq!(
            summary.average_unique_source_episode_ratio(),
            Some(2.0 / 3.0)
        );
        assert_eq!(summary.average_max_source_episode_share(), Some(2.0 / 3.0));
    }

    #[test]
    fn revoke_retrieval_case_pass_keeps_tier_pass_counts_consistent() {
        let mut summary = RetrievalSuiteSummary::default();

        let passed = record_retrieval_case(
            &mut summary,
            "direct",
            &["graph_anchor".to_string()],
            1,
            1,
            1.0,
            RetrievalCaseDiagnostics {
                actual_tiers: &["direct"],
                first_relevant_rank: Some(1),
                ..RetrievalCaseDiagnostics::default()
            },
        );

        assert!(passed);
        revoke_retrieval_case_pass(&mut summary, "direct", &["graph_anchor".to_string()]);

        assert_eq!(summary.passed_cases, 0);
        assert_eq!(summary.expected_tier_passed_cases.get("direct"), Some(&0));
        assert_eq!(
            summary.expected_tag_passed_cases.get("graph_anchor"),
            Some(&0)
        );
    }

    #[test]
    fn first_relevant_rank_uses_first_matching_result() {
        let rank = first_relevant_rank(
            &[
                "noise result",
                "April 2026 Security Expert decision is approved",
                "another relevant result",
            ],
            &[
                "Security Expert decision".to_string(),
                "Policy Expert decision".to_string(),
            ],
        );

        assert_eq!(rank, Some(2));
    }

    #[test]
    fn recall_and_pass_rate_default_to_one_for_empty_summary() {
        let summary = RetrievalSuiteSummary::default();

        assert_eq!(summary.recall_at_5(), 1.0);
        assert_eq!(summary.pass_rate(), 1.0);
        assert_eq!(summary.mrr(), 1.0);
        assert_eq!(summary.top_1_hit_rate(), 1.0);
        assert_eq!(summary.diversity_pass_rate(), None);
    }
}
