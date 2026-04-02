use super::metrics::{
    ExtractionSuiteSummary, LatencySuiteSummary, RetrievalSuiteSummary, percentile_ms,
};

pub fn print_retrieval_summary(summary: &RetrievalSuiteSummary) {
    let total = summary.total_cases.max(1) as f64;
    println!(
        "suite=eval_retrieval total={} passed={} recall_at_5={:.3} precision_at_5={:.3} mrr={:.3} empty_when_irrelevant={:.3}",
        summary.total_cases,
        summary.passed_cases,
        summary.recall_at_k_sum / total,
        summary.precision_at_k_sum / total,
        summary.reciprocal_rank_sum / total,
        summary.empty_when_irrelevant_hits as f64 / total,
    );

    // Print per-tier breakdowns
    if !summary.tier_totals.is_empty() {
        println!("--- tier breakdown ---");
        for (tier, stats) in &summary.tier_totals {
            let recall = stats.recall_sum / stats.total.max(1) as f64;
            let pass_rate = stats.passed as f64 / stats.total.max(1) as f64;
            println!(
                "  tier={tier} total={} passed={} recall_at_5={recall:.3} pass_rate={pass_rate:.3}",
                stats.total, stats.passed
            );
        }
    }
}

pub fn print_extraction_summary(summary: &ExtractionSuiteSummary) {
    let tp = summary.entity_true_positive as f64;
    let fp = summary.entity_false_positive as f64;
    let fn_ = summary.entity_false_negative as f64;
    let precision = if (tp + fp) == 0.0 {
        0.0
    } else {
        tp / (tp + fp)
    };
    let recall = if (tp + fn_) == 0.0 {
        0.0
    } else {
        tp / (tp + fn_)
    };
    let f1 = if (precision + recall) == 0.0 {
        0.0
    } else {
        2.0 * precision * recall / (precision + recall)
    };
    let fact_type_accuracy = if summary.fact_type_total == 0 {
        0.0
    } else {
        summary.fact_type_hits as f64 / summary.fact_type_total as f64
    };

    println!(
        "suite=eval_extraction total={} passed={} entity_precision={:.3} entity_recall={:.3} entity_f1={:.3} fact_type_accuracy={:.3}",
        summary.total_cases, summary.passed_cases, precision, recall, f1, fact_type_accuracy,
    );
}

pub fn print_latency_summary(summary: &LatencySuiteSummary) {
    println!(
        "suite=eval_latency ingest_p50_ms={:.2} ingest_p95_ms={:.2} ingest_p99_ms={:.2} extract_p50_ms={:.2} extract_p95_ms={:.2} extract_p99_ms={:.2} assemble_p50_ms={:.2} assemble_p95_ms={:.2} assemble_p99_ms={:.2}",
        percentile_ms(&summary.ingest_ms, 0.50),
        percentile_ms(&summary.ingest_ms, 0.95),
        percentile_ms(&summary.ingest_ms, 0.99),
        percentile_ms(&summary.extract_ms, 0.50),
        percentile_ms(&summary.extract_ms, 0.95),
        percentile_ms(&summary.extract_ms, 0.99),
        percentile_ms(&summary.assemble_ms, 0.50),
        percentile_ms(&summary.assemble_ms, 0.95),
        percentile_ms(&summary.assemble_ms, 0.99),
    );
}
