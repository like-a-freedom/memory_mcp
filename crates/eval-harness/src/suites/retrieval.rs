use std::collections::{BTreeMap, BTreeSet};

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;

use super::retrieval_cases::{RetrievalEvalCase, case_as_of, fixture_path, load_cases};
use crate::domain::*;
use crate::error::EvalError;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;
use memory_mcp::models::EdgeAttributes;

/// The pre-A1 local fixture remains a weak-label diagnostic. These metric IDs
/// are explicitly versioned so the historical string-scored series cannot be
/// confused with the native retrieval-quality suite.
pub(crate) struct LocalRetrievalDiagnosticReducer {
    suite_id: SuiteId,
}

impl LocalRetrievalDiagnosticReducer {
    pub(crate) fn new() -> Self {
        Self {
            suite_id: SuiteId::parse("local-retrieval")
                .expect("local retrieval suite id is a valid constant"),
        }
    }
}

impl crate::reducer::SuiteReducer for LocalRetrievalDiagnosticReducer {
    fn suite_id(&self) -> &SuiteId {
        &self.suite_id
    }

    fn reduce(
        &self,
        outcomes: &[EvalCaseOutcome],
    ) -> Result<Vec<crate::artifact::SuiteSummary>, EvalError> {
        let mut total_groups = 0_u64;
        let mut covered_groups = 0_u64;
        let mut reciprocal_rank_sum = 0.0;
        let mut top_one_hits = 0_u64;
        let mut answerable_cases = 0_u64;
        let mut passed = 0_usize;
        let mut quality_failed = 0_usize;
        let mut invalid = 0_usize;

        for outcome in outcomes {
            match outcome.status {
                CaseStatus::Passed => passed += 1,
                CaseStatus::QualityFailed => quality_failed += 1,
                CaseStatus::Invalid => invalid += 1,
            }
            let Some(MetricEvidence::Retrieval {
                relevant,
                hits_at_k,
                first_relevant_rank,
                ..
            }) = outcome.evidence.get("retrieval")
            else {
                continue;
            };
            total_groups += relevant;
            covered_groups += hits_at_k;
            if *relevant > 0 {
                answerable_cases += 1;
                if let Some(rank) = first_relevant_rank.filter(|rank| *rank > 0) {
                    reciprocal_rank_sum += 1.0 / f64::from(rank);
                }
                if *first_relevant_rank == Some(1) {
                    top_one_hits += 1;
                }
            }
        }

        let mut metrics = BTreeMap::new();
        if total_groups > 0 {
            metrics.insert(
                "legacy_v1_required_group_recall_at_budget".into(),
                covered_groups as f64 / total_groups as f64,
            );
            metrics.insert(
                "legacy_v1_recall_applicable_groups".into(),
                total_groups as f64,
            );
        }
        if answerable_cases > 0 {
            metrics.insert(
                "legacy_v1_mrr_at_budget".into(),
                reciprocal_rank_sum / answerable_cases as f64,
            );
            metrics.insert(
                "legacy_v1_top_1_hit_rate_at_budget".into(),
                top_one_hits as f64 / answerable_cases as f64,
            );
            metrics.insert(
                "legacy_v1_rank_applicable_cases".into(),
                answerable_cases as f64,
            );
        }

        Ok(vec![crate::artifact::SuiteSummary {
            suite_id: self.suite_id.as_str().to_string(),
            mode: outcomes
                .first()
                .map_or(EvalMode::RetrievalOnly, |outcome| outcome.mode),
            total: outcomes.len(),
            passed,
            quality_failed,
            invalid,
            metrics,
        }])
    }
}

fn resolve_gold_groups(
    selectors: &[String],
    seeded_facts: &BTreeMap<String, String>,
) -> Result<Vec<BTreeSet<String>>, &'static str> {
    selectors
        .iter()
        .map(|selector| {
            let ids = seeded_facts
                .iter()
                .filter(|(_, content)| content.contains(selector))
                .map(|(fact_id, _)| fact_id.clone())
                .collect::<BTreeSet<_>>();
            if ids.is_empty() {
                return Err("legacy gold selector did not resolve to a seeded fact ID");
            }
            Ok(ids)
        })
        .collect()
}

