use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use crate::domain::*;
use crate::error::EvalError;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

#[derive(Debug, Deserialize)]
struct RetrievalEvalCase {
    id: String,
    #[allow(dead_code)]
    description: String,
    query: String,
    scope: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    tags: Vec<String>,
    #[serde(default = "default_budget")]
    budget: i32,
    facts: Vec<SeedFact>,
    #[serde(default)]
    entities: Vec<SeedEntity>,
    #[serde(default)]
    communities: Vec<SeedCommunity>,
    #[serde(default)]
    edges: Vec<SeedEdge>,
    expected: RetrievalExpectation,
}

#[derive(Debug, Deserialize)]
struct SeedFact {
    content: String,
    t_valid: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    source_id: Option<String>,
    #[serde(default)]
    entity_links: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SeedEntity {
    entity_id: String,
    entity_type: String,
    canonical_name: String,
    #[serde(default)]
    aliases: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct SeedCommunity {
    community_id: String,
    member_entities: Vec<String>,
    summary: String,
    updated_at: String,
}

#[derive(Debug, Deserialize)]
struct SeedEdge {
    from_id: String,
    relation: String,
    to_id: String,
}

#[derive(Debug, Deserialize)]
struct RetrievalExpectation {
    #[allow(dead_code)]
    tier: String,
    must_contain: Vec<String>,
    #[serde(default)]
    must_not_contain: Vec<String>,
    #[serde(default = "default_min_recall_at_k")]
    min_recall_at_k: f64,
}

fn default_budget() -> i32 {
    5
}

fn default_min_recall_at_k() -> f64 {
    1.0
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/evals/retrieval_cases.json")
}

fn load_cases() -> Result<Vec<RetrievalEvalCase>, EvalError> {
    let raw = std::fs::read_to_string(fixture_path()).map_err(|source| EvalError::Io {
        path: fixture_path(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(EvalError::Artifact)
}

fn case_as_of(case: &RetrievalEvalCase) -> DateTime<Utc> {
    let latest = case
        .facts
        .iter()
        .filter_map(|f| f.t_valid.parse::<DateTime<Utc>>().ok())
        .chain(
            case.communities
                .iter()
                .filter_map(|c| c.updated_at.parse::<DateTime<Utc>>().ok()),
        )
        .max()
        .unwrap_or_else(Utc::now);

    std::cmp::max(Utc::now(), latest) + Duration::seconds(1)
}

pub struct LocalRetrievalSuite {
    expected_ids: Vec<EvalCaseId>,
}

impl LocalRetrievalSuite {
    pub fn new() -> Result<Self, EvalError> {
        let cases = load_cases()?;
        let expected_ids = cases
            .iter()
            .map(|c| EvalCaseId::parse(&c.id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { expected_ids })
    }

    async fn run_case(case: &RetrievalEvalCase) -> EvalCaseOutcome {
        let case_id = EvalCaseId::parse(&case.id).unwrap();
        let start = std::time::Instant::now();

        let (service, db_client) = test_support::make_service_with_client().await;

        for entity in &case.entities {
            test_support::seed_entity(
                &db_client,
                &case.scope,
                &entity.entity_id,
                &entity.entity_type,
                &entity.canonical_name,
                &entity.aliases,
            )
            .await;
        }
        for edge in &case.edges {
            if let Err(err) = service
                .relate(&edge.from_id, &edge.relation, &edge.to_id)
                .await
            {
                return EvalCaseOutcome {
                    case_key: CaseKey::parse("local-retrieval", case_id.as_str()).unwrap(),
                    mode: EvalMode::RetrievalOnly,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: Some(format!("failed to seed edge: {err}")),
                    failures: vec![],
                    duration_ms: start.elapsed().as_millis() as u64,
                    attempts: 1,
                };
            }
        }
        for community in &case.communities {
            let updated_at = match community.updated_at.parse::<DateTime<Utc>>() {
                Ok(t) => t,
                Err(err) => {
                    return EvalCaseOutcome {
                        case_key: CaseKey::parse("local-retrieval", case_id.as_str()).unwrap(),
                        mode: EvalMode::RetrievalOnly,
                        split: CorpusSplit::Development,
                        label_trust: LabelTrust::Official,
                        status: CaseStatus::Invalid,
                        metrics: std::collections::BTreeMap::new(),
                        evidence: std::collections::BTreeMap::new(),
                        invalid_reason: Some(format!("invalid community timestamp: {err}")),
                        failures: vec![],
                        duration_ms: start.elapsed().as_millis() as u64,
                        attempts: 1,
                    };
                }
            };
            test_support::seed_community(
                &db_client,
                &case.scope,
                &community.community_id,
                &community.member_entities,
                &community.summary,
                updated_at,
            )
            .await;
        }
        for fact in &case.facts {
            let t_valid = match fact.t_valid.parse::<DateTime<Utc>>() {
                Ok(t) => t,
                Err(err) => {
                    return EvalCaseOutcome {
                        case_key: CaseKey::parse("local-retrieval", case_id.as_str()).unwrap(),
                        mode: EvalMode::RetrievalOnly,
                        split: CorpusSplit::Development,
                        label_trust: LabelTrust::Official,
                        status: CaseStatus::Invalid,
                        metrics: std::collections::BTreeMap::new(),
                        evidence: std::collections::BTreeMap::new(),
                        invalid_reason: Some(format!("invalid fact timestamp: {err}")),
                        failures: vec![],
                        duration_ms: start.elapsed().as_millis() as u64,
                        attempts: 1,
                    };
                }
            };
            test_support::seed_fact_with_links_and_project(
                &service,
                &case.scope,
                &fact.content,
                t_valid,
                fact.entity_links.clone(),
                fact.project.as_deref(),
                fact.source_id.as_deref(),
            )
            .await;
        }

        let as_of = case_as_of(case);
        let items = match service
            .assemble_context(memory_mcp::models::AssembleContextRequest {
                query: case.query.clone(),
                scope: case.scope.clone(),
                as_of: Some(as_of),
                budget: case.budget,
                project: case.project.clone(),
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            })
            .await
        {
            Ok(items) => items,
            Err(err) => {
                return EvalCaseOutcome {
                    case_key: CaseKey::parse("local-retrieval", case_id.as_str()).unwrap(),
                    mode: EvalMode::RetrievalOnly,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: Some(format!("assemble_context failed: {err}")),
                    failures: vec![],
                    duration_ms: start.elapsed().as_millis() as u64,
                    attempts: 1,
                };
            }
        };

        let mut hits_at_k: u64 = 0;
        let mut first_relevant_rank: Option<u32> = None;
        let relevant_count = case.expected.must_contain.len() as u64;

        for (idx, item) in items.iter().take(5).enumerate() {
            let rank = (idx + 1) as u32;
            let matched = case.expected.must_contain.iter().any(|needle| {
                item.content.contains(needle.as_str())
                    || item.source_episode.contains(needle.as_str())
            });
            if matched {
                hits_at_k += 1;
                if first_relevant_rank.is_none() {
                    first_relevant_rank = Some(rank);
                }
            }
        }

        let unexpected: Vec<String> = case
            .expected
            .must_not_contain
            .iter()
            .filter(|needle| {
                items
                    .iter()
                    .any(|item| item.content.contains(needle.as_str()))
            })
            .cloned()
            .collect();

        let recall_at_k = if relevant_count > 0 {
            hits_at_k as f64 / relevant_count as f64
        } else {
            0.0
        };
        let mrr = if let Some(rank) = first_relevant_rank {
            1.0 / rank as f64
        } else {
            0.0
        };
        let top_1_hit_rate = if first_relevant_rank == Some(1) {
            1.0
        } else {
            0.0
        };

        let mut metric_map = std::collections::BTreeMap::new();
        metric_map.insert("recall_at_5".into(), recall_at_k);
        metric_map.insert("mrr".into(), mrr);
        metric_map.insert("top_1_hit_rate".into(), top_1_hit_rate);

        let mut evidence_map = std::collections::BTreeMap::new();
        evidence_map.insert(
            "retrieval".to_string(),
            MetricEvidence::retrieval(relevant_count, hits_at_k, first_relevant_rank, 5),
        );

        let meets_recall = recall_at_k >= case.expected.min_recall_at_k;
        let no_unexpected = unexpected.is_empty();

        let status = if meets_recall && no_unexpected {
            CaseStatus::Passed
        } else {
            CaseStatus::QualityFailed
        };

        EvalCaseOutcome {
            case_key: CaseKey::parse("local-retrieval", case_id.as_str()).unwrap(),
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Development,
            label_trust: LabelTrust::Official,
            status,
            metrics: metric_map,
            evidence: evidence_map,
            invalid_reason: None,
            failures: if !meets_recall {
                vec![format!(
                    "recall_at_5={recall_at_k:.4} < {}",
                    case.expected.min_recall_at_k
                )]
            } else if !no_unexpected {
                vec![format!("unexpected items: {unexpected:?}")]
            } else {
                vec![]
            },
            duration_ms: start.elapsed().as_millis() as u64,
            attempts: 1,
        }
    }
}

#[async_trait]
impl EvalSuite for LocalRetrievalSuite {
    fn id(&self) -> &str {
        "local-retrieval"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::RetrievalOnly
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    fn reducer(&self) -> &dyn crate::reducer::SuiteReducer {
        use std::sync::OnceLock;
        static R: OnceLock<&dyn crate::reducer::SuiteReducer> = OnceLock::new();
        *R.get_or_init(|| {
            &*Box::leak(Box::new(crate::reducer::RetrievalReducer::new(
                "local-retrieval",
                5,
            )))
        })
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let cases = match load_cases() {
            Ok(cases) => cases,
            Err(err) => {
                return vec![EvalCaseOutcome {
                    case_key: CaseKey::parse("local-retrieval", "fixture-load-error").unwrap(),
                    mode: EvalMode::RetrievalOnly,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: Some(err.to_string()),
                    failures: vec![],
                    duration_ms: 0,
                    attempts: 1,
                }];
            }
        };

        let mut outcomes = Vec::with_capacity(cases.len());
        for case in &cases {
            outcomes.push(Self::run_case(case).await);
        }
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_loads_and_has_cases() {
        let cases = load_cases().unwrap();
        assert!(
            cases.len() >= 50,
            "expected at least 50 cases, got {}",
            cases.len()
        );
    }

    #[test]
    fn case_ids_are_deterministic() {
        let cases = load_cases().unwrap();
        let ids: Vec<_> = cases.iter().map(|c| c.id.as_str()).collect();
        let cases2 = load_cases().unwrap();
        let ids2: Vec<_> = cases2.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ids2, "loading should be deterministic");
        assert_eq!(
            ids.len(),
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            "case IDs must be unique"
        );
    }

    #[tokio::test]
    async fn single_case_produces_valid_outcome() {
        let cases = load_cases().unwrap();
        let case = &cases[0];
        let outcome = LocalRetrievalSuite::run_case(case).await;
        assert_eq!(outcome.suite_id(), "local-retrieval");
        assert!(outcome.duration_ms > 0);
    }
}
