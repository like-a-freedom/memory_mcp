use async_trait::async_trait;
use chrono::{DateTime, Utc};
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;
use memory_mcp::service::capabilities::explain::ExplainCapability;

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

            let explain_input = items
                .iter()
                .map(|item| memory_mcp::models::ExplainItem {
                    fact_id: Some(item.fact_id.clone()),
                    content: item.content.clone(),
                    quote: item.quote.clone(),
                    source_episode: item.source_episode.clone(),
                    provenance: item.provenance.clone(),
                    ..Default::default()
                })
                .collect();
            let explain_items = match ExplainCapability::explain(
                &service.build_context(),
                memory_mcp::models::ExplainRequest {
                    context_pack: explain_input,
                    compact: false,
                },
                None,
            )
            .await
            {
                Ok(items) => items,
                Err(_) => continue,
            };
            let explain_verbose = serde_json::to_string(&explain_items).unwrap_or_default();
            let explain_verbose_bytes = explain_verbose.len();
            let _guard = memory_mcp::tools::compact::set_compact(true);
            let explain_compact = serde_json::to_string(&explain_items).unwrap_or_default();
            let explain_compact_bytes = explain_compact.len();
            drop(_guard);

            let assemble_delta_pct = if verbose_bytes > 0 {
                100.0 * (1.0 - compact_bytes as f64 / verbose_bytes as f64)
            } else {
                0.0
            };
            let explain_delta_pct = if explain_verbose_bytes > 0 {
                100.0 * (1.0 - explain_compact_bytes as f64 / explain_verbose_bytes as f64)
            } else {
                0.0
            };

            let mut metrics = std::collections::BTreeMap::new();
            metrics.insert("assemble_verbose_bytes".to_string(), verbose_bytes as f64);
            metrics.insert("assemble_compact_bytes".to_string(), compact_bytes as f64);
            metrics.insert("assemble_delta_pct".to_string(), assemble_delta_pct);
            metrics.insert(
                "explain_verbose_bytes".to_string(),
                explain_verbose_bytes as f64,
            );
            metrics.insert(
                "explain_compact_bytes".to_string(),
                explain_compact_bytes as f64,
            );
            metrics.insert("explain_delta_pct".to_string(), explain_delta_pct);
            metrics.insert("items".to_string(), items.len() as f64);

            let mut evidence = std::collections::BTreeMap::new();
            evidence.insert(
                "response_size".to_string(),
                MetricEvidence::Ratio {
                    numerator: compact_bytes as u64,
                    denominator: verbose_bytes as u64,
                },
            );

            let execution_invalid = verbose_bytes == 0
                || compact_bytes == 0
                || explain_verbose_bytes == 0
                || explain_compact_bytes == 0;
            outcomes.push(EvalCaseOutcome {
                case_key: CaseKey::parse("response-size", case_id.as_str()).unwrap(),
                mode: EvalMode::RetrievalOnly,
                split: CorpusSplit::Development,
                label_trust: LabelTrust::Official,
                status: if execution_invalid {
                    CaseStatus::Invalid
                } else {
                    CaseStatus::Passed
                },
                metrics,
                evidence,
                invalid_reason: execution_invalid.then(|| {
                    "response-size serialization produced an empty response class".to_string()
                }),
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

        let mut assemble_deltas: Vec<f64> = Vec::with_capacity(outcomes.len());
        let mut explain_deltas: Vec<f64> = Vec::with_capacity(outcomes.len());
        let mut assemble_verbose_bytes_total: u64 = 0;
        let mut assemble_compact_bytes_total: u64 = 0;
        let mut explain_verbose_bytes_total: u64 = 0;
        let mut explain_compact_bytes_total: u64 = 0;

        for outcome in outcomes {
            match outcome.status {
                CaseStatus::Passed => passed += 1,
                CaseStatus::QualityFailed => quality_failed += 1,
                CaseStatus::Invalid => invalid += 1,
            }

            if let Some(delta) = outcome.metrics.get("assemble_delta_pct") {
                assemble_deltas.push(*delta);
            }
            if let Some(delta) = outcome.metrics.get("explain_delta_pct") {
                explain_deltas.push(*delta);
            }
            if let Some(vb) = outcome.metrics.get("assemble_verbose_bytes") {
                assemble_verbose_bytes_total += *vb as u64;
            }
            if let Some(cb) = outcome.metrics.get("assemble_compact_bytes") {
                assemble_compact_bytes_total += *cb as u64;
            }
            if let Some(vb) = outcome.metrics.get("explain_verbose_bytes") {
                explain_verbose_bytes_total += *vb as u64;
            }
            if let Some(cb) = outcome.metrics.get("explain_compact_bytes") {
                explain_compact_bytes_total += *cb as u64;
            }
        }

        if assemble_deltas.is_empty() || explain_deltas.is_empty() {
            return Err(EvalError::InvalidInput(
                "response-size requires non-empty assemble and explain classes".into(),
            ));
        }

        fn summary(
            deltas: &mut [f64],
            verbose: u64,
            compact: u64,
        ) -> (f64, f64, f64, f64, f64, f64) {
            deltas.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            let n = deltas.len();
            let mean = if n > 0 {
                deltas.iter().sum::<f64>() / n as f64
            } else {
                0.0
            };
            let median = if n > 0 { deltas[n / 2] } else { 0.0 };
            let p95 = if n > 0 {
                let index = ((n as f64) * 0.95).ceil() as usize - 1;
                deltas.get(index).copied().unwrap_or(0.0)
            } else {
                0.0
            };
            let min = deltas.first().copied().unwrap_or(0.0);
            let max = deltas.last().copied().unwrap_or(0.0);
            let overall = if verbose > 0 {
                100.0 * (1.0 - compact as f64 / verbose as f64)
            } else {
                0.0
            };
            (mean, median, p95, min, max, overall)
        }

        let (
            assemble_mean,
            assemble_median,
            assemble_p95,
            assemble_min,
            assemble_max,
            assemble_overall,
        ) = summary(
            &mut assemble_deltas,
            assemble_verbose_bytes_total,
            assemble_compact_bytes_total,
        );
        let (explain_mean, explain_median, explain_p95, explain_min, explain_max, explain_overall) =
            summary(
                &mut explain_deltas,
                explain_verbose_bytes_total,
                explain_compact_bytes_total,
            );

        let mut metrics = BTreeMap::new();
        metrics.insert("assemble_mean_delta_pct".to_string(), assemble_mean);
        metrics.insert("assemble_median_delta_pct".to_string(), assemble_median);
        metrics.insert("assemble_p95_delta_pct".to_string(), assemble_p95);
        metrics.insert("assemble_min_delta_pct".to_string(), assemble_min);
        metrics.insert("assemble_max_delta_pct".to_string(), assemble_max);
        metrics.insert("assemble_overall_savings_pct".to_string(), assemble_overall);
        metrics.insert("assemble_cases".to_string(), assemble_deltas.len() as f64);
        metrics.insert("explain_mean_delta_pct".to_string(), explain_mean);
        metrics.insert("explain_median_delta_pct".to_string(), explain_median);
        metrics.insert("explain_p95_delta_pct".to_string(), explain_p95);
        metrics.insert("explain_min_delta_pct".to_string(), explain_min);
        metrics.insert("explain_max_delta_pct".to_string(), explain_max);
        metrics.insert("explain_overall_savings_pct".to_string(), explain_overall);
        metrics.insert("explain_cases".to_string(), explain_deltas.len() as f64);

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

#[cfg(test)]
mod tests {
    use super::ResponseSizeReducer;
    use crate::domain::{
        CaseKey, CaseStatus, CorpusSplit, EvalCaseOutcome, EvalMode, LabelTrust, MetricEvidence,
    };
    use crate::reducer::SuiteReducer;
    use std::collections::{BTreeMap, BTreeSet};

    fn outcome(id: &str, assemble: f64, explain: f64) -> EvalCaseOutcome {
        let mut metrics = BTreeMap::new();
        metrics.insert("assemble_delta_pct".into(), assemble);
        metrics.insert("explain_delta_pct".into(), explain);
        metrics.insert("assemble_verbose_bytes".into(), 100.0);
        metrics.insert("assemble_compact_bytes".into(), 100.0 - assemble);
        metrics.insert("explain_verbose_bytes".into(), 100.0);
        metrics.insert("explain_compact_bytes".into(), 100.0 - explain);
        EvalCaseOutcome {
            case_key: CaseKey::parse("response-size", id).unwrap(),
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Development,
            label_trust: LabelTrust::Official,
            status: CaseStatus::Passed,
            metrics,
            evidence: BTreeMap::from([("response_size".into(), MetricEvidence::ratio(1, 2))]),
            invalid_reason: None,
            failures: vec![],
            duration_ms: 0,
            attempts: 1,
        }
    }

    #[test]
    fn reducer_reports_independent_response_classes() {
        let summaries = ResponseSizeReducer::new("response-size")
            .reduce(&[outcome("a", 20.0, 40.0), outcome("b", 30.0, 50.0)])
            .unwrap();
        let metrics = &summaries[0].metrics;
        assert_eq!(metrics.get("assemble_cases"), Some(&2.0));
        assert_eq!(metrics.get("explain_cases"), Some(&2.0));
        assert!(metrics.get("assemble_overall_savings_pct").unwrap() > &0.0);
        assert!(metrics.get("explain_overall_savings_pct").unwrap() > &0.0);
        let non_zero: BTreeSet<_> = ["assemble", "explain"].into_iter().collect();
        let reported: BTreeSet<_> = metrics
            .keys()
            .filter_map(|key| key.strip_suffix("_cases"))
            .collect();
        assert_eq!(reported, non_zero);
    }

    #[test]
    fn reducer_rejects_missing_response_class() {
        let mut only_assemble = outcome("a", 20.0, 40.0);
        only_assemble.metrics.remove("explain_delta_pct");
        let error = ResponseSizeReducer::new("response-size")
            .reduce(&[only_assemble])
            .unwrap_err();
        assert!(error.to_string().contains("non-empty assemble and explain"));
    }
}
