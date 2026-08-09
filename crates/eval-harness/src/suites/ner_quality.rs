//! Per-extractor NER quality evaluation.
//!
//! One suite instance per `NER_EXTRACTOR` backend. All instances score the
//! same corpus (`evals/corpora/ner/ner_quality.json`) so the suite summaries
//! render a comparable precision/recall/F1 matrix. Mention matching is
//! case-insensitive on canonical names; typed match is a per-case diagnostic
//! because lightweight backends (regex, anno) use their own type vocabularies.

use std::collections::BTreeSet;
use std::path::PathBuf;

use async_trait::async_trait;
use memory_mcp::config::NerExtractorKind;
use memory_mcp::service::EntityExtractor;
use serde::Deserialize;

use crate::domain::*;
use crate::error::EvalError;
use crate::ner_fixtures;
use crate::reducer::{ClassificationReducer, CountReducer, SuiteReducer};
use crate::runner::{EvalSuite, RunContext};

#[derive(Debug, Clone, Deserialize)]
pub struct NerQualityCase {
    pub id: String,
    pub language: String,
    pub text: String,
    pub labels: Vec<String>,
    pub entities: Vec<NerQualityEntity>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NerQualityEntity {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub label: String,
}

#[derive(Deserialize)]
struct CorpusFile {
    #[allow(dead_code)]
    schema_version: u32,
    #[allow(dead_code)]
    fixture_status: String,
    #[allow(dead_code)]
    languages: Vec<String>,
    cases: Vec<NerQualityCase>,
}

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evals/corpora/ner/ner_quality.json")
}

fn load_cases() -> Result<Vec<NerQualityCase>, EvalError> {
    let path = corpus_path();
    let raw = std::fs::read_to_string(&path).map_err(|source| EvalError::Io { path, source })?;
    let corpus: CorpusFile = serde_json::from_str(&raw).map_err(EvalError::Artifact)?;
    Ok(corpus.cases)
}

/// Maps `ner-quality-*` suite ids to their extractor kind.
fn kind_for_id(suite_id: &str) -> Option<NerExtractorKind> {
    match suite_id {
        "ner-quality-anno" => Some(NerExtractorKind::Anno),
        "ner-quality-regex" => Some(NerExtractorKind::Regex),
        "ner-quality-anno-onnx" => Some(NerExtractorKind::AnnoOnnx),
        "ner-quality-gliner" => Some(NerExtractorKind::ClassicGliner),
        "ner-quality-vago" => Some(NerExtractorKind::SauerkrautLfm25),
        _ => None,
    }
}

/// Pushes the `NerQualitySuite` for `suite_id` (must be a `ner-quality-*` id).
pub fn register(suite_id: &str, suites: &mut Vec<Box<dyn EvalSuite>>) -> Result<(), EvalError> {
    let Some(kind) = kind_for_id(suite_id) else {
        return Ok(());
    };
    let suite = NerQualitySuite::new(suite_id.to_string(), kind)?;
    suites.push(Box::new(suite));
    Ok(())
}

/// The reducer depends on fixture availability: when the checkpoint is absent
/// the run emits only `Invalid` outcomes, and a count-based reducer keeps the
/// report honest (explicit per-case reasons) instead of presenting zero-valued
/// classification metrics. With a present fixture the classification reducer is
/// used; it degrades to zeroes if an extractor produces no predictions rather
/// than failing the run.
enum NerSuiteReducer {
    Class(ClassificationReducer),
    Count(CountReducer),
}

pub struct NerQualitySuite {
    id: String,
    kind: NerExtractorKind,
    cases: Vec<NerQualityCase>,
    expected_ids: Vec<EvalCaseId>,
    reducer: NerSuiteReducer,
}

impl NerQualitySuite {
    fn new(id: String, kind: NerExtractorKind) -> Result<Self, EvalError> {
        let cases = load_cases()?;
        let expected_ids = cases
            .iter()
            .map(|c| EvalCaseId::parse(&c.id))
            .collect::<Result<Vec<_>, _>>()?;
        let reducer = if ner_fixtures::fixture_present(kind) {
            NerSuiteReducer::Class(ClassificationReducer::new(id.clone(), "entity_mention"))
        } else {
            NerSuiteReducer::Count(CountReducer::new(id.clone()))
        };
        Ok(Self {
            id,
            kind,
            cases,
            expected_ids,
            reducer,
        })
    }
}

#[async_trait]
impl EvalSuite for NerQualitySuite {
    fn id(&self) -> &str {
        &self.id
    }

