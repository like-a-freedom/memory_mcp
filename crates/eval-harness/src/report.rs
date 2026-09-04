use crate::artifact::RunArtifact;
use crate::error::EvalError;

pub fn render_markdown(artifact: &RunArtifact) -> Result<String, EvalError> {
    let mut out = String::new();

    out.push_str(&format!(
        "# Evaluation Report — {}\n\n",
        format!("{:?}", artifact.profile).to_lowercase()
    ));

    let verdict_str = match artifact.verdict {
        crate::domain::RunVerdict::Passed => "PASSED",
        crate::domain::RunVerdict::QualityFailed => "QUALITY FAILED",
        crate::domain::RunVerdict::Invalid => "INVALID",
    };
    out.push_str(&format!("**Verdict:** {verdict_str}\n"));
    out.push_str(&format!("**Schema:** `{}`\n", artifact.schema_version));
    out.push_str(&format!("**Run ID:** `{}`\n", artifact.run_id));
    out.push_str(&format!("**Duration:** {}ms\n", artifact.duration_ms));

    if let Some(ref budget) = artifact.budget_status {
        out.push_str(&format!(
            "**Budget:** {}\n",
            format!("{budget:?}").to_lowercase()
        ));
    }

    let failed_cases = artifact
        .outcomes
        .iter()
        .filter(|o| o.status == crate::domain::CaseStatus::QualityFailed)
        .count();
    let invalid_cases = artifact
        .outcomes
        .iter()
        .filter(|o| o.status == crate::domain::CaseStatus::Invalid)
        .count();
    let failed_gates = artifact
        .gates
        .iter()
        .filter(|g| g.status == crate::artifact::GateStatus::Failed)
        .count();
    let invalid_gates = artifact
        .gates
        .iter()
        .filter(|g| g.status == crate::artifact::GateStatus::Invalid)
        .count();

    out.push_str("\n## Coverage\n\n");
    let expected_cases = if artifact.expected_cases.is_empty() {
        artifact.expected_case_ids.len()
    } else {
        artifact.expected_cases.len()
    };
    out.push_str(&format!("**Expected cases:** {}\n", expected_cases));
    out.push_str(&format!(
        "**Outcomes:** {} (failed: {}, invalid: {})\n\n",
        artifact.outcomes.len(),
        failed_cases,
        invalid_cases
    ));
    out.push_str(&format!(
        "**Gates:** {} (failed: {}, invalid: {})\n\n",
        artifact.gates.len(),
        failed_gates,
        invalid_gates
    ));

    let passed = artifact
        .outcomes
        .iter()
        .filter(|o| o.status == crate::domain::CaseStatus::Passed)
        .count();
    let failed = artifact
        .outcomes
        .iter()
        .filter(|o| o.status == crate::domain::CaseStatus::QualityFailed)
        .count();
    let invalid = artifact
        .outcomes
        .iter()
        .filter(|o| o.status == crate::domain::CaseStatus::Invalid)
        .count();

    out.push_str(&format!(
        "| Status | Count |\n|--------|-------|\n| Passed | {passed} |\n| Quality Failed | {failed} |\n| Invalid | {invalid} |\n"
    ));

    if !artifact.suite_summaries.is_empty() {
        out.push_str("\n## Suite Metrics\n\n");
        out.push_str("| Suite | Total | Passed | Failed | Invalid | Metrics |\n");
        out.push_str("|-------|-------|--------|--------|---------|--------|\n");
        for summary in &artifact.suite_summaries {
            let metrics_str: Vec<String> = summary
                .metrics
                .iter()
                .map(|(k, v)| format!("{k}={v:.4}"))
                .collect();
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} |\n",
                summary.suite_id,
                summary.total,
                summary.passed,
                summary.quality_failed,
                summary.invalid,
                metrics_str.join(", ")
            ));
        }
    }

    if !artifact.gates.is_empty() {
        out.push_str("\n## Gates\n\n");
        out.push_str("| Suite | Metric | Observed | Floor | Baseline | Budget | Status |\n");
        out.push_str("|-------|--------|----------|-------|----------|--------|--------|\n");
        for gate in &artifact.gates {
            let floor = gate
                .hard_floor
                .map_or("-".to_string(), |f| format!("{f:.4}"));
            let baseline = gate.baseline.map_or("-".to_string(), |b| format!("{b:.4}"));
            let budget = gate
                .regression_budget
                .map_or("-".to_string(), |b| format!("{b:.4}"));
            out.push_str(&format!(
                "| {} | {} | {:.4} | {floor} | {baseline} | {budget} | {} |\n",
                gate.suite_id,
                gate.metric,
                gate.observed,
                format!("{:?}", gate.status).to_lowercase()
            ));
        }
    }

    let invalid_cases: Vec<_> = artifact
        .outcomes
        .iter()
        .filter(|o| o.status == crate::domain::CaseStatus::Invalid)
        .collect();
    if !invalid_cases.is_empty() {
        out.push_str("\n## Invalid Cases\n\n");
        for case in &invalid_cases {
            if let Some(reason) = &case.invalid_reason {
                out.push_str(&format!("- **{}**: {}\n", case.case_id().as_str(), reason));
            }
        }
    }

    Ok(out)
}