fn id_retrieval_evidence(
    required_groups: &[BTreeSet<String>],
    returned_ids: &[String],
    cutoff: u32,
) -> Result<MetricEvidence, &'static str> {
    if required_groups.is_empty() {
        return Err("legacy retrieval case has no required evidence groups");
    }
    if cutoff == 0 {
        return Err("legacy retrieval budget must be positive");
    }

    let mut unique_ids = Vec::with_capacity(returned_ids.len());
    let mut seen = BTreeSet::new();
    for fact_id in returned_ids {
        if seen.insert(fact_id) {
            unique_ids.push(fact_id.clone());
        }
    }
    let within_budget = unique_ids
        .iter()
        .take(cutoff as usize)
        .collect::<BTreeSet<_>>();
    let covered_groups = required_groups
        .iter()
        .filter(|group| group.iter().any(|fact_id| within_budget.contains(fact_id)))
        .count();
    let first_relevant_rank = unique_ids
        .iter()
        .take(cutoff as usize)
        .position(|fact_id| required_groups.iter().any(|group| group.contains(fact_id)))
        .and_then(|rank| u32::try_from(rank + 1).ok());

    Ok(MetricEvidence::retrieval(
        u64::try_from(required_groups.len()).unwrap_or(u64::MAX),
        u64::try_from(covered_groups).unwrap_or(u64::MAX),
        first_relevant_rank,
        cutoff,
    ))
}

fn invalid_case_outcome(
    case_key: CaseKey,
    reason: &str,
    elapsed: std::time::Duration,
) -> EvalCaseOutcome {
    EvalCaseOutcome {
        case_key,
        mode: EvalMode::RetrievalOnly,
        split: CorpusSplit::Development,
        label_trust: LabelTrust::Weak,
        status: CaseStatus::Invalid,
        metrics: BTreeMap::new(),
        evidence: BTreeMap::new(),
        invalid_reason: Some(reason.into()),
        failures: vec![],
        duration_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
        attempts: 1,
    }
}

pub struct LocalRetrievalSuite {
    expected_ids: Vec<EvalCaseId>,
    reducer: LocalRetrievalDiagnosticReducer,
}

impl LocalRetrievalSuite {
    pub fn new() -> Result<Self, EvalError> {
        let cases = load_cases()?;
        let expected_ids = cases
            .iter()
            .map(|c| EvalCaseId::parse(&c.id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            expected_ids,
            reducer: LocalRetrievalDiagnosticReducer::new(),
        })
    }

