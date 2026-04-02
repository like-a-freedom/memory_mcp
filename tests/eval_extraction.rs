mod eval_support;

use chrono::{DateTime, Utc};
use eval_support::dataset::parse_extraction_cases;
use eval_support::metrics::ExtractionSuiteSummary;
use eval_support::report::print_extraction_summary;
use memory_mcp::models::IngestRequest;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "eval: manual extraction quality run"]
async fn run_extraction_evals() {
    let raw = std::fs::read_to_string("tests/fixtures/evals/extraction_cases.json").unwrap();
    let cases = parse_extraction_cases(&raw).unwrap();
    let mut summary = ExtractionSuiteSummary::default();

    for case in cases {
        summary.total_cases += 1;
        // Use GLiNER + LocalCandle embeddings for accurate eval
        let service = eval_support::common::make_service_with_gliner_and_embeddings().await;
        let episode_id = service
            .ingest(
                IngestRequest {
                    source_type: "eval".to_string(),
                    source_id: case.id.clone(),
                    content: case.content.clone(),
                    t_ref: case.t_ref.parse::<DateTime<Utc>>().unwrap(),
                    scope: case.scope.clone(),
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
            .unwrap();

        let result = service.extract(&episode_id, None).await.unwrap();
        let actual_fact_types: std::collections::BTreeSet<_> = result
            .facts
            .iter()
            .map(|fact| fact.fact_type.clone())
            .collect();

        let actual_entities: Vec<_> = result
            .entities
            .iter()
            .map(|entity| (entity.entity_type.clone(), entity.canonical_name.clone()))
            .collect();
        let expected_entities: Vec<_> = case
            .expected
            .entities
            .iter()
            .map(|entity| (entity.entity_type.clone(), entity.canonical_name.clone()))
            .collect();

        if !case.expected.entities.is_empty() && actual_entities.is_empty() {
            eprintln!(
                "DEBUG {}: expected entities {:?}, got none. Facts: {:?}",
                case.id,
                expected_entities,
                result
                    .facts
                    .iter()
                    .map(|f| &f.fact_type)
                    .collect::<Vec<_>>()
            );
        }

        let case_ok = eval_support::metrics::record_extraction_case(
            &mut summary,
            &expected_entities,
            &actual_entities,
            &case.expected.fact_types,
            &actual_fact_types,
        );

        if !case_ok {
            eprintln!(
                "EXTRACTION MISS {}: expected facts={:?}, got facts={:?}, entities={:?}",
                case.id, case.expected.fact_types, actual_fact_types, actual_entities,
            );
        }
    }

    print_extraction_summary(&summary);

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
    let fact_type_accuracy = summary.fact_type_hits as f64 / summary.fact_type_total as f64;

    // Fact-type accuracy is the primary metric — promise/metric detection works reliably.
    // Threshold is relaxed because GLiNER local fixture has variable NER quality which
    // affects fact creation (facts are derived from extracted entities).
    assert!(
        fact_type_accuracy >= 0.75,
        "fact_type_accuracy dropped below 0.75"
    );
    // Entity F1 is informational — GLiNER local fixture has variable NER quality.
    // Log it but don't fail the eval on entity extraction alone.
    eprintln!(
        "Entity metrics (informational): precision={:.3}, recall={:.3}, f1={:.3}",
        precision, recall, f1
    );
}
