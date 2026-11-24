//! Finite-cohort deterministic acceptance. No statistical or agent-benefit claim.
use super::{ReplayIndex, compare_replays};
use crate::EvalError;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeterministicAcceptancePlan {
    pub schema_version: String,
    pub corpus_sha256: String,
    pub config_sha256: String,
    #[serde(default)]
    pub candidate_config_sha256: Option<String>,
    #[serde(default)]
    pub product_binary_sha256: Option<String>,
    #[serde(default)]
    pub score_channel: AcceptanceScoreChannel,
    pub families: BTreeMap<String, String>,
    pub ordinary_cases: BTreeSet<String>,
    pub minimum_gain_basis_points: u32,
    pub maximum_items: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceScoreChannel {
    #[default]
    EvidenceEnabledTemporal,
    Primary,
}

fn acceptance_observation(
    case: &super::receipts::ReplayCase,
    channel: AcceptanceScoreChannel,
) -> Result<(super::scoring::ProbeScore, super::scoring::Observed), EvalError> {
    use super::scoring::{Observed, ProbeScore, ScoreReason};
    let invalid = |message: &str| EvalError::InvalidInput(message.into());
    match channel {
        AcceptanceScoreChannel::EvidenceEnabledTemporal => {
            let score: ProbeScore = serde_json::from_value(
                case.evidence_enabled_temporal_score
                    .clone()
                    .ok_or_else(|| invalid("missing independent temporal decision score"))?,
            )?;
            let observation: Observed = serde_json::from_value(
                case.evidence_enabled_temporal_observation
                    .clone()
                    .ok_or_else(|| invalid("missing independent temporal decision observation"))?,
            )?;
            Ok((score, observation))
        }
        AcceptanceScoreChannel::Primary => {
            if !matches!(
                case.score_observation_source.as_deref(),
                Some("primary" | "temporal_revision")
            ) {
                return Err(invalid(
                    "primary acceptance requires an explicitly registered score source",
                ));
            }
            let observation: Observed = serde_json::from_value(
                case.score_observation
                    .clone()
                    .ok_or_else(|| invalid("missing complete primary observation"))?,
            )?;
            let evidence_success = case
                .evidence_success
                .ok_or_else(|| invalid("missing primary evidence score"))?;
            let decision_success = case
                .decision_success
                .ok_or_else(|| invalid("missing primary decision score"))?;
            let probe_success = case
                .probe_success
                .ok_or_else(|| invalid("missing primary probe score"))?;
            if probe_success != (evidence_success && decision_success) {
                return Err(invalid("inconsistent primary decision score"));
            }
            let source_order = observation
                .sources
                .iter()
                .map(|source| {
                    source
                        .event_id
                        .clone()
                        .unwrap_or_else(|| "<unbound>".into())
                })
                .collect::<Vec<_>>();
            let roles = observation
                .sources
                .iter()
                .map(|source| source.role.as_str().to_string())
                .collect::<Vec<_>>();
            if source_order != case.source_order
                || roles != case.roles
                || case.disposition != format!("{:?}", observation.disposition).to_lowercase()
            {
                return Err(invalid(
                    "primary observation differs from its normalized replay fields",
                ));
            }
            let reason_codes = case
                .reason_codes
                .iter()
                .map(|reason| serde_json::from_value(serde_json::json!(reason)))
                .collect::<Result<Vec<ScoreReason>, _>>()?;
            Ok((
                ProbeScore {
                    evidence_success,
                    decision_success,
                    probe_success,
                    reason_codes,
                },
                observation,
            ))
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeterministicAcceptanceReport {
    pub schema_version: String,
    pub plan_sha256: String,
    pub incumbent_sha256: String,
    pub candidate_sha256: String,
    pub cases: usize,
    pub families: usize,
    pub wins: Vec<String>,
    pub losses: Vec<String>,
    pub incumbent_family_success: f64,
    pub candidate_family_success: f64,
    pub hard_failures: Vec<String>,
    pub baseline_safety_findings: Vec<String>,
    /// Descriptive family resampling on this synthetic cohort; not a population guarantee.
    pub descriptive_family_gain_interval: crate::paired::ConfidenceInterval,
    pub accepted: bool,
}

fn safety_findings(
    case_id: &str,
    score: &super::scoring::ProbeScore,
    observation: &super::scoring::Observed,
    maximum_items: u64,
) -> Vec<String> {
    use super::scoring::{ObservedDisposition, ScoreReason};
    let mut findings = Vec::new();
    if observation.item_count as u64 > maximum_items
        || observation.budget_used as u64 > maximum_items
    {
        findings.push(format!("{case_id}:resource_ceiling"));
    }
    if score.reason_codes.contains(&ScoreReason::IncorrectRole)
        && observation
            .sources
            .iter()
            .any(|source| source.role == super::scoring::EvidenceRole::Current)
    {
        findings.push(format!("{case_id}:unsafe_role_promotion"));
    }
    if matches!(
        observation.disposition,
        ObservedDisposition::Supported | ObservedDisposition::Correction
    ) && score.reason_codes.iter().any(|reason| {
        matches!(
            reason,
            ScoreReason::IncorrectDisposition | ScoreReason::WrongAssertion
        )
    }) {
        findings.push(format!("{case_id}:unsupported_positive_decision"));
    }
    for reason in &score.reason_codes {
        if matches!(
            reason,
            ScoreReason::ForbiddenSource
                | ScoreReason::UnboundSource
                | ScoreReason::BudgetExceeded
                | ScoreReason::UnsupportedTemporal
                | ScoreReason::Nonrepeatable
        ) {
            findings.push(format!("{case_id}:{}", reason.as_str()));
        }
    }
    findings.sort();
    findings.dedup();
    findings
}
pub fn accept_candidate(
    plan: &DeterministicAcceptancePlan,
    incumbent: [&ReplayIndex; 2],
    candidate: [&ReplayIndex; 2],
) -> Result<DeterministicAcceptanceReport, EvalError> {
    let invalid = |s: &str| EvalError::InvalidInput(s.into());
    if plan.schema_version != "memory-mcp-deterministic-acceptance/v1"
        || plan.minimum_gain_basis_points == 0
        || plan.minimum_gain_basis_points > 10_000
        || plan.families.values().collect::<BTreeSet<_>>().len() < 2
        || plan.ordinary_cases.is_empty()
        || plan.families.values().any(|f| f.trim().is_empty())
        || !plan
            .ordinary_cases
            .iter()
            .all(|id| plan.families.contains_key(id))
    {
        return Err(invalid("invalid frozen deterministic acceptance plan"));
    }
    compare_replays(incumbent[0], incumbent[1])?;
    compare_replays(candidate[0], candidate[1])?;
    for repeats in [incumbent, candidate] {
        if repeats[0]
            .cases
            .iter()
            .zip(&repeats[1].cases)
            .any(|(a, b)| a.receipt_sha256 == b.receipt_sha256)
        {
            return Err(invalid(
                "two distinct raw captures are required; repeated artifact supplied",
            ));
        }
    }
    let left = incumbent[0];
    let right = candidate[0];
    if plan.score_channel == AcceptanceScoreChannel::Primary
        && (left.identity.product_revision != right.identity.product_revision
            || left.identity.product_binary_sha256 != right.identity.product_binary_sha256)
    {
        return Err(invalid(
            "primary rollout comparison requires the same product build on both stages",
        ));
    }
    if plan
        .product_binary_sha256
        .as_deref()
        .is_some_and(|expected| {
            left.identity.product_binary_sha256 != expected
                || right.identity.product_binary_sha256 != expected
        })
    {
        return Err(invalid(
            "replay product binary differs from frozen acceptance plan",
        ));
    }
    for (replay, expected_config_sha256) in [
        (left, plan.config_sha256.as_str()),
        (
            right,
            plan.candidate_config_sha256
                .as_deref()
                .unwrap_or(plan.config_sha256.as_str()),
        ),
    ] {
        if replay.identity.corpus_sha256 != plan.corpus_sha256
            || replay.identity.config_sha256 != expected_config_sha256
            || replay.identity.input_schema_sha256.is_none()
            || replay.identity.suite_id != "memory-decisions"
        {
            return Err(invalid(
                "replay corpus, configuration or public schema is incompatible",
            ));
        }
        let keys = replay
            .cases
            .iter()
            .map(|c| c.case_id.clone())
            .collect::<BTreeSet<_>>();
        if keys != plan.families.keys().cloned().collect() {
            return Err(invalid("incomplete deterministic case coverage"));
        }
    }
    if left.identity.input_schema_sha256 != right.identity.input_schema_sha256 {
        return Err(invalid(
            "public schema differs; requires separately reviewed compatibility protocol",
        ));
    }
    let mut hard = vec![];
    let mut baseline_safety_findings = vec![];
    let mut wins = vec![];
    let mut losses = vec![];
    let mut families: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for old in &left.cases {
        let new = right
            .cases
            .iter()
            .find(|c| c.case_id == old.case_id)
            .ok_or_else(|| invalid("missing paired case"))?;
        let (a, _) = acceptance_observation(old, plan.score_channel)?;
        let (b, obs) = acceptance_observation(new, plan.score_channel)?;
        if a.evidence_success && !b.evidence_success {
            hard.push(format!("{}:evidence_regression", new.case_id));
        }
        if plan.ordinary_cases.contains(&new.case_id) && a.probe_success && !b.probe_success {
            hard.push(format!("{}:ordinary_regression", new.case_id));
        }
        let incumbent_safety = safety_findings(
            &old.case_id,
            &a,
            &acceptance_observation(old, plan.score_channel)?.1,
            plan.maximum_items,
        );
        let candidate_safety = safety_findings(&new.case_id, &b, &obs, plan.maximum_items);
        baseline_safety_findings.extend(incumbent_safety.iter().cloned());
        if plan.score_channel == AcceptanceScoreChannel::Primary {
            hard.extend(
                candidate_safety
                    .into_iter()
                    .filter(|finding| !incumbent_safety.contains(finding)),
            );
        } else {
            hard.extend(candidate_safety);
        }
        if !a.probe_success && b.probe_success {
            wins.push(new.case_id.clone());
        }
        if a.probe_success && !b.probe_success {
            losses.push(new.case_id.clone());
        }
        let family = plan
            .families
            .get(&new.case_id)
            .ok_or_else(|| invalid("missing family"))?;
        let entry = families.entry(family).or_default();
        entry.0 += usize::from(a.probe_success);
        entry.1 += usize::from(b.probe_success);
        entry.2 += 1;
    }
    let n = families.len() as f64;
    let a = families
        .values()
        .map(|(a, _, total)| *a as f64 / *total as f64)
        .sum::<f64>()
        / n;
    let b = families
        .values()
        .map(|(_, b, total)| *b as f64 / *total as f64)
        .sum::<f64>()
        / n;
    hard.sort();
    hard.dedup();
    baseline_safety_findings.sort();
    baseline_safety_findings.dedup();
    wins.sort();
    losses.sort();
    let deltas = families
        .iter()
        .map(|(id, (a, b, total))| ((*id).to_owned(), (*b as f64 - *a as f64) / *total as f64))
        .collect();
    let interval = crate::paired::cluster_bootstrap(
        &deltas,
        &crate::paired::BootstrapConfig {
            seed: 20261121,
            draws: 10_000,
            confidence_level_basis_points: 9500,
        },
    )?;
    Ok(DeterministicAcceptanceReport {
        schema_version: "memory-mcp-deterministic-acceptance-report/v2".into(),
        plan_sha256: super::receipts::sha256(&serde_json::to_vec(plan)?),
        incumbent_sha256: left.identity.product_binary_sha256.clone(),
        candidate_sha256: right.identity.product_binary_sha256.clone(),
        cases: left.cases.len(),
        families: families.len(),
        wins,
        losses,
        incumbent_family_success: a,
        candidate_family_success: b,
        accepted: hard.is_empty()
            && (b - a) * 10_000.0 + 1e-9 >= f64::from(plan.minimum_gain_basis_points),
        hard_failures: hard,
        baseline_safety_findings,
        descriptive_family_gain_interval: interval,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(success: bool, product: &str) -> ReplayIndex {
        let mut replay: ReplayIndex = serde_json::from_value(serde_json::json!({
      "schema_version":"memory-decisions-replay/v1",
      "identity":{"suite_id":"memory-decisions","product_revision":product,"product_binary_sha256":product.repeat(64),"corpus_sha256":"c".repeat(64),"config_sha256":"d".repeat(64),"input_schema_sha256":"e".repeat(64)},
      "cases":[{"case_id":"case","source_order":[],"roles":[],"disposition":"unknown","evidence_success":true,"decision_success":success,"probe_success":success,"temporal_observation":null,"temporal_score":null,
        "evidence_enabled_temporal_observation":{"sources":[],"disposition":"unknown","item_count":0,"budget_used":0,"relation_outcomes":[],"guidance":null},
        "evidence_enabled_temporal_score":{"evidence_success":true,"decision_success":success,"probe_success":success,"reason_codes":[]},"reason_codes":[],"receipt_path":"private-receipt","receipt_sha256":"f".repeat(64)}]
    })).unwrap();
        let mut control = replay.cases[0].clone();
        control.case_id = "control".into();
        control.evidence_enabled_temporal_score = Some(
            serde_json::json!({"evidence_success":true,"decision_success":true,"probe_success":true,"reason_codes":[]}),
        );
        replay.cases.push(control);
        replay
    }
    fn repeated(replay: &ReplayIndex) -> ReplayIndex {
        let mut copy = replay.clone();
        for case in &mut copy.cases {
            case.receipt_sha256 = "9".repeat(64);
            case.receipt_path = format!("second/{}", case.receipt_path);
        }
        copy
    }
    fn plan() -> DeterministicAcceptancePlan {
        DeterministicAcceptancePlan {
            schema_version: "memory-mcp-deterministic-acceptance/v1".into(),
            corpus_sha256: "c".repeat(64),
            config_sha256: "d".repeat(64),
            candidate_config_sha256: None,
            product_binary_sha256: None,
            score_channel: AcceptanceScoreChannel::default(),
            families: BTreeMap::from([
                ("case".into(), "family".into()),
                ("control".into(), "control-family".into()),
            ]),
            ordinary_cases: BTreeSet::from(["control".into()]),
            minimum_gain_basis_points: 1000,
            maximum_items: 5,
        }
    }
    #[test]
    fn frozen_positive_gain_accepts_repeatable_candidate() {
        let a = fixture(false, "a");
        let b = fixture(true, "b");
        let report = accept_candidate(&plan(), [&a, &repeated(&a)], [&b, &repeated(&b)])
            .expect("complete repeatable observations");
        assert!(report.accepted);
        assert_eq!(report.wins, vec!["case"]);
    }
    #[test]
    fn explicitly_pinned_candidate_configuration_can_differ_from_incumbent() {
        let a = fixture(false, "a");
        let mut b = fixture(true, "b");
        b.identity.config_sha256 = "g".repeat(64);
        let mut plan = plan();
        plan.candidate_config_sha256 = Some("g".repeat(64));

        let report = accept_candidate(&plan, [&a, &repeated(&a)], [&b, &repeated(&b)])
            .expect("explicitly pinned transition between registered profiles");

        assert!(report.accepted);
    }
    #[test]
    fn primary_channel_accepts_only_the_observed_public_stage_gain() {
        let mut a = fixture(false, "a");
        let mut b = fixture(true, "a");
        a.identity.product_binary_sha256 = "same-binary".into();
        b.identity.product_binary_sha256 = "same-binary".into();
        b.identity.config_sha256 = "g".repeat(64);
        for replay in [&mut a, &mut b] {
            let control = replay
                .cases
                .iter_mut()
                .find(|case| case.case_id == "control")
                .unwrap();
            control.evidence_success = Some(true);
            control.decision_success = Some(true);
            control.probe_success = Some(true);
        }
        for case in a.cases.iter_mut().chain(b.cases.iter_mut()) {
            case.score_observation_source = Some("primary".into());
            case.score_observation = Some(serde_json::json!({
                "sources": [],
                "disposition": "unknown",
                "item_count": 0,
                "budget_used": 0,
                "relation_outcomes": [],
                "guidance": null
            }));
            case.evidence_enabled_temporal_score = Some(
                serde_json::json!({"evidence_success":true,"decision_success":false,"probe_success":false,"reason_codes":[]}),
            );
        }
        let mut plan = plan();
        plan.candidate_config_sha256 = Some("g".repeat(64));
        plan.product_binary_sha256 = Some("same-binary".into());
        plan.score_channel = AcceptanceScoreChannel::Primary;

        let report = accept_candidate(&plan, [&a, &repeated(&a)], [&b, &repeated(&b)])
            .expect("same-product primary rollout observations");

        assert!(report.accepted);
        assert_eq!(report.wins, vec!["case"]);
    }
    #[test]
    fn primary_channel_reports_shared_safety_failure_without_calling_it_a_regression() {
        let mut a = fixture(false, "same");
        let mut b = fixture(false, "same");
        a.identity.product_binary_sha256 = "same-binary".into();
        b.identity.product_binary_sha256 = "same-binary".into();
        b.identity.config_sha256 = "g".repeat(64);
        for replay in [&mut a, &mut b] {
            for case in &mut replay.cases {
                case.score_observation_source = Some("primary".into());
                case.score_observation = Some(serde_json::json!({
                    "sources": [],
                    "disposition": "unknown",
                    "item_count": 0,
                    "budget_used": 0,
                    "relation_outcomes": [],
                    "guidance": null
                }));
            }
            let shared = &mut replay.cases[0];
            shared.evidence_success = Some(false);
            shared.decision_success = Some(false);
            shared.probe_success = Some(false);
            shared.source_order = vec!["target".into(), "distractor".into()];
            shared.roles = vec!["unknown".into(), "unknown".into()];
            shared.reason_codes = vec!["forbidden_source".into()];
            shared.score_observation = Some(serde_json::json!({
                "sources": [
                    {"event_id":"target","text":"","role":"unknown"},
                    {"event_id":"distractor","text":"","role":"unknown"}
                ],
                "disposition": "unknown",
                "item_count": 2,
                "budget_used": 2,
                "relation_outcomes": [],
                "guidance": null
            }));
            let control = &mut replay.cases[1];
            control.evidence_success = Some(true);
            control.decision_success = Some(true);
            control.probe_success = Some(true);
        }
        let mut old_gain = a.cases[0].clone();
        old_gain.case_id = "gain".into();
        old_gain.evidence_success = Some(true);
        old_gain.decision_success = Some(false);
        old_gain.probe_success = Some(false);
        old_gain.source_order = vec!["source".into()];
        old_gain.roles = vec!["current".into()];
        old_gain.reason_codes.clear();
        old_gain.score_observation = Some(serde_json::json!({
            "sources": [{"event_id":"source","text":"","role":"current"}],
            "disposition": "unknown",
            "item_count": 1,
            "budget_used": 1,
            "relation_outcomes": [],
            "guidance": null
        }));
        let mut new_gain = old_gain.clone();
        new_gain.decision_success = Some(true);
        new_gain.probe_success = Some(true);
        a.cases.push(old_gain);
        b.cases.push(new_gain);
        let mut plan = plan();
        plan.families.insert("gain".into(), "gain-family".into());
        plan.candidate_config_sha256 = Some("g".repeat(64));
        plan.score_channel = AcceptanceScoreChannel::Primary;

        let report = accept_candidate(&plan, [&a, &repeated(&a)], [&b, &repeated(&b)])
            .expect("paired stage comparison with a shared pre-existing failure");

        assert!(report.accepted);
        assert!(report.hard_failures.is_empty());
        assert_eq!(
            report.baseline_safety_findings,
            vec!["case:forbidden_source"]
        );
    }
    #[test]
    fn unpinned_candidate_configuration_still_fails() {
        let a = fixture(false, "a");
        let mut b = fixture(true, "b");
        b.identity.config_sha256 = "g".repeat(64);

        assert!(accept_candidate(&plan(), [&a, &repeated(&a)], [&b, &repeated(&b)]).is_err());
    }
    #[test]
    fn primary_channel_rejects_binary_outside_frozen_plan() {
        let mut a = fixture(false, "same");
        let mut b = fixture(true, "same");
        a.identity.product_binary_sha256 = "same-binary".into();
        b.identity.product_binary_sha256 = "same-binary".into();
        b.identity.config_sha256 = "g".repeat(64);
        for case in a.cases.iter_mut().chain(b.cases.iter_mut()) {
            case.score_observation_source = Some("primary".into());
            case.score_observation = Some(serde_json::json!({
                "sources": [],
                "disposition": "unknown",
                "item_count": 0,
                "budget_used": 0,
                "relation_outcomes": [],
                "guidance": null
            }));
        }
        let mut plan = plan();
        plan.candidate_config_sha256 = Some("g".repeat(64));
        plan.product_binary_sha256 = Some("different-binary".into());
        plan.score_channel = AcceptanceScoreChannel::Primary;

        assert!(accept_candidate(&plan, [&a, &repeated(&a)], [&b, &repeated(&b)]).is_err());
    }
    #[test]
    fn primary_channel_rejects_missing_complete_observation() {
        let mut a = fixture(false, "same");
        let mut b = fixture(true, "same");
        a.identity.product_binary_sha256 = "same-binary".into();
        b.identity.product_binary_sha256 = "same-binary".into();
        b.identity.config_sha256 = "g".repeat(64);
        for case in a.cases.iter_mut().chain(b.cases.iter_mut()) {
            case.score_observation_source = Some("primary".into());
            case.score_observation = Some(serde_json::json!({
                "sources": [],
                "disposition": "unknown",
                "item_count": 0,
                "budget_used": 0,
                "relation_outcomes": [],
                "guidance": null
            }));
        }
        b.cases[0].score_observation = None;
        let mut plan = plan();
        plan.candidate_config_sha256 = Some("g".repeat(64));
        plan.score_channel = AcceptanceScoreChannel::Primary;

        assert!(accept_candidate(&plan, [&a, &repeated(&a)], [&b, &repeated(&b)]).is_err());
    }
    #[test]
    fn primary_channel_rejects_candidate_resource_ceiling_regression() {
        let mut a = fixture(false, "same");
        let mut b = fixture(true, "same");
        a.identity.product_binary_sha256 = "same-binary".into();
        b.identity.product_binary_sha256 = "same-binary".into();
        b.identity.config_sha256 = "g".repeat(64);
        for case in a.cases.iter_mut().chain(b.cases.iter_mut()) {
            case.score_observation_source = Some("primary".into());
            case.score_observation = Some(serde_json::json!({
                "sources": [],
                "disposition": "unknown",
                "item_count": 0,
                "budget_used": 0,
                "relation_outcomes": [],
                "guidance": null
            }));
        }
        b.cases[0].score_observation.as_mut().unwrap()["budget_used"] = serde_json::json!(6);
        let mut plan = plan();
        plan.candidate_config_sha256 = Some("g".repeat(64));
        plan.score_channel = AcceptanceScoreChannel::Primary;

        let report = accept_candidate(&plan, [&a, &repeated(&a)], [&b, &repeated(&b)])
            .expect("valid but over-budget candidate observation");

        assert!(!report.accepted);
        assert_eq!(report.hard_failures, vec!["case:resource_ceiling"]);
    }
    #[test]
    fn no_gain_is_a_completed_rejection() {
        let a = fixture(true, "a");
        let b = fixture(true, "b");
        assert!(
            !accept_candidate(&plan(), [&a, &repeated(&a)], [&b, &repeated(&b)])
                .unwrap()
                .accepted
        );
    }
    #[test]
    fn missing_repeat_observation_is_invalid() {
        let a = fixture(false, "a");
        let b = fixture(true, "b");
        let mut c = fixture(true, "b");
        c.cases[0].evidence_enabled_temporal_observation = None;
        assert!(accept_candidate(&plan(), [&a, &repeated(&a)], [&b, &c]).is_err());
    }
    #[test]
    fn missing_case_is_invalid() {
        let a = fixture(false, "a");
        let mut b = fixture(true, "b");
        b.cases.clear();
        assert!(accept_candidate(&plan(), [&a, &repeated(&a)], [&b, &repeated(&b)]).is_err());
    }
    #[test]
    fn hard_violation_defeats_positive_gain() {
        let a = fixture(false, "a");
        let mut b = fixture(true, "b");
        b.cases[0].evidence_enabled_temporal_score.as_mut().unwrap()["reason_codes"] =
            serde_json::json!(["forbidden_source"]);
        assert!(
            !accept_candidate(&plan(), [&a, &repeated(&a)], [&b, &repeated(&b)])
                .unwrap()
                .accepted
        );
    }
    #[test]
    fn incompatible_schema_cannot_be_compared() {
        let a = fixture(false, "a");
        let mut b = fixture(true, "b");
        b.identity.input_schema_sha256 = Some("f".repeat(64));
        assert!(accept_candidate(&plan(), [&a, &repeated(&a)], [&b, &repeated(&b)]).is_err());
    }
    #[test]
    fn reused_artifact_cannot_substitute_for_a_second_capture() {
        let a = fixture(false, "a");
        let b = fixture(true, "b");
        assert!(accept_candidate(&plan(), [&a, &a], [&b, &b]).is_err());
    }
    #[test]
    fn one_reused_case_cannot_hide_behind_another_fresh_capture() {
        let a = fixture(false, "a");
        let b = fixture(true, "b");
        let mut bb = repeated(&b);
        bb.cases[0].receipt_sha256 = b.cases[0].receipt_sha256.clone();
        assert!(accept_candidate(&plan(), [&a, &repeated(&a)], [&b, &bb]).is_err());
    }

    #[test]
    fn unsafe_current_role_cannot_be_paid_for_by_other_family_wins() {
        let mut a = fixture(false, "a");
        let mut b = fixture(true, "b");
        let mut old = a.cases[0].clone();
        old.case_id = "improvement".into();
        a.cases.push(old);
        let mut new = b.cases[0].clone();
        new.case_id = "improvement".into();
        b.cases.push(new);
        b.cases[0].evidence_enabled_temporal_score = Some(
            serde_json::json!({"evidence_success":true,"decision_success":false,"probe_success":false,"reason_codes":["incorrect_role"]}),
        );
        b.cases[0]
            .evidence_enabled_temporal_observation
            .as_mut()
            .unwrap()["sources"] =
            serde_json::json!([{"event_id":"source","text":"disputed","role":"current"}]);
        let mut p = plan();
        p.families
            .insert("improvement".into(), "third-family".into());
        let report = accept_candidate(&p, [&a, &repeated(&a)], [&b, &repeated(&b)]).unwrap();
        assert!(
            !report.accepted,
            "unsafe current role cannot be averaged away"
        );
        assert!(
            report
                .hard_failures
                .contains(&"case:unsafe_role_promotion".to_string())
        );
    }
}
