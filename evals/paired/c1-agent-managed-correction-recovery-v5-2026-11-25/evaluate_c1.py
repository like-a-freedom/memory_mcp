#!/usr/bin/env python3
"""Apply the frozen C1 workflow gates to paired artifacts and private probes."""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import sys
from pathlib import Path
from typing import Any

import validate_recovery


PAIRED_SCHEMA = "memory-mcp-agent-paired/v8"
STAGE_SCHEMA = "memory-mcp-agent-managed-run/v6"
PLAN_ID = "c1-agent-managed-correction-recovery-v5-2026-11-25"


class EvaluationError(ValueError):
    pass


def load(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def identity_sha256(identity: dict[str, Any]) -> str:
    encoded = json.dumps(identity, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def validate_frozen_inputs(
    tasks_path: Path,
    criteria_path: Path,
    plan_path: Path,
    fingerprints_path: Path,
) -> None:
    fingerprints = load(fingerprints_path)
    if fingerprints.get("schema_version") != "memory-mcp-agent-managed-fingerprints/v1":
        raise EvaluationError("fingerprint manifest schema is unsupported")
    tasks = load(tasks_path)
    criteria = load(criteria_path)
    plan = load(plan_path)
    if tasks.get("schema_version") != "memory-mcp-agent-managed-tasks/v1":
        raise EvaluationError("registered task manifest schema is unsupported")
    if criteria.get("schema_version") != "memory-mcp-agent-managed-screen-criteria/v2":
        raise EvaluationError("registered criteria schema is unsupported")
    if plan.get("schema_version") != "memory-mcp-agent-plan/v2":
        raise EvaluationError("registered plan schema is unsupported")
    pinned_files = {
        "tasks_manifest_file_sha256": tasks_path,
        "criteria_file_sha256": criteria_path,
        "plan_file_sha256": plan_path,
        "c1_evaluator_sha256": Path(__file__),
        "recovery_validator_sha256": Path(validate_recovery.__file__),
    }
    for field, path in pinned_files.items():
        if fingerprints.get(field) != sha256_file(path):
            raise EvaluationError(f"registered {field} does not match the evaluated file")


def key_tuple(key: dict[str, Any]) -> tuple[Any, ...]:
    try:
        return (
            key["stratum_id"],
            key["family_id"],
            key["case_id"],
            key["repetition"],
            key["incumbent_first"],
        )
    except (KeyError, TypeError) as error:
        raise EvaluationError("trial key is incomplete") from error


def unique_by_key(items: Any, label: str, key_field: str | None = None) -> dict[tuple[Any, ...], dict[str, Any]]:
    if not isinstance(items, list):
        raise EvaluationError(f"{label} must be an array")
    result: dict[tuple[Any, ...], dict[str, Any]] = {}
    for item in items:
        if not isinstance(item, dict):
            raise EvaluationError(f"{label} contains a non-object")
        key = key_tuple(item[key_field] if key_field else item)
        if key in result:
            raise EvaluationError(f"{label} contains a duplicate trial key")
        result[key] = item
    return result


def operation(stage: dict[str, Any], name: str) -> dict[str, int]:
    matches = [
        item
        for item in stage.get("write_operations", [])
        if item.get("tool_name") == f"memory_decisions/{name}"
    ]
    if len(matches) != 1:
        return {"attempts": 0, "succeeded": 0, "failed": 0, "unresolved": 0}
    return matches[0]


def validate_registered_identity(
    arm: str, artifact: dict[str, Any], plan: dict[str, Any]
) -> None:
    identity = artifact.get("identity")
    if not isinstance(identity, dict) or identity.get("shared") != plan.get("shared_identity"):
        raise EvaluationError(f"{arm}: artifact shared identity differs from registered plan")
    registered_product = plan.get(f"{arm}_product")
    observed_product = identity.get("product")
    if not isinstance(registered_product, dict) or not isinstance(observed_product, dict):
        raise EvaluationError(f"{arm}: registered or observed product identity is missing")
    fields = (
        "revision",
        "binary_sha256",
        "product_config_sha256",
        "tool_schema_sha256",
        "enabled_features",
        "claim_rollout_stage",
    )
    if {field: observed_product.get(field) for field in fields} != {
        field: registered_product.get(field) for field in fields
    }:
        raise EvaluationError(f"{arm}: artifact product identity differs from registered plan")


def probe_matches_oracle(stage: dict[str, Any], task: dict[str, Any]) -> bool:
    probe = stage.get("independent_probe", {})
    oracle = task.get("oracle", {})
    observed = set(probe.get("source_event_ids", []))
    target = task.get("target_write_event_id")
    forbidden = set(oracle.get("forbidden_event_ids", []))
    alternatives = oracle.get("sufficient_evidence_alternatives", [])
    return (
        probe.get("evidence_success") is True
        and probe.get("unsupported_source_count") == 0
        and probe.get("prohibited_source_count") == 0
        and isinstance(target, str)
        and target in observed
        and not observed.intersection(forbidden)
        and any(
            alternative
            and all(any(source_id in observed for source_id in group) for group in alternative)
            for alternative in alternatives
        )
    )


def arm_metrics(
    arm: str,
    artifact_path: Path,
    artifact: dict[str, Any],
    supplement: dict[str, Any],
    expected: dict[tuple[Any, ...], dict[str, Any]],
    criteria: dict[str, Any],
) -> dict[str, Any]:
    if artifact.get("schema_version") != PAIRED_SCHEMA:
        raise EvaluationError(f"{arm}: paired artifact must use {PAIRED_SCHEMA}")
    if supplement.get("schema_version") != STAGE_SCHEMA:
        raise EvaluationError(f"{arm}: stage sidecar must use {STAGE_SCHEMA}")
    if artifact.get("arm") != arm or supplement.get("arm") != arm:
        raise EvaluationError(f"{arm}: arm labels do not match filenames")
    if artifact.get("run_id") != supplement.get("run_id"):
        raise EvaluationError(f"{arm}: paired artifact and stage sidecar run IDs differ")
    recovery = validate_recovery.validate_arm(
        artifact_path,
        {key[2] for key in expected},
    )

    receipts = unique_by_key(artifact.get("receipts"), f"{arm} receipts", "key")
    stages = unique_by_key(supplement.get("stages"), f"{arm} stages", "key")
    artifact_trials = unique_by_key(artifact.get("expected_trials"), f"{arm} expected trials")
    sidecar_trials = unique_by_key(supplement.get("expected_trials"), f"{arm} sidecar trials")
    if (
        receipts.keys() != expected.keys()
        or stages.keys() != expected.keys()
        or artifact_trials.keys() != expected.keys()
        or sidecar_trials.keys() != expected.keys()
    ):
        raise EvaluationError(f"{arm}: receipts or stage sidecars do not cover the frozen cohort")

    task_successes = 0
    exact_cited_answers = 0
    hard_violations = 0
    resource_exits = 0
    probe_successes = 0
    unsupported_or_prohibited = 0
    extracts = 0
    invalidations = 0
    exact_invalidations = 0
    evidence_before_invalidation = 0
    read_outcomes: dict[str, int] = {}
    read_count = 0
    first_read_exact_registered_question_matches = 0
    nonempty_read_source_coverage = 0
    recall_latencies = []

    for key, task in expected.items():
        receipt = receipts[key]
        stage = stages[key]
        if stage.get("exact_stale_fact_invalidation_verified") is True:
            exact_invalidations += 1
        formation_status = stage.get("formation", {}).get("status")
        status = receipt.get("status")
        diagnostics = receipt.get("task_oracle_diagnostics") or {}
        exact_and_cited = (
            diagnostics.get("final_answer_present") is True
            and diagnostics.get("final_answer_schema_valid") is True
            and diagnostics.get("accepted_answer_match") == "exact"
            and diagnostics.get("required_evidence_cited") is True
        )
        success = status == "completed" and receipt.get("task_oracle_passed") is True and exact_and_cited
        task_successes += int(success)
        exact_cited_answers += int(exact_and_cited)
        hard_violations += len(receipt.get("hard_violations", []))
        resource_exits += int(
            status in {"timed_out", "budget_exceeded"}
            or formation_status in {"timed_out", "budget_exceeded"}
        )
        probe = stage.get("independent_probe", {})
        probe_matches = probe_matches_oracle(stage, task)
        probe_successes += int(probe_matches)
        unsupported_or_prohibited += probe.get("unsupported_source_count", 0)
        unsupported_or_prohibited += probe.get("prohibited_source_count", 0)

        extract = operation(stage, "extract")
        invalidate = operation(stage, "invalidate")
        exact_extract = (
            extract.get("attempts") == criteria["required_extract_attempts_per_trial"]
            and extract.get("succeeded") == 1
            and extract.get("failed") == 0
            and extract.get("unresolved") == 0
        )
        exact_invalidate = (
            invalidate.get("attempts") == criteria["required_invalidation_attempts_per_trial"]
            and invalidate.get("succeeded") == 1
            and invalidate.get("failed") == 0
            and invalidate.get("unresolved") == 0
        )
        extracts += int(exact_extract)
        invalidations += int(exact_invalidate)
        audit = stage.get("fact_evidence_audit") or {}
        evidence_before_invalidation += int(
            audit.get("telemetry_trials") == 1
            and audit.get("extract_calls") == 1
            and audit.get("evidence_requests") == 1
            and audit.get("evidence_responses") == 1
            and audit.get("evidence_before_invalidation") == 1
        )
        if receipt.get("read_capture_status") != "observed":
            raise EvaluationError(f"{arm}:{key[2]}: read capture is incomplete")
        reads = receipt.get("read_diagnostics")
        if not isinstance(reads, list):
            raise EvaluationError(f"{arm}:{key[2]}: read diagnostics are missing")
        read_count += len(reads)
        registered_question = task.get("independent_probe", {}).get("query")
        if not isinstance(registered_question, str) or not registered_question:
            raise EvaluationError(f"{arm}:{key[2]}: registered question query is missing")
        if task.get("recall_prompt") != registered_question:
            raise EvaluationError(f"{arm}:{key[2]}: recall task and independent probe questions differ")
        registered_question_sha256 = hashlib.sha256(
            registered_question.encode("utf-8")
        ).hexdigest()
        first_read_exact_registered_question_matches += int(
            bool(reads)
            and isinstance(reads[0], dict)
            and reads[0].get("request_query_sha256") == registered_question_sha256
        )
        for read in reads:
            outcome = read.get("outcome", "unknown")
            read_outcomes[outcome] = read_outcomes.get(outcome, 0) + 1
            if outcome == "nonempty" and read.get("source_coverage") is True:
                nonempty_read_source_coverage += 1
        latency = receipt.get("wall_time_ms")
        if type(latency) is not int or latency < 0:
            raise EvaluationError(f"{arm}:{key[2]}: recall phase wall time is missing or invalid")
        recall_latencies.append(latency)

    recovery_trials = recovery["trials"]
    n = len(expected)
    gates = {
        "task_success_floor": task_successes >= criteria["minimum_task_successes_per_arm"],
        "independent_probe_floor": probe_successes >= criteria["minimum_independent_probe_successes_per_arm"],
        "exact_extract_floor": extracts >= criteria["minimum_successful_extracts_per_arm"],
        "successful_invalidation_floor": invalidations >= criteria["minimum_successful_invalidations_per_arm"],
        "exact_stale_fact_invalidation_floor": exact_invalidations >= criteria["minimum_exact_stale_fact_invalidations_verified_per_arm"],
        "evidence_before_invalidation_floor": evidence_before_invalidation >= criteria["minimum_evidence_before_invalidation_per_arm"],
        "bounded_retry_contract": len(recovery["retry_contract_violations"])
        <= criteria["maximum_bounded_retry_contract_violations_per_arm"],
        "zero_untrusted_probe_sources": unsupported_or_prohibited <= criteria["maximum_unsupported_or_prohibited_sources_per_arm"],
        "zero_hard_violations": hard_violations <= criteria["maximum_hard_violations_per_arm"],
        "zero_resource_exits": resource_exits <= criteria["maximum_resource_or_timeout_exits_per_arm"],
    }
    if recovery_trials != n:
        raise EvaluationError(f"{arm}: recovery validator trial count differs from cohort")
    return {
        "trials": n,
        "task_successes": task_successes,
        "exact_cited_answers": exact_cited_answers,
        "independent_probe_successes": probe_successes,
        "successful_exact_once_extracts": extracts,
        "successful_exact_once_invalidations": invalidations,
        "exact_stale_fact_invalidation_verified": exact_invalidations,
        "evidence_verified_before_invalidation": evidence_before_invalidation,
        "guidance_authorized_retry_trials": recovery["guidance_authorized_retry_trials"],
        "retry_contract_violations": recovery["retry_contract_violations"],
        "read_count": read_count,
        "first_read_exact_registered_question_matches": first_read_exact_registered_question_matches,
        "read_outcomes": dict(sorted(read_outcomes.items())),
        "nonempty_reads_with_source_coverage": nonempty_read_source_coverage,
        "hard_violations": hard_violations,
        "unsupported_or_prohibited_probe_sources": unsupported_or_prohibited,
        "resource_or_timeout_exits": resource_exits,
        "recall_wall_time_ms": {
            "mean": statistics.mean(recall_latencies),
            "median": statistics.median(recall_latencies),
            "max": max(recall_latencies),
        },
        "gates": gates,
        "passed": all(gates.values()),
    }


def evaluate(
    incumbent_path: Path,
    candidate_path: Path,
    incumbent_stages_path: Path,
    candidate_stages_path: Path,
    tasks_path: Path,
    criteria_path: Path,
    plan_path: Path,
    fingerprints_path: Path,
) -> dict[str, Any]:
    validate_frozen_inputs(tasks_path, criteria_path, plan_path, fingerprints_path)
    tasks_document = load(tasks_path)
    criteria = load(criteria_path)
    plan = load(plan_path)
    if plan.get("plan_id") != PLAN_ID:
        raise EvaluationError("registered C1 plan ID does not match evaluator")
    tasks = tasks_document.get("tasks")
    if not isinstance(tasks, list) or not tasks:
        raise EvaluationError("registered task manifest is empty or malformed")
    expected = unique_by_key(tasks, "registered tasks", "key")
    if len(expected) != criteria.get("expected_trials"):
        raise EvaluationError("criteria trial count differs from registered tasks")
    families = {key[1] for key in expected}
    if len(families) != criteria.get("expected_independent_families"):
        raise EvaluationError("registered independent family count differs from criteria")
    planned_trials = unique_by_key(plan.get("expected_trials"), "planned trials")
    if planned_trials.keys() != expected.keys():
        raise EvaluationError("plan trial coverage differs from registered tasks")
    if criteria.get("maximum_read_capture_status_other_than_observed_per_arm") != 0:
        raise EvaluationError("evaluator requires complete read-capture status")
    if criteria.get("maximum_bounded_retry_contract_violations_per_arm") != 0:
        raise EvaluationError("evaluator requires zero bounded-retry contract violations")

    incumbent = load(incumbent_path)
    candidate = load(candidate_path)
    incumbent_stages = load(incumbent_stages_path)
    candidate_stages = load(candidate_stages_path)
    validate_registered_identity("incumbent", incumbent, plan)
    validate_registered_identity("candidate", candidate, plan)
    if incumbent.get("run_id") != candidate.get("run_id"):
        raise EvaluationError("paired arms do not share one paired run")
    expected_incumbent_identity = identity_sha256(incumbent["identity"])
    expected_candidate_identity = identity_sha256(candidate["identity"])
    if incumbent_stages.get("identity_sha256") != expected_incumbent_identity:
        raise EvaluationError("incumbent stage sidecar identity differs from its paired artifact")
    if candidate_stages.get("identity_sha256") != expected_candidate_identity:
        raise EvaluationError("candidate stage sidecar identity differs from its paired artifact")
    if incumbent.get("identity") != candidate.get("identity"):
        raise EvaluationError("paired arms do not share the same frozen run identity")
    if incumbent_stages.get("identity_sha256") != candidate_stages.get("identity_sha256"):
        raise EvaluationError("paired sidecars do not share the same run identity")
    incumbent_report = arm_metrics(
        "incumbent", incumbent_path, incumbent, incumbent_stages, expected, criteria
    )
    candidate_report = arm_metrics(
        "candidate", candidate_path, candidate, candidate_stages, expected, criteria
    )
    passed = incumbent_report["passed"] and candidate_report["passed"]
    return {
        "schema_version": "c1-agent-managed-correction-recovery-evaluation/v1",
        "plan_id": PLAN_ID,
        "interpretation": "same-product workflow qualification; not a treatment-effect estimate",
        "exact_invalidation_evidence": "private successful-read fact_id/source match precedes invalidation; IDs and sequence values are not persisted",
        "eligible_for_capture_02": passed,
        "status": "pass" if passed else "no_go_or_inconclusive",
        "incumbent": incumbent_report,
        "candidate": candidate_report,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--incumbent", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--incumbent-stages", type=Path, required=True)
    parser.add_argument("--candidate-stages", type=Path, required=True)
    parser.add_argument("--tasks", type=Path, default=Path(__file__).with_name("tasks.json"))
    parser.add_argument("--criteria", type=Path, default=Path(__file__).with_name("criteria.json"))
    parser.add_argument("--plan", type=Path, default=Path(__file__).with_name("plan.json"))
    parser.add_argument("--fingerprints", type=Path, default=Path(__file__).with_name("fingerprints.json"))
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    try:
        report = evaluate(
            args.incumbent,
            args.candidate,
            args.incumbent_stages,
            args.candidate_stages,
            args.tasks,
            args.criteria,
            args.plan,
            args.fingerprints,
        )
        encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
        args.output.write_text(encoded, encoding="utf-8")
        sys.stdout.write(encoded)
        return 0 if report["eligible_for_capture_02"] else 1
    except (
        OSError,
        json.JSONDecodeError,
        KeyError,
        TypeError,
        EvaluationError,
        validate_recovery.ValidationError,
    ) as error:
        sys.stderr.write(f"C1 evaluation failed: {error}\n")
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
