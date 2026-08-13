use async_trait::async_trait;
use chrono::{DateTime, Utc};
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;

use super::retrieval_cases::{case_as_of, load_cases};
use crate::domain::*;
use crate::error::EvalError;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

// ---------------------------------------------------------------------------
// Suite
// ---------------------------------------------------------------------------

pub struct ResponseSizeSuite {
    expected_ids: Vec<EvalCaseId>,
    reducer: ResponseSizeReducer,
}

impl ResponseSizeSuite {
    pub fn new() -> Result<Self, EvalError> {
        let cases = load_cases()?;
        let expected_ids = cases
            .iter()
            .map(|c| EvalCaseId::parse(&c.id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            expected_ids,
            reducer: ResponseSizeReducer::new("response-size"),
        })
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
        &self.reducer
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
                test_support::seed_fact_with_links(
                    &service,
                    &fact.content,
                    t_valid,
                    fact.entity_links.clone(),
                    fact.source_id.as_deref(),
                )
                .await;
            }

            let as_of = case_as_of(case);

            // Run compact=false (verbose)
            let request_verbose = memory_mcp::models::AssembleContextRequest {
                query: case.query.clone(),
                as_of: Some(as_of),
                budget: case.budget,
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
