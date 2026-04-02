/// Per-tier statistics for retrieval eval reporting.
#[derive(Debug, Default, Clone)]
pub struct TierStats {
    pub total: usize,
    pub passed: usize,
    pub recall_sum: f64,
}

#[derive(Debug, Default, Clone)]
pub struct RetrievalSuiteSummary {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub recall_at_k_sum: f64,
    pub precision_at_k_sum: f64,
    pub reciprocal_rank_sum: f64,
    pub empty_when_irrelevant_hits: usize,
    /// Per-tier breakdowns keyed by tier name.
    pub tier_totals: std::collections::BTreeMap<String, TierStats>,
}

#[derive(Debug, Default, Clone)]
pub struct ExtractionSuiteSummary {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub entity_true_positive: usize,
    pub entity_false_positive: usize,
    pub entity_false_negative: usize,
    pub fact_type_hits: usize,
    pub fact_type_total: usize,
}

#[derive(Debug, Default, Clone)]
pub struct LatencySuiteSummary {
    pub ingest_ms: Vec<f64>,
    pub extract_ms: Vec<f64>,
    pub assemble_ms: Vec<f64>,
}

pub fn percentile_ms(values: &[f64], percentile: f64) -> f64 {
    assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let rank = ((sorted.len() - 1) as f64 * percentile).round() as usize;
    sorted[rank]
}

pub fn normalize_label(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

pub fn record_extraction_case(
    summary: &mut ExtractionSuiteSummary,
    expected_entities: &[(String, String)],
    actual_entities: &[(String, String)],
    expected_fact_types: &[String],
    actual_fact_types: &std::collections::BTreeSet<String>,
) -> bool {
    let expected_entity_set: std::collections::BTreeSet<_> = expected_entities
        .iter()
        .map(|(entity_type, name)| (normalize_label(entity_type), normalize_label(name)))
        .collect();
    let actual_entity_set: std::collections::BTreeSet<_> = actual_entities
        .iter()
        .map(|(entity_type, name)| (normalize_label(entity_type), normalize_label(name)))
        .collect();

    // Entity metrics are informational — track TP/FP/FN but don't fail on them
    for item in &expected_entity_set {
        if actual_entity_set.contains(item) {
            summary.entity_true_positive += 1;
        } else {
            summary.entity_false_negative += 1;
        }
    }

    for item in &actual_entity_set {
        if !expected_entity_set.contains(item) {
            summary.entity_false_positive += 1;
        }
    }

    // Fact-type accuracy is the primary pass/fail criterion
    let mut fact_types_ok = true;
    for fact_type in expected_fact_types {
        summary.fact_type_total += 1;
        if actual_fact_types.contains(fact_type) {
            summary.fact_type_hits += 1;
        } else {
            fact_types_ok = false;
        }
    }

    if fact_types_ok {
        summary.passed_cases += 1;
    }

    fact_types_ok
}

pub fn record_retrieval_case(
    summary: &mut RetrievalSuiteSummary,
    must_contain: &[String],
    must_not_contain: &[String],
    expect_empty: bool,
    contents: &[String],
    tier: &str,
) -> bool {
    let top_k = contents.len().max(1) as f64;
    let contains = |needle: &str| {
        let needle = normalize_label(needle);
        contents
            .iter()
            .any(|item| normalize_label(item).contains(&needle))
    };

    if expect_empty {
        if contents.is_empty() {
            summary.passed_cases += 1;
            summary.empty_when_irrelevant_hits += 1;
            summary.precision_at_k_sum += 1.0;
            summary.recall_at_k_sum += 1.0;
            summary.reciprocal_rank_sum += 1.0;
            return true;
        }
        return false;
    }

    let hits = must_contain
        .iter()
        .filter(|needle| contains(needle))
        .count();
    let forbidden_present = must_not_contain.iter().any(|needle| contains(needle));
    let recall = if must_contain.is_empty() {
        1.0
    } else {
        hits as f64 / must_contain.len() as f64
    };
    let precision = if contents.is_empty() {
        0.0
    } else {
        hits as f64 / top_k
    };
    let reciprocal_rank = must_contain
        .iter()
        .find_map(|needle| {
            let normalized = normalize_label(needle);
            contents
                .iter()
                .position(|item| normalize_label(item).contains(&normalized))
        })
        .map(|index| 1.0 / (index as f64 + 1.0))
        .unwrap_or(0.0);

    summary.recall_at_k_sum += recall;
    summary.precision_at_k_sum += precision;
    summary.reciprocal_rank_sum += reciprocal_rank;

    let case_ok = recall >= 1.0 && !forbidden_present;
    if case_ok {
        summary.passed_cases += 1;
    }

    // Update per-tier tracking
    let entry = summary.tier_totals.entry(tier.to_string()).or_default();
    entry.total += 1;
    if case_ok {
        entry.passed += 1;
    }
    entry.recall_sum += recall;

    case_ok
}

pub fn tier_recall(summary: &RetrievalSuiteSummary, tier: &str) -> Option<f64> {
    summary
        .tier_totals
        .get(tier)
        .filter(|stats| stats.total > 0)
        .map(|stats| stats.recall_sum / stats.total as f64)
}

pub fn tier_pass_rate(summary: &RetrievalSuiteSummary, tier: &str) -> Option<f64> {
    summary
        .tier_totals
        .get(tier)
        .filter(|stats| stats.total > 0)
        .map(|stats| stats.passed as f64 / stats.total as f64)
}
