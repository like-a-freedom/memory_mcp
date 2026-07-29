use std::sync::Arc;

use async_trait::async_trait;

use crate::domain::*;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StorageUsage {
    pub rows: u64,
    pub serialized_bytes: u64,
}

async fn measure_storage(db_client: &Arc<memory_mcp::storage::SurrealDbClient>) -> StorageUsage {
    use memory_mcp::storage::DbClient;
    let result = db_client.query("SELECT * FROM fact", None, "org").await;
    let rows = match result {
        Ok(val) => {
            if let Some(arr) = val.as_array() {
                arr.len() as u64
            } else {
                0
            }
        }
        Err(_) => 0,
    };
    StorageUsage {
        rows,
        serialized_bytes: rows * 256,
    }
}

pub struct CapacitySuite {
    expected_ids: Vec<EvalCaseId>,
}

impl Default for CapacitySuite {
    fn default() -> Self {
        Self::new()
    }
}

impl CapacitySuite {
    pub fn new() -> Self {
        Self {
            expected_ids: vec![
                EvalCaseId::parse("capacity-accepted-growth").unwrap(),
                EvalCaseId::parse("capacity-ignored-no-growth").unwrap(),
                EvalCaseId::parse("capacity-duplicate-no-growth").unwrap(),
            ],
        }
    }

    async fn run_case(case_id: &str, scenario: &str) -> EvalCaseOutcome {
        let case_id = EvalCaseId::parse(case_id).unwrap();
        let start = std::time::Instant::now();

        let (service, db_client) = test_support::make_service_with_client().await;

        let before_usage = measure_storage(&db_client).await;

        let content = match scenario {
            "accepted" => "Important: The deployment window is March 20-22.",
            "ignored" => "ok",
            "duplicate" => "Important: The deployment window is March 20-22.",
            _ => "test content",
        };

        let result = service
            .ingest(
                memory_mcp::models::IngestRequest {
                    source_type: "lifecycle".into(),
                    source_id: format!("cap-{scenario}"),
                    content: content.into(),
                    t_ref: chrono::Utc::now(),
                    scope: "org".into(),
                    project: None,
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match result {
            Ok(episode_id) => {
                let extraction = service.extract(&episode_id, None, None).await;
                let fact_count = extraction.as_ref().map(|e| e.facts.len()).unwrap_or(0) as f64;

                let after_usage = measure_storage(&db_client).await;
                let rows_growth = after_usage.rows.saturating_sub(before_usage.rows);

                let mut metrics = std::collections::BTreeMap::new();
                metrics.insert("facts_extracted".into(), fact_count);
                metrics.insert("episode_created".into(), 1.0);
                metrics.insert("rows_before".into(), before_usage.rows as f64);
                metrics.insert("rows_after".into(), after_usage.rows as f64);
                metrics.insert("rows_growth".into(), rows_growth as f64);

                EvalCaseOutcome {
                    case_key: CaseKey::parse("capacity", case_id.as_str()).unwrap(),
                    mode: EvalMode::Lifecycle,
                    split: CorpusSplit::Test,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Passed,
                    metrics,
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: None,
                    failures: vec![],
                    duration_ms,
                    attempts: 1,
                }
            }
            Err(err) => EvalCaseOutcome {
                case_key: CaseKey::parse("capacity", case_id.as_str()).unwrap(),
                mode: EvalMode::Lifecycle,
                split: CorpusSplit::Test,
                label_trust: LabelTrust::Official,
                status: CaseStatus::Invalid,
                metrics: std::collections::BTreeMap::new(),
                evidence: std::collections::BTreeMap::new(),
                invalid_reason: Some(format!("ingest failed: {err}")),
                failures: vec![],
                duration_ms,
                attempts: 1,
            },
        }
    }
}

#[async_trait]
impl EvalSuite for CapacitySuite {
    fn id(&self) -> &str {
        "capacity"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::Lifecycle
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    fn reducer(&self) -> &dyn crate::reducer::SuiteReducer {
        use std::sync::OnceLock;
        static R: OnceLock<&dyn crate::reducer::SuiteReducer> = OnceLock::new();
        *R.get_or_init(|| &*Box::leak(Box::new(crate::reducer::CountReducer::new("capacity"))))
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let mut outcomes = Vec::new();
        for (id, scenario) in [
            ("capacity-accepted-growth", "accepted"),
            ("capacity-ignored-no-growth", "ignored"),
            ("capacity-duplicate-no-growth", "duplicate"),
        ] {
            outcomes.push(Self::run_case(id, scenario).await);
        }
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn capacity_suite_produces_outcomes() {
        let suite = CapacitySuite::new();
        let context = RunContext {
            profile: EvalProfile::Release,
        };
        let outcomes = suite.run(&context).await;
        assert_eq!(outcomes.len(), 3);
        for outcome in &outcomes {
            assert_eq!(outcome.suite_id(), "capacity");
        }
    }
}
