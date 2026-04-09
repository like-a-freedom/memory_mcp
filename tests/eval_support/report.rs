use super::metrics::RetrievalSuiteSummary;

pub fn render_retrieval_summary(suite_name: &str, summary: &RetrievalSuiteSummary) -> String {
    let diversity_pass_rate = summary
        .diversity_pass_rate()
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "n/a".to_string());
    let mut lines = vec![format!(
        "suite={} total={} passed={} recall_at_5={:.2} mrr={:.2} top1_hit_rate={:.2} diversity_pass_rate={} pass_rate={:.2}",
        suite_name,
        summary.total_cases,
        summary.passed_cases,
        summary.recall_at_5(),
        summary.mrr(),
        summary.top_1_hit_rate(),
        diversity_pass_rate,
        summary.pass_rate(),
    )];

    if let Some(unique_ratio) = summary.average_unique_source_episode_ratio() {
        lines.push(format!(
            "avg_unique_source_episode_ratio={:.2}",
            unique_ratio
        ));
    }
    if let Some(max_share) = summary.average_max_source_episode_share() {
        lines.push(format!("avg_max_source_episode_share={:.2}", max_share));
    }

    for (tier, total) in &summary.expected_tier_totals {
        let passed = summary
            .expected_tier_passed_cases
            .get(tier)
            .copied()
            .unwrap_or(0);
        let pass_rate = summary.expected_tier_pass_rate(tier).unwrap_or(1.0);
        lines.push(format!(
            "expected_tier={} total={} passed={} pass_rate={:.2}",
            tier, total, passed, pass_rate
        ));
    }
    for (tier, total) in &summary.actual_tier_totals {
        lines.push(format!("actual_tier={} total={}", tier, total));
    }

    lines.join("\n")
}

#[allow(dead_code)]
pub fn print_retrieval_summary(suite_name: &str, summary: &RetrievalSuiteSummary) {
    println!("{}", render_retrieval_summary(suite_name, summary));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval_support::metrics::RetrievalSuiteSummary;

    #[test]
    fn render_retrieval_summary_includes_expected_and_actual_tiers() {
        let mut summary = RetrievalSuiteSummary {
            total_cases: 2,
            passed_cases: 1,
            expected_hits: 2,
            matched_hits: 1,
            reciprocal_rank_sum: 1.0,
            top_1_hits: 1,
            expected_tier_totals: Default::default(),
            expected_tier_passed_cases: Default::default(),
            actual_tier_totals: Default::default(),
            ..RetrievalSuiteSummary::default()
        };
        summary.expected_tier_totals.insert("direct".to_string(), 2);
        summary
            .expected_tier_passed_cases
            .insert("direct".to_string(), 1);
        summary.actual_tier_totals.insert("direct".to_string(), 1);
        summary.actual_tier_totals.insert("graph".to_string(), 1);

        let rendered = render_retrieval_summary("eval_retrieval", &summary);

        assert!(
            rendered.contains(
                "suite=eval_retrieval total=2 passed=1 recall_at_5=0.50 mrr=0.50 top1_hit_rate=0.50 diversity_pass_rate=n/a pass_rate=0.50"
            )
        );
        assert!(rendered.contains("expected_tier=direct total=2 passed=1 pass_rate=0.50"));
        assert!(rendered.contains("actual_tier=direct total=1"));
        assert!(rendered.contains("actual_tier=graph total=1"));
    }
}
