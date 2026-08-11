//! Fixture-gated end-to-end check: the real classic GLiNER checkpoint must
//! build through `ner_fixtures` and score the shared quality corpus without
//! error. Requires the local checkpoint under
//! `crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1/` (gitignored).
//! Run with `--ignored`.

use eval_harness::ner_fixtures;
use eval_harness::suites::ner_quality::{NerQualityCase, run_case};
use memory_mcp::config::NerExtractorKind;

#[tokio::test]
#[ignore = "requires the local GLiNER checkpoint under crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1/"]
async fn real_gliner_scores_quality_corpus() {
    let Some(extractor) = ner_fixtures::build_extractor(NerExtractorKind::ClassicGliner).await
    else {
        panic!("GLiNER fixture missing; run with the checkpoint in place");
    };
    let cases: Vec<NerQualityCase> =
        eval_harness::suites::ner_quality::load_cases().expect("read corpus");
    assert_eq!(cases.len(), 10);
    for case in &cases {
        let outcome = run_case("ner-quality-gliner", extractor.as_ref(), case).await;
        assert!(
            outcome.status == eval_harness::CaseStatus::Passed
                || outcome.status == eval_harness::CaseStatus::QualityFailed,
            "case {} must produce a scored outcome, got {:?}",
            case.id,
            outcome.status
        );
        assert!(
            outcome.metrics.contains_key("entity_mention_f1"),
            "case {} must carry entity_mention_f1",
            case.id
        );
    }
}
