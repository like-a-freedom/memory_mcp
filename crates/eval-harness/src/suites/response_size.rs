use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Duration, Utc};
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;
use serde::Deserialize;

use crate::domain::*;
use crate::error::EvalError;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

// ---------------------------------------------------------------------------
// Case definitions — identical shape to retrieval suite (reuse the fixture)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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
#[allow(dead_code)]
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/retrieval_cases.json")
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

// ---------------------------------------------------------------------------
// Suite
// ---------------------------------------------------------------------------

pub struct ResponseSizeSuite {
    expected_ids: Vec<EvalCaseId>,
}

impl ResponseSizeSuite {
    pub fn new() -> Result<Self, EvalError> {
        let cases = load_cases()?;
        let expected_ids = cases
            .iter()
            .map(|c| EvalCaseId::parse(&c.id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { expected_ids })
    }
}

#[async_trait]
impl EvalSuite for ResponseSizeSuite {
    fn id(&self) -> &str {
        "response-size"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::RetrievalOnly
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    fn reducer(&self) -> &dyn crate::reducer::SuiteReducer {
        use std::sync::OnceLock;
        static R: OnceLock<ResponseSizeReducer> = OnceLock::new();
        R.get_or_init(|| ResponseSizeReducer::new("response-size"))
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let cases = match load_cases() {
            Ok(c) => c,
            Err(_) => return vec![],
        };

        let mut outcomes = Vec::with_capacity(cases.len());

        for case in &cases {
            let case_id = EvalCaseId::parse(&case.id).unwrap();
            let start = std::time::Instant::now();

            let (service, db_client) = test_support::make_service_with_client().await;

            // Seed entities
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

            // Seed edges
            for edge in &case.edges {
                if service
                    .relate(&edge.from_id, &edge.relation, &edge.to_id)
                    .await
                    .is_err()
                {
                    // Skip on edge failures — measurement-only suite
                    continue;
                }
            }

            // Seed communities
            for community in &case.communities {
                let Ok(updated_at) = community.updated_at.parse::<DateTime<Utc>>() else {
                    continue;
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

            // Seed facts
            for fact in &case.facts {
                let Ok(t_valid) = fact.t_valid.parse::<DateTime<Utc>>() else {
                    continue;
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

            // Run compact=false (verbose)
            let request_verbose = memory_mcp::models::AssembleContextRequest {
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
                compact: false,
            };

            let items = match AssembleContextCapability::assemble_context(
                &service.build_context(),
                request_verbose,
            )
            .await
            {
                Ok(items) => items,
                Err(_) => continue,
            };

            // Serialize verbose (no compact guard)
            let verbose_val = serde_json::to_string(&items).unwrap_or_default();
            let verbose_bytes = verbose_val.len();

            // Serialize compact (with guard)
            let _guard = memory_mcp::tools::compact::set_compact(true);
            let compact_val = serde_json::to_string(&items).unwrap_or_default();
            let compact_bytes = compact_val.len();
            drop(_guard);

            let delta_pct = if verbose_bytes > 0 {
                100.0 * (1.0 - compact_bytes as f64 / verbose_bytes as f64)
            } else {
                0.0
            };

            let mut metrics = std::collections::BTreeMap::new();
            metrics.insert("verbose_bytes".to_string(), verbose_bytes as f64);
            metrics.insert("compact_bytes".to_string(), compact_bytes as f64);
            metrics.insert("delta_pct".to_string(), delta_pct);
            metrics.insert("items".to_string(), items.len() as f64);

            let mut evidence = std::collections::BTreeMap::new();
            evidence.insert(
                "response_size".to_string(),
                MetricEvidence::Ratio {
                    numerator: compact_bytes as u64,
                    denominator: verbose_bytes as u64,
                },
            );

            outcomes.push(EvalCaseOutcome {
                case_key: CaseKey::parse("response-size", case_id.as_str()).unwrap(),
                mode: EvalMode::RetrievalOnly,
                split: CorpusSplit::Development,
                label_trust: LabelTrust::Official,
                status: CaseStatus::Passed,
                metrics,
                evidence,
                invalid_reason: None,
                failures: vec![],
                duration_ms: start.elapsed().as_millis() as u64,
                attempts: 1,
            });
        }

        outcomes
    }
}

// ---------------------------------------------------------------------------
// Reducer — computes median / mean / p95 byte savings across cases
// ---------------------------------------------------------------------------

use std::collections::BTreeMap;

use crate::artifact::SuiteSummary;
use crate::reducer::SuiteReducer;

pub(crate) struct ResponseSizeReducer {
    suite_id: SuiteId,
}

impl ResponseSizeReducer {
    pub(crate) fn new(suite_id: impl Into<String>) -> Self {
        Self {
            suite_id: SuiteId::parse(suite_id).expect("suite_id must not be empty"),
        }
    }
}

impl SuiteReducer for ResponseSizeReducer {
    fn suite_id(&self) -> &SuiteId {
        &self.suite_id
    }

    fn reduce(&self, outcomes: &[EvalCaseOutcome]) -> Result<Vec<SuiteSummary>, EvalError> {
        let mut passed = 0usize;
        let mut quality_failed = 0usize;
        let mut invalid = 0usize;

        let mut deltas: Vec<f64> = Vec::with_capacity(outcomes.len());
        let mut verbose_bytes_total: u64 = 0;
        let mut compact_bytes_total: u64 = 0;

        for outcome in outcomes {
            match outcome.status {
                CaseStatus::Passed => passed += 1,
                CaseStatus::QualityFailed => quality_failed += 1,
                CaseStatus::Invalid => invalid += 1,
            }

            if let Some(delta) = outcome.metrics.get("delta_pct") {
                deltas.push(*delta);
            }
            if let Some(vb) = outcome.metrics.get("verbose_bytes") {
                verbose_bytes_total += *vb as u64;
            }
            if let Some(cb) = outcome.metrics.get("compact_bytes") {
                compact_bytes_total += *cb as u64;
            }
        }

        deltas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let n = deltas.len();
        let mean_delta = if n > 0 {
            deltas.iter().sum::<f64>() / n as f64
        } else {
            0.0
        };
        let median_delta = if n > 0 { deltas[n / 2] } else { 0.0 };
        let p95_idx = if n > 0 {
            ((n as f64) * 0.95).ceil() as usize - 1
        } else {
            0
        };
        let p95_delta = if n > 0 && p95_idx < n {
            deltas[p95_idx]
        } else {
            0.0
        };
        let min_delta = deltas.first().copied().unwrap_or(0.0);
        let max_delta = deltas.last().copied().unwrap_or(0.0);

        let overall_savings_pct = if verbose_bytes_total > 0 {
            100.0 * (1.0 - compact_bytes_total as f64 / verbose_bytes_total as f64)
        } else {
            0.0
        };

        let mut metrics = BTreeMap::new();
        metrics.insert("mean_delta_pct".to_string(), mean_delta);
        metrics.insert("median_delta_pct".to_string(), median_delta);
        metrics.insert("p95_delta_pct".to_string(), p95_delta);
        metrics.insert("min_delta_pct".to_string(), min_delta);
        metrics.insert("max_delta_pct".to_string(), max_delta);
        metrics.insert("overall_savings_pct".to_string(), overall_savings_pct);

        let total = passed + quality_failed + invalid;

        Ok(vec![SuiteSummary {
            suite_id: self.suite_id.as_str().to_string(),
            mode: EvalMode::RetrievalOnly,
            total,
            passed,
            quality_failed,
            invalid,
            metrics,
        }])
    }
}
