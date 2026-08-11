//! Suite registry: the single home for "which suite ids exist, how to build a
//! suite, and how to reduce its outcomes".
//!
//! `main.rs` dispatches through `build_suite`; `merge_shards` reduces through
//! `reducer_for`; profile validation can assert coverage against the same id
//! space. Adding a backend therefore touches this module only.

use crate::error::EvalError;
use crate::profile::SuiteDecl;
use crate::reducer::{
    ClassificationReducer, CountReducer, RatioMetricSpec, RatioReducer, RetrievalReducer,
    SuiteReducer,
};
use crate::runner::EvalSuite;
use crate::suites::ner_quality;
use crate::suites::response_size::ResponseSizeReducer;

static E2E_SPECS: &[RatioMetricSpec] = &[RatioMetricSpec {
    evidence_key: "context_match",
    metric_name: "context_match_rate",
}];

static LIFECYCLE_SPECS: &[RatioMetricSpec] = &[
    RatioMetricSpec {
        evidence_key: "action_grounding",
        metric_name: "action_grounding_pass_rate",
    },
    RatioMetricSpec {
        evidence_key: "poisoning",
        metric_name: "poisoning_pass_rate",
    },
];

/// The reducer registered for a suite id. Unknown ids fall back to a
/// count-only reducer so merged artifacts stay honest (explicit pass/fail/
/// invalid counts, no fabricated metrics).
pub fn reducer_for(suite_id: &str) -> Box<dyn SuiteReducer> {
    match suite_id {
        "local-retrieval" => Box::new(RetrievalReducer::new("local-retrieval", 5)),
        "extraction" => Box::new(ClassificationReducer::new("extraction", "entity")),
        "claim-reconciliation" => {
            Box::new(ClassificationReducer::new("claim-reconciliation", "claim"))
        }
        "end-to-end" => Box::new(RatioReducer::new("end-to-end", E2E_SPECS)),
        "external-retrieval" => Box::new(RetrievalReducer::new("external-retrieval", 5)),
        "action-grounding" => Box::new(CountReducer::new("action-grounding")),
        "capacity" => Box::new(CountReducer::new("capacity")),
        "poisoning" => Box::new(CountReducer::new("poisoning")),
        "lifecycle" => Box::new(RatioReducer::new("lifecycle", LIFECYCLE_SPECS)),
        "downstream-qa" => Box::new(CountReducer::new("downstream-qa")),
        "response-size" => Box::new(ResponseSizeReducer::new("response-size")),
        other => ner_quality::reducer_for_suite(other)
            .unwrap_or_else(|| Box::new(CountReducer::new(other))),
    }
}

/// Builds the suite declared by `decl`. Returns `Ok(None)` for unknown ids;
/// `Err` when the suite exists but cannot be constructed (callers record an
/// empty-suite issue and continue).
pub fn build_suite(decl: &SuiteDecl) -> Result<Option<Box<dyn EvalSuite>>, EvalError> {
    match decl.id.as_str() {
        "local-retrieval" => Ok(Some(Box::new(
            crate::suites::retrieval::LocalRetrievalSuite::new()?,
        ))),
        "extraction" => Ok(Some(Box::new(
            crate::suites::extraction::ExtractionSuite::new()?,
        ))),
        "claim-reconciliation" => Ok(Some(Box::new(
            crate::suites::claims::ClaimReconciliationSuite::new()?,
        ))),
        "end-to-end" => Ok(Some(Box::new(
            crate::suites::end_to_end::EndToEndSuite::new(),
        ))),
        "external-retrieval" => build_external_retrieval(decl).map(Some),
        "action-grounding" => Ok(Some(Box::new(
            crate::suites::action_grounding::ActionGroundingSuite::new(),
        ))),
        "capacity" => Ok(Some(
            Box::new(crate::suites::capacity::CapacitySuite::new()),
        )),
        "poisoning" => Ok(Some(Box::new(
            crate::suites::poisoning::PoisoningSuite::new(),
        ))),
        "lifecycle" => Ok(Some(Box::new(
            crate::suites::lifecycle::LifecycleReleaseSuite::new(),
        ))),
        "downstream-qa" => Ok(Some(Box::new(
            crate::suites::downstream_qa::DownstreamQaSuite::new(),
        ))),
        "response-size" => Ok(Some(Box::new(
            crate::suites::response_size::ResponseSizeSuite::new()?,
        ))),
        other => ner_quality::build_suite(other),
    }
}

fn build_external_retrieval(decl: &SuiteDecl) -> Result<Box<dyn EvalSuite>, EvalError> {
    let Some(root) = decl.corpus_root.as_deref() else {
        return Err(EvalError::InvalidConfig(
            "external-retrieval requires corpus_root".into(),
        ));
    };
    let root = std::path::PathBuf::from(root);
    let manifest_path = root.join("manifest.json");
    let raw = std::fs::read_to_string(&manifest_path).map_err(|source| EvalError::Io {
        path: manifest_path,
        source,
    })?;
    let manifest = crate::corpus::manifest::CorpusManifest::parse(&raw)?;
    let kind =
        crate::corpus::adapters::DatasetKind::parse_name(&manifest.corpus_id).ok_or_else(|| {
            EvalError::InvalidConfig(format!("unsupported corpus {}", manifest.corpus_id))
        })?;
    let prepared = manifest.validate_at(&root)?;
    let cases = crate::corpus::adapters::load_and_normalize(kind, &prepared)?;
    Ok(Box::new(
        crate::suites::external_retrieval::ExternalRetrievalSuite::new(kind, cases),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(id: &str) -> SuiteDecl {
        SuiteDecl {
            id: id.into(),
            mode: None,
            corpus_root: None,
            expected_coverage: None,
        }
    }

    /// Every id a profile can declare builds a suite and has a registered
    /// reducer whose suite id matches — the coverage contract `main.rs` and
    /// `merge_shards` rely on.
    #[test]
    fn every_known_suite_builds_and_has_a_matching_reducer() {
        for id in [
            "local-retrieval",
            "extraction",
            "claim-reconciliation",
            "end-to-end",
            "action-grounding",
            "capacity",
            "poisoning",
            "lifecycle",
            "downstream-qa",
            "response-size",
            "ner-quality-anno",
            "ner-quality-regex",
            "ner-quality-anno-onnx",
            "ner-quality-gliner",
            "ner-quality-vago",
        ] {
            assert!(build_suite(&decl(id)).unwrap().is_some(), "{id} must build");
            assert_eq!(
                reducer_for(id).suite_id().as_str(),
                id,
                "{id} must have a registered reducer"
            );
        }
        // external-retrieval needs a prepared corpus root to build.
        assert!(build_suite(&decl("external-retrieval")).is_err());
    }

    #[test]
    fn unknown_suite_is_rejected_by_builder_and_falls_back_to_count_reducer() {
        assert!(build_suite(&decl("nope")).unwrap().is_none());
        let summaries = reducer_for("nope").reduce(&[]).unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].suite_id, "nope");
        assert_eq!(summaries[0].total, 0);
    }
}
