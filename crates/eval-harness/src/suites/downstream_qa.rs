// PINNED-DEFERRED per ADR-0019: the downstream QA diagnostic suite is
// intentionally parked until its model, prompt, parameters, provider, and
// evaluator are pinned. Do not register this suite into PR/Release/Nightly
// profiles until that pinning happens.

use async_trait::async_trait;
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;

use crate::domain::*;
use crate::error::EvalError;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReaderContract {
    pub provider: String,
    pub model: String,
    pub model_revision: String,
    pub prompt_sha256: String,
    pub temperature: f32,
    pub top_p: f32,
    pub max_output_tokens: u32,
    pub evaluator_version: String,
}

pub struct DownstreamQaSuite {
    expected_ids: Vec<EvalCaseId>,
    reader_contract: Option<ReaderContract>,
    reducer: crate::reducer::CountReducer,
}

impl Default for DownstreamQaSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl DownstreamQaSuite {
    pub fn new() -> Self {
        Self {
            expected_ids: vec![EvalCaseId::parse("qa-diagnostic").unwrap()],
            reader_contract: None,
            reducer: crate::reducer::CountReducer::new("downstream-qa"),
        }
    }

    pub fn with_reader_contract(contract: ReaderContract) -> Self {
        Self {
            expected_ids: vec![EvalCaseId::parse("qa-diagnostic").unwrap()],
            reader_contract: Some(contract),
            reducer: crate::reducer::CountReducer::new("downstream-qa"),
        }
    }

    pub fn validate_contract(contract: &ReaderContract) -> Result<(), EvalError> {
        if contract.model_revision.is_empty() {
            return Err(EvalError::InvalidConfig(
                "reader contract requires model_revision".into(),
            ));
        }
        if contract.prompt_sha256.is_empty() {
            return Err(EvalError::InvalidConfig(
                "reader contract requires prompt_sha256".into(),
            ));
        }
        if contract.temperature < 0.0 || contract.temperature > 2.0 {
            return Err(EvalError::InvalidConfig(format!(
                "reader contract temperature must be in [0, 2], got {}",
                contract.temperature
            )));
        }
        Ok(())
    }
}

#[async_trait]
impl EvalSuite for DownstreamQaSuite {
    fn id(&self) -> &str {
        "downstream-qa"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::Performance
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    fn reducer(&self) -> &dyn crate::reducer::SuiteReducer {
        &self.reducer
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        if self.reader_contract.is_none() {
            return vec![EvalCaseOutcome {
                case_key: CaseKey::parse("downstream-qa", "qa-diagnostic").unwrap(),
                mode: EvalMode::Performance,
                split: CorpusSplit::Development,
                label_trust: LabelTrust::Weak,
                status: CaseStatus::Invalid,
                metrics: std::collections::BTreeMap::new(),
                evidence: std::collections::BTreeMap::new(),
                invalid_reason: Some("no reader contract provided".into()),
                failures: vec![],
                duration_ms: 0,
                attempts: 1,
            }];
        }

        let service = test_support::make_service().await;
        let content = "The team decided to adopt TypeScript for the new microservice.";
        let episode_id = match IngestCapability::ingest(
            &service.build_context(),
            memory_mcp::models::IngestRequest {
                source_type: "qa-test".into(),
                source_id: "qa-001".into(),
                content: content.into(),
                t_ref: chrono::Utc::now(),
                t_ingested: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        {
            Ok(id) => id,
            Err(err) => {
                return vec![EvalCaseOutcome {
                    case_key: CaseKey::parse("downstream-qa", "qa-diagnostic").unwrap(),
                    mode: EvalMode::Performance,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Weak,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: Some(format!("ingest failed: {err}")),
                    failures: vec![],
                    duration_ms: 0,
                    attempts: 1,
                }];
            }
        };

        let _ = ExtractCapability::extract(&service.build_context(), &episode_id, None, None).await;

        let start = std::time::Instant::now();
        let context_result = AssembleContextCapability::assemble_context(
            &service.build_context(),
            memory_mcp::models::AssembleContextRequest {
                query: "what language did the team choose?".into(),
                as_of: Some(chrono::Utc::now()),
                budget: 5,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
                compact: false,
            },
        )
        .await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match context_result {
            Ok(items) => {
                let has_relevant = items.iter().any(|item| item.content.contains("TypeScript"));

                let mut metrics = std::collections::BTreeMap::new();
                metrics.insert("context_items".into(), items.len() as f64);
                metrics.insert(
                    "retrieval_recall".into(),
                    if has_relevant { 1.0 } else { 0.0 },
                );
                metrics.insert("diagnostic_only".into(), 1.0);

                vec![EvalCaseOutcome {
                    case_key: CaseKey::parse("downstream-qa", "qa-diagnostic").unwrap(),
                    mode: EvalMode::Performance,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Weak,
                    status: CaseStatus::Passed,
                    metrics,
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: None,
                    failures: vec![],
                    duration_ms,
                    attempts: 1,
                }]
            }
            Err(err) => vec![EvalCaseOutcome {
                case_key: CaseKey::parse("downstream-qa", "qa-diagnostic").unwrap(),
                mode: EvalMode::Performance,
                split: CorpusSplit::Development,
                label_trust: LabelTrust::Weak,
                status: CaseStatus::Invalid,
                metrics: std::collections::BTreeMap::new(),
                evidence: std::collections::BTreeMap::new(),
                invalid_reason: Some(format!("assemble_context failed: {err}")),
                failures: vec![],
                duration_ms,
                attempts: 1,
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract_rejects_empty_model_revision() {
        let contract = ReaderContract {
            provider: "openai".into(),
            model: "gpt-4".into(),
            model_revision: "".into(),
            prompt_sha256: "abc123".into(),
            temperature: 0.0,
            top_p: 1.0,
            max_output_tokens: 1024,
            evaluator_version: "1".into(),
        };
        assert!(DownstreamQaSuite::validate_contract(&contract).is_err());
    }

    #[test]
    fn contract_rejects_empty_prompt_sha256() {
        let contract = ReaderContract {
            provider: "openai".into(),
            model: "gpt-4".into(),
            model_revision: "rev1".into(),
            prompt_sha256: "".into(),
            temperature: 0.0,
            top_p: 1.0,
            max_output_tokens: 1024,
            evaluator_version: "1".into(),
        };
        assert!(DownstreamQaSuite::validate_contract(&contract).is_err());
    }

    #[test]
    fn contract_rejects_invalid_temperature() {
        let contract = ReaderContract {
            provider: "openai".into(),
            model: "gpt-4".into(),
            model_revision: "rev1".into(),
            prompt_sha256: "abc".into(),
            temperature: 3.0,
            top_p: 1.0,
            max_output_tokens: 1024,
            evaluator_version: "1".into(),
        };
        assert!(DownstreamQaSuite::validate_contract(&contract).is_err());
    }

    #[test]
    fn contract_accepts_valid() {
        let contract = ReaderContract {
            provider: "openai".into(),
            model: "gpt-4".into(),
            model_revision: "rev1".into(),
            prompt_sha256: "abc123".into(),
            temperature: 0.7,
            top_p: 0.9,
            max_output_tokens: 1024,
            evaluator_version: "1".into(),
        };
        assert!(DownstreamQaSuite::validate_contract(&contract).is_ok());
    }

    #[tokio::test]
    async fn qa_without_contract_is_invalid() {
        let suite = DownstreamQaSuite::new();
        let context = RunContext {
            profile: EvalProfile::Nightly,
        };
        let outcomes = suite.run(&context).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, CaseStatus::Invalid);
    }
}
