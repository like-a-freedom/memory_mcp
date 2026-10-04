//! Fixture-gated end-to-end check: the real classic GLiNER checkpoint must
//! build through `ner_fixtures` and score the shared quality corpus without
//! error. Requires the local checkpoint under
//! `crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1/` (gitignored).
//! Run with `--ignored`.

use eval_harness::ner_fixtures;
use eval_harness::suites::ner_quality::{NerQualityCase, run_case};
use memory_mcp::config::NerExtractorKind;

async fn score_case(case_id: &str) {
    let Some(extractor) = ner_fixtures::build_extractor(NerExtractorKind::ClassicGliner).await
    else {
        panic!("GLiNER fixture missing; run with the checkpoint in place");
    };
    let cases: Vec<NerQualityCase> =
        eval_harness::suites::ner_quality::load_cases().expect("read corpus");
    let case = cases
        .iter()
        .find(|case| case.id == case_id)
        .unwrap_or_else(|| panic!("quality corpus must contain {case_id}"));
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

macro_rules! scored_case {
    ($name:ident, $id:literal) => {
        #[tokio::test]
        #[ignore = "requires the local GLiNER checkpoint; never downloads models"]
        async fn $name() {
            score_case($id).await;
        }
    };
}

scored_case!(gliner_russian_one, "q-ru-1");
scored_case!(gliner_russian_two, "q-ru-2");
scored_case!(gliner_russian_three, "q-ru-3");
scored_case!(gliner_english_one, "q-en-1");
scored_case!(gliner_english_two, "q-en-2");
scored_case!(gliner_english_three, "q-en-3");
scored_case!(gliner_english_four, "q-en-4");
scored_case!(gliner_mixed_one, "q-mixed-1");
scored_case!(gliner_mixed_two, "q-mixed-2");
scored_case!(gliner_mixed_three, "q-mixed-3");