    async fn run_case(case: &RetrievalEvalCase) -> EvalCaseOutcome {
        let case_id = EvalCaseId::parse(&case.id).unwrap();
        let case_key = CaseKey::parse("local-retrieval", case_id.as_str()).unwrap();
        let start = std::time::Instant::now();

        let (service, db_client) = test_support::make_service_with_client().await;

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
        for edge in &case.edges {
            if let Err(err) = service
                .relate(
                    &edge.from_id,
                    &edge.relation,
                    &edge.to_id,
                    EdgeAttributes::inferred(),
                )
                .await
            {
                return EvalCaseOutcome {
                    case_key: case_key.clone(),
                    mode: EvalMode::RetrievalOnly,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Weak,
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
                        case_key: case_key.clone(),
                        mode: EvalMode::RetrievalOnly,
                        split: CorpusSplit::Development,
                        label_trust: LabelTrust::Weak,
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
                &community.community_id,
                &community.member_entities,
                &community.summary,
                updated_at,
            )
            .await;
        }
        let mut seeded_facts = BTreeMap::new();
        for fact in &case.facts {
            let t_valid = match fact.t_valid.parse::<DateTime<Utc>>() {
                Ok(t) => t,
                Err(err) => {
                    return EvalCaseOutcome {
                        case_key: case_key.clone(),
                        mode: EvalMode::RetrievalOnly,
                        split: CorpusSplit::Development,
                        label_trust: LabelTrust::Weak,
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
            let fact_id = test_support::seed_fact_with_links(
                &service,
                &fact.content,
                t_valid,
                fact.entity_links.clone(),
                fact.source_id.as_deref(),
            )
            .await;
            seeded_facts.insert(fact_id, fact.content.clone());
        }

        let required_groups = match resolve_gold_groups(&case.expected.must_contain, &seeded_facts)
        {
            Ok(groups) if !groups.is_empty() => groups,
            Ok(_) => {
                return invalid_case_outcome(
                    case_key,
                    "legacy retrieval case has no required gold evidence",
                    start.elapsed(),
                );
            }
            Err(reason) => return invalid_case_outcome(case_key, reason, start.elapsed()),
        };
        let prohibited_groups =
            match resolve_gold_groups(&case.expected.must_not_contain, &seeded_facts) {
                Ok(groups) => groups,
                Err(reason) => return invalid_case_outcome(case_key, reason, start.elapsed()),
            };
        let prohibited_ids = prohibited_groups
            .into_iter()
            .flatten()
            .collect::<BTreeSet<_>>();

        let as_of = case_as_of(case);
        let items = match AssembleContextCapability::assemble_context_from_service(
            &service,
            memory_mcp::models::AssembleContextRequest {
                query: case.query.clone(),
                as_of: Some(as_of),
                budget: case.budget,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
                compact: false,
            },
        )
        .await
        {
            Ok(items) => items,
            Err(err) => {
                return EvalCaseOutcome {
                    case_key: case_key.clone(),
                    mode: EvalMode::RetrievalOnly,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Weak,
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

        let returned_ids = items
            .iter()
            .map(|item| item.fact_id.clone())
            .collect::<Vec<_>>();
        let cutoff = match u32::try_from(case.budget) {
            Ok(cutoff) if cutoff > 0 => cutoff,
            _ => {
                return invalid_case_outcome(
                    case_key,
                    "legacy retrieval budget is outside the supported range",
                    start.elapsed(),
                );
            }
        };
        let evidence = match id_retrieval_evidence(&required_groups, &returned_ids, cutoff) {
            Ok(evidence) => evidence,
            Err(reason) => return invalid_case_outcome(case_key, reason, start.elapsed()),
        };
        let metric_names = crate::metrics::CaseMetricNames {
            recall_at_k: Some("legacy_v1_required_group_recall_at_budget".into()),
            mrr: Some("legacy_v1_mrr_at_budget".into()),
            top_1_hit_rate: Some("legacy_v1_top_1_hit_rate_at_budget".into()),
            ..crate::metrics::CaseMetricNames::default()
        };
        let metric_map = crate::metrics::render_case_metrics(&evidence, &metric_names);
        let recall_at_budget = metric_map
            .get("legacy_v1_required_group_recall_at_budget")
            .copied()
            .unwrap_or(0.0);
        let unique_returned_ids = returned_ids.iter().cloned().collect::<BTreeSet<_>>();
        let duplicate_free = unique_returned_ids.len() == returned_ids.len();
        let prohibited_returned_ids = unique_returned_ids
            .intersection(&prohibited_ids)
            .cloned()
            .collect::<Vec<_>>();
        let within_budget = items.len() <= cutoff as usize;

        let mut evidence_map = std::collections::BTreeMap::new();
        evidence_map.insert("retrieval".to_string(), evidence);

        let meets_recall = recall_at_budget >= case.expected.min_recall_at_k;
        let no_prohibited = prohibited_returned_ids.is_empty();
        let status = if meets_recall && no_prohibited && within_budget && duplicate_free {
            CaseStatus::Passed
        } else {
            CaseStatus::QualityFailed
        };

        let mut failures = Vec::new();
        if !meets_recall {
            failures.push(format!(
                "legacy required-group recall at budget={recall_at_budget:.4} < {}",
                case.expected.min_recall_at_k
            ));
        }
        if !no_prohibited {
            failures.push(format!(
                "explicitly prohibited fact IDs returned: {prohibited_returned_ids:?}"
            ));
        }
        if !within_budget {
            failures.push(format!(
                "retrieval returned {} items for budget {}",
                items.len(),
                case.budget
            ));
        }
        if !duplicate_free {
            failures.push("retrieval returned duplicate fact IDs".into());
        }

        EvalCaseOutcome {
            case_key,
            mode: EvalMode::RetrievalOnly,
            split: CorpusSplit::Development,
            label_trust: LabelTrust::Weak,
            status,
            metrics: metric_map,
            evidence: evidence_map,
            invalid_reason: None,
            failures,
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
        &self.reducer
    }

    fn fingerprint_metadata(&self) -> BTreeMap<String, String> {
        use sha2::Digest;

        let mut metadata = BTreeMap::from([
            (
                "metric_version".into(),
                "local-retrieval-v1-legacy-diagnostic".into(),
            ),
            ("label_trust".into(), "weak".into()),
        ]);
        if let Ok(bytes) = std::fs::read(fixture_path()) {
            metadata.insert(
                "fixture_sha256".into(),
                hex::encode(sha2::Sha256::digest(bytes)),
            );
        }
        metadata
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let cases = match load_cases() {
            Ok(cases) => cases,
            Err(err) => {
                return vec![EvalCaseOutcome {
                    case_key: CaseKey::parse("local-retrieval", "fixture-load-error").unwrap(),
                    mode: EvalMode::RetrievalOnly,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Weak,
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
    use crate::reducer::SuiteReducer;

    fn reducer_outcome(
        id: &str,
        relevant_groups: u64,
        covered_groups: u64,
        first_rank: Option<u32>,
    ) -> EvalCaseOutcome {
        let mut outcome = EvalCaseOutcome::new(
            "local-retrieval",
            id,
            EvalMode::RetrievalOnly,
            CorpusSplit::Development,
            LabelTrust::Weak,
            CaseStatus::Passed,
        );
        outcome.evidence.insert(
            "retrieval".into(),
            MetricEvidence::retrieval(relevant_groups, covered_groups, first_rank, 5),
        );
        outcome
    }

    #[test]
    fn local_reducer_uses_weighted_group_recall_and_legacy_metric_ids() {
        let outcomes = vec![
            reducer_outcome("one-group", 1, 1, Some(1)),
            reducer_outcome("three-groups", 3, 1, Some(2)),
        ];

        let summary = LocalRetrievalDiagnosticReducer::new()
            .reduce(&outcomes)
            .expect("legacy diagnostic outcomes reduce");

        assert_eq!(
            summary[0]
                .metrics
                .get("legacy_v1_required_group_recall_at_budget"),
            Some(&0.5)
        );
        assert_eq!(
            summary[0].metrics.get("legacy_v1_mrr_at_budget"),
            Some(&0.75)
        );
        assert!(!summary[0].metrics.contains_key("recall_at_5"));
        assert!(!summary[0].metrics.contains_key("mrr"));
    }

    #[test]
    fn local_reducer_omits_metrics_when_no_case_has_applicable_evidence() {
        let mut invalid = EvalCaseOutcome::new(
            "local-retrieval",
            "invalid",
            EvalMode::RetrievalOnly,
            CorpusSplit::Development,
            LabelTrust::Weak,
            CaseStatus::Invalid,
        );
        invalid.invalid_reason = Some("native setup unavailable".into());

        let summary = LocalRetrievalDiagnosticReducer::new()
            .reduce(&[invalid])
            .expect("invalid diagnostic run remains reportable");

        assert!(summary[0].metrics.is_empty());
    }

    #[test]
    fn gold_selectors_resolve_to_seeded_fact_id_groups_before_retrieval() {
        let seeded_facts = std::collections::BTreeMap::from([
            ("fact:tea".to_string(), "The user prefers tea.".to_string()),
            (
                "fact:coffee".to_string(),
                "The user prefers coffee.".to_string(),
            ),
        ]);

        let groups = resolve_gold_groups(&["tea".into(), "prefers".into()], &seeded_facts)
            .expect("selectors must resolve against the seeded fact registry");

        assert_eq!(
            groups[0],
            std::collections::BTreeSet::from(["fact:tea".into()])
        );
        assert_eq!(
            groups[1],
            std::collections::BTreeSet::from(["fact:coffee".into(), "fact:tea".into()])
        );
    }

    #[test]
    fn id_retrieval_evidence_deduplicates_before_counting_covered_groups() {
        let required_groups = vec![
            std::collections::BTreeSet::from(["fact:tea".to_string()]),
            std::collections::BTreeSet::from(["fact:coffee".to_string()]),
        ];
        let returned_ids = vec!["fact:tea".to_string(), "fact:tea".to_string()];

        let evidence = id_retrieval_evidence(&required_groups, &returned_ids, 5)
            .expect("ID evidence must be valid");

        assert_eq!(
            evidence,
            MetricEvidence::retrieval(2, 1, Some(1), 5),
            "duplicate IDs must not inflate covered evidence groups"
        );
    }

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