    fn mode(&self) -> EvalMode {
        EvalMode::Performance
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    fn reducer(&self) -> &dyn SuiteReducer {
        match &self.reducer {
            NerSuiteReducer::Class(reducer) => reducer,
            NerSuiteReducer::Count(reducer) => reducer,
        }
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let fixture_present = ner_fixtures::fixture_present(self.kind);
        if !fixture_present {
            return self
                .cases
                .iter()
                .map(|case| invalid_outcome(&self.id, case, "fixture unavailable"))
                .collect();
        }
        let Some(extractor) = ner_fixtures::build_extractor(self.kind).await else {
            return self
                .cases
                .iter()
                .map(|case| invalid_outcome(&self.id, case, "fixture unavailable"))
                .collect();
        };
        let mut outcomes = Vec::with_capacity(self.cases.len());
        for case in &self.cases {
            outcomes.push(run_case(&self.id, extractor.as_ref(), case).await);
        }
        outcomes
    }
}

fn invalid_outcome(suite_id: &str, case: &NerQualityCase, reason: &str) -> EvalCaseOutcome {
    EvalCaseOutcome {
        case_key: CaseKey::parse(suite_id, case.id.as_str()).expect("valid case key"),
        mode: EvalMode::Performance,
        split: CorpusSplit::Test,
        label_trust: LabelTrust::Official,
        status: CaseStatus::Invalid,
        metrics: std::collections::BTreeMap::new(),
        evidence: std::collections::BTreeMap::new(),
        invalid_reason: Some(reason.to_string()),
        failures: vec![],
        duration_ms: 0,
        attempts: 1,
    }
}

pub async fn run_case(
    suite_id: &str,
    extractor: &dyn EntityExtractor,
    case: &NerQualityCase,
) -> EvalCaseOutcome {
    let start = std::time::Instant::now();

    let expected_names: BTreeSet<String> = case
        .entities
        .iter()
        .map(|e| e.text.to_lowercase())
        .collect();

    let predicted = match extractor
        .extract_candidates_with_labels(&case.text, &case.labels)
        .await
    {
        Ok(candidates) => candidates,
        Err(err) => {
            let mut outcome = invalid_outcome(suite_id, case, &format!("extraction failed: {err}"));
            outcome.duration_ms = start.elapsed().as_millis() as u64;
            return outcome;
        }
    };

    let predicted_names: BTreeSet<String> = predicted
        .iter()
        .map(|c| c.canonical_name.to_lowercase())
        .collect();

    let tp = expected_names.intersection(&predicted_names).count() as u64;
    let fp = (predicted_names.len() as u64).saturating_sub(tp);
    let fn_ = (expected_names.len() as u64).saturating_sub(tp);

    let evidence = MetricEvidence::classification(tp, fp, fn_, 0);
    let mut metrics = crate::metrics::render_case_metrics(
        &evidence,
        &crate::metrics::CaseMetricNames::classification("entity_mention"),
    );

    // Suite-local diagnostic (not gate-consumed): typed recall-ish score over
    // the expected set, so users see where backends name-match but mislabel.
    let typed_tp = case
        .entities
        .iter()
        .filter(|expected| {
            predicted.iter().any(|candidate| {
                candidate.canonical_name.to_lowercase() == expected.text.to_lowercase()
                    && candidate.entity_type.to_lowercase() == expected.label
            })
        })
        .count() as u64;
    // Unique predicted names: duplicate candidates must not deflate precision.
    let typed_precision = if predicted_names.is_empty() {
        0.0
    } else {
        typed_tp as f64 / predicted_names.len() as f64
    };
    let typed_recall = if expected_names.is_empty() {
        1.0
    } else {
        typed_tp as f64 / expected_names.len() as f64
    };
    let typed_f1 = if typed_precision + typed_recall == 0.0 {
        0.0
    } else {
        2.0 * typed_precision * typed_recall / (typed_precision + typed_recall)
    };
    metrics.insert("ner_typed_f1".into(), typed_f1);

    let mut failures = Vec::new();
    for expected in &case.entities {
        if !predicted_names.contains(&expected.text.to_lowercase()) {
            failures.push(format!("missing mention `{}`", expected.text));
        }
    }
    for candidate in &predicted {
        if !expected_names.contains(&candidate.canonical_name.to_lowercase()) {
            failures.push(format!("unexpected mention `{}`", candidate.canonical_name));
        }
    }

    let case_passed = tp == expected_names.len() as u64 && fp == 0;
    let mut evidence_map = std::collections::BTreeMap::new();
    evidence_map.insert("classification".to_string(), evidence);

    EvalCaseOutcome {
        case_key: CaseKey::parse(suite_id, case.id.as_str()).expect("valid case key"),
        mode: EvalMode::Performance,
        split: CorpusSplit::Test,
        label_trust: LabelTrust::Official,
        status: if case_passed {
            CaseStatus::Passed
        } else {
            CaseStatus::QualityFailed
        },
        metrics,
        evidence: evidence_map,
        invalid_reason: None,
        failures,
        duration_ms: start.elapsed().as_millis() as u64,
        attempts: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_mcp::service::EntityExtractor;

    /// Scripted extractor: returns a fixed candidate list per case text.
    struct FakeExtractor {
        by_text: std::collections::BTreeMap<String, Vec<memory_mcp::models::EntityCandidate>>,
    }

    #[async_trait::async_trait]
    impl EntityExtractor for FakeExtractor {
        fn provider_name(&self) -> &'static str {
            "fake"
        }
        async fn extract_candidates(
            &self,
            _content: &str,
        ) -> Result<Vec<memory_mcp::models::EntityCandidate>, memory_mcp::MemoryError> {
            Ok(Vec::new())
        }
        async fn extract_candidates_with_labels(
            &self,
            content: &str,
            _labels: &[String],
        ) -> Result<Vec<memory_mcp::models::EntityCandidate>, memory_mcp::MemoryError> {
            Ok(self.by_text.get(content).cloned().unwrap_or_default())
        }
    }

    fn candidate(name: &str, entity_type: &str) -> memory_mcp::models::EntityCandidate {
        memory_mcp::models::EntityCandidate {
            entity_type: entity_type.to_string(),
            canonical_name: name.to_string(),
            aliases: Vec::new(),
        }
    }

    fn sample_cases() -> Vec<NerQualityCase> {
        vec![
            NerQualityCase {
                id: "q-en-1".into(),
                language: "en".into(),
                text: "Alice Smith from OpenAI presented the Surface Laptop 6 at Build 2026 in Seattle.".into(),
                labels: vec![
                    "person".into(),
                    "company".into(),
                    "product".into(),
                    "event".into(),
                    "location".into(),
                ],
                entities: vec![
                    NerQualityEntity { start: 0, end: 11, text: "Alice Smith".into(), label: "person".into() },
                    NerQualityEntity { start: 17, end: 23, text: "OpenAI".into(), label: "company".into() },
                    NerQualityEntity { start: 72, end: 79, text: "Seattle".into(), label: "location".into() },
                ],
            },
            NerQualityCase {
                id: "q-en-2".into(),
                language: "en".into(),
                text: "At Cloud Summit 2026 in Berlin, Bob Jones and DeepMind compared Pixel 8 Pro with PostgreSQL.".into(),
                labels: vec![
                    "person".into(),
                    "company".into(),
                    "product".into(),
                    "event".into(),
                    "location".into(),
                    "technology".into(),
                ],
                entities: vec![
                    NerQualityEntity { start: 3, end: 20, text: "Cloud Summit 2026".into(), label: "event".into() },
                    NerQualityEntity { start: 32, end: 41, text: "Bob Jones".into(), label: "person".into() },
                ],
            },
        ]
    }

    fn perfect_extractor() -> FakeExtractor {
        let cases = sample_cases();
        FakeExtractor {
            by_text: std::collections::BTreeMap::from([
                (
                    cases[0].text.clone(),
                    vec![
                        candidate("Alice Smith", "person"),
                        candidate("OpenAI", "company"),
                        candidate("Seattle", "location"),
                    ],
                ),
                (
                    cases[1].text.clone(),
                    vec![
                        candidate("Cloud Summit 2026", "event"),
                        candidate("Bob Jones", "person"),
                    ],
                ),
            ]),
        }
    }

    #[tokio::test]
    async fn perfect_extractor_passes_all_cases() {
        let cases = sample_cases();
        let extractor = perfect_extractor();
        for case in &cases {
            let outcome = run_case("ner-quality-fake", &extractor, case).await;
            assert_eq!(outcome.status, CaseStatus::Passed, "case {}", case.id);
            let f1 = outcome.metrics["entity_mention_f1"];
            assert!((f1 - 1.0).abs() < 1e-9, "case {}: f1 = {f1}", case.id);
        }
    }

    #[tokio::test]
    async fn extra_and_missing_mentions_are_scored() {
        let extractor = FakeExtractor {
            by_text: {
                let cases = sample_cases();
                std::collections::BTreeMap::from([(
                    cases[0].text.clone(),
                    vec![
                        candidate("Alice Smith", "person"),
                        candidate("OpenAI", "company"),
                        candidate("NotACorpusEntity", "company"), // FP
                                                                  // Seattle missing -> FN
                    ],
                )])
            },
        };
        let case = sample_cases().into_iter().next().unwrap();
        let outcome = run_case("ner-quality-fake", &extractor, &case).await;
        assert_eq!(outcome.status, CaseStatus::QualityFailed);
        // Case-level: tp=2, fp=1, fn=1 -> precision 2/3, recall 2/3, f1 2/3.
        assert!((outcome.metrics["entity_mention_precision"] - 2.0 / 3.0).abs() < 1e-9);
        assert!((outcome.metrics["entity_mention_recall"] - 2.0 / 3.0).abs() < 1e-9);
        assert!((outcome.metrics["entity_mention_f1"] - 2.0 / 3.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn typed_diagnostic_punishes_label_mismatch() {
        let extractor = FakeExtractor {
            by_text: {
                let cases = sample_cases();
                std::collections::BTreeMap::from([(
                    cases[0].text.clone(),
                    vec![
                        candidate("Alice Smith", "company"), // name ok, label wrong
                        candidate("OpenAI", "company"),
                        candidate("Seattle", "location"),
                    ],
                )])
            },
        };
        let case = sample_cases().into_iter().next().unwrap();
        let outcome = run_case("ner-quality-fake", &extractor, &case).await;
        assert_eq!(
            outcome.status,
            CaseStatus::Passed,
            "mention match is perfect"
        );
        assert!(
            outcome.metrics["ner_typed_f1"] < 1.0,
            "typed diagnostic must reflect the label mismatch"
        );
    }
}
