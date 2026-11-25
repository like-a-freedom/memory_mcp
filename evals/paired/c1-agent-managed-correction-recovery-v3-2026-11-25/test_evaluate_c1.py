import json
import hashlib
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import evaluate_c1
import validate_recovery


def key(index: int) -> dict:
    return {
        "stratum_id": "c1",
        "family_id": f"family-{index}",
        "case_id": f"case-{index}",
        "repetition": 1,
        "incumbent_first": index % 2 == 0,
    }


def task(index: int) -> dict:
    return {
        "key": key(index),
        "recall_prompt": f"registered-question-{index}",
        "independent_probe": {"query": f"registered-question-{index}"},
        "target_write_event_id": f"event-new-{index}",
        "oracle": {
            "sufficient_evidence_alternatives": [[[f"event-new-{index}"]]],
            "forbidden_event_ids": [f"event-old-{index}"],
        },
    }


def read(query: str, *, outcome: str, guidance: str | None = None,
         completeness: str = "partial") -> dict:
    query_sha256 = (
        query
        if len(query) == 64
        else hashlib.sha256(query.encode("utf-8")).hexdigest()
    )
    return {
        "outcome": outcome,
        "request_query_sha256": query_sha256,
        "non_query_arguments_sha256": "c" * 64,
        "response_guidance_sha256": guidance,
        "source_coverage": outcome == "nonempty",
        "completeness": completeness,
    }


def files(
    directory: Path,
    *,
    wrong_invalidation: bool = False,
    task_failures: int = 0,
    retry_exposure: bool = True,
    unauthorized_retry: bool = False,
    query_mismatch: bool = False,
):
    tasks = [task(index) for index in range(4)]
    criteria = {
        "expected_trials": 4,
        "expected_independent_families": 4,
        "minimum_task_successes_per_arm": 3,
        "minimum_independent_probe_successes_per_arm": 4,
        "minimum_successful_extracts_per_arm": 4,
        "minimum_successful_invalidations_per_arm": 4,
        "minimum_exact_stale_fact_invalidations_verified_per_arm": 4,
        "minimum_evidence_before_invalidation_per_arm": 4,
        "required_extract_attempts_per_trial": 1,
        "required_invalidation_attempts_per_trial": 1,
        "maximum_read_capture_status_other_than_observed_per_arm": 0,
        "maximum_bounded_retry_contract_violations_per_arm": 0,
        "maximum_unsupported_or_prohibited_sources_per_arm": 0,
        "maximum_hard_violations_per_arm": 0,
        "maximum_resource_or_timeout_exits_per_arm": 0,
    }
    identity = {
        "shared": {"model": "synthetic"},
        "product": {
            "revision": "test+dirty",
            "binary_sha256": "a" * 64,
            "product_config_sha256": "b" * 64,
            "tool_schema_sha256": "c" * 64,
            "enabled_features": ["fs-watch"],
            "claim_rollout_stage": "evidence",
        },
    }
    identity_sha256 = hashlib.sha256(
        json.dumps(identity, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    ).hexdigest()
    plan = {
        "schema_version": "memory-mcp-agent-plan/v2",
        "plan_id": "c1-agent-managed-correction-recovery-v3-2026-11-25",
        "expected_trials": [entry["key"] for entry in tasks],
        "shared_identity": identity["shared"],
        "incumbent_product": identity["product"],
        "candidate_product": identity["product"],
    }
    paths = {}
    for arm in ("incumbent", "candidate"):
        receipts = []
        stages = []
        for index, item in enumerate(tasks):
            registered_question = item["independent_probe"]["query"]
            first_query = (
                "unregistered-paraphrase"
                if query_mismatch and index == 0
                else registered_question
            )
            if retry_exposure and index == 0:
                guidance = (
                    "f" * 64
                    if unauthorized_retry
                    else next(iter(validate_recovery.AUTHORIZED_PARTIAL_GUIDANCE_SHA256S))
                )
                reads = [
                    read(
                        first_query,
                        outcome="empty",
                        guidance=guidance,
                    ),
                    read(
                        f"narrowed: {registered_question}",
                        outcome="nonempty",
                        completeness="partial",
                    ),
                ]
            else:
                reads = [
                    read(first_query, outcome="nonempty")
                ]
            task_ok = index >= task_failures
            receipts.append(
                {
                    "key": item["key"],
                    "arm_order": 1,
                    "status": "completed",
                    "task_oracle_passed": task_ok,
                    "task_oracle_diagnostics": {
                        "final_answer_present": True,
                        "final_answer_schema_valid": True,
                        "accepted_answer_match": "exact" if task_ok else "no_accepted_value_mentioned",
                        "required_evidence_cited": task_ok,
                    },
                    "hard_violations": [],
                    "read_capture_status": "observed",
                    "read_diagnostics": reads,
                    "wall_time_ms": 100 + index,
                }
            )
            stages.append(
                {
                    "key": item["key"],
                    "formation": {"status": "completed"},
                    "write_operations": [
                        {"tool_name": "memory_decisions/extract", "attempts": 1, "succeeded": 1, "failed": 0, "unresolved": 0},
                        {"tool_name": "memory_decisions/invalidate", "attempts": 1, "succeeded": 1, "failed": 0, "unresolved": 0},
                    ],
                    "exact_stale_fact_invalidation_verified": not wrong_invalidation,
                    "independent_probe": {
                        "evidence_success": True,
                        "source_event_ids": [f"event-new-{index}"],
                        "unsupported_source_count": 0,
                        "prohibited_source_count": 0,
                    },
                    "fact_evidence_audit": {
                        "telemetry_trials": 1,
                        "extract_calls": 1,
                        "evidence_requests": 1,
                        "evidence_responses": 1,
                        "evidence_before_invalidation": 1,
                    },
                }
            )
        run_id = "run-1"
        paired = {
            "schema_version": evaluate_c1.PAIRED_SCHEMA,
            "arm": arm,
            "run_id": run_id,
            "identity": identity,
            "expected_trials": [entry["key"] for entry in tasks],
            "receipts": receipts,
        }
        sidecar = {
            "schema_version": evaluate_c1.STAGE_SCHEMA,
            "arm": arm,
            "run_id": run_id,
            "identity_sha256": identity_sha256,
            "expected_trials": [entry["key"] for entry in tasks],
            "stages": stages,
        }
        for suffix, value in (("paired", paired), ("stages", sidecar)):
            path = directory / f"{arm}-{suffix}.json"
            path.write_text(json.dumps(value), encoding="utf-8")
            paths[(arm, suffix)] = path
    task_path = directory / "tasks.json"
    task_path.write_text(
        json.dumps(
            {"schema_version": "memory-mcp-agent-managed-tasks/v1", "tasks": tasks}
        ),
        encoding="utf-8",
    )
    criteria_path = directory / "criteria.json"
    criteria["schema_version"] = "memory-mcp-agent-managed-screen-criteria/v2"
    criteria_path.write_text(json.dumps(criteria), encoding="utf-8")
    plan_path = directory / "plan.json"
    plan_path.write_text(json.dumps(plan), encoding="utf-8")
    fingerprints = {
        "schema_version": "memory-mcp-agent-managed-fingerprints/v1",
        "plan_file_sha256": hashlib.sha256(plan_path.read_bytes()).hexdigest(),
        "tasks_manifest_file_sha256": hashlib.sha256(task_path.read_bytes()).hexdigest(),
        "criteria_file_sha256": hashlib.sha256(criteria_path.read_bytes()).hexdigest(),
        "c1_evaluator_sha256": hashlib.sha256(Path(evaluate_c1.__file__).read_bytes()).hexdigest(),
        "recovery_validator_sha256": hashlib.sha256(Path(validate_recovery.__file__).read_bytes()).hexdigest(),
    }
    fingerprints_path = directory / "fingerprints.json"
    fingerprints_path.write_text(json.dumps(fingerprints), encoding="utf-8")
    return paths, task_path, criteria_path, plan_path, fingerprints_path


class C1EvaluationTests(unittest.TestCase):
    def evaluate_fixture(
        self, *, wrong_invalidation=False, task_failures=0, retry_exposure=True,
        unauthorized_retry=False, query_mismatch=False
    ):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        paths, tasks_path, criteria_path, plan_path, fingerprints_path = files(
            directory,
            wrong_invalidation=wrong_invalidation,
            task_failures=task_failures,
            retry_exposure=retry_exposure,
            unauthorized_retry=unauthorized_retry,
            query_mismatch=query_mismatch,
        )
        return evaluate_c1.evaluate(
            paths[("incumbent", "paired")],
            paths[("candidate", "paired")],
            paths[("incumbent", "stages")],
            paths[("candidate", "stages")],
            tasks_path,
            criteria_path,
            plan_path,
            fingerprints_path,
        )

    def test_all_registered_gates_pass_on_complete_synthetic_receipts(self):
        report = self.evaluate_fixture()

        self.assertTrue(report["eligible_for_capture_02"])
        self.assertEqual(report["incumbent"]["guidance_authorized_retry_trials"], 1)
        self.assertEqual(report["incumbent"]["first_read_exact_registered_question_matches"], 4)
        self.assertEqual(report["candidate"]["exact_stale_fact_invalidation_verified"], 4)

    def test_zero_retry_exposure_is_not_a_failure_when_the_workflow_succeeds(self):
        report = self.evaluate_fixture(retry_exposure=False)

        self.assertTrue(report["eligible_for_capture_02"])
        self.assertEqual(report["incumbent"]["guidance_authorized_retry_trials"], 0)
        self.assertTrue(report["incumbent"]["gates"]["bounded_retry_contract"])

    def test_first_read_exact_question_match_is_diagnostic_not_a_gate(self):
        report = self.evaluate_fixture(retry_exposure=False, query_mismatch=True)

        self.assertTrue(report["eligible_for_capture_02"])
        self.assertEqual(report["incumbent"]["first_read_exact_registered_question_matches"], 3)

    def test_unauthorized_retry_is_reported_and_blocks_repeat(self):
        report = self.evaluate_fixture(unauthorized_retry=True)

        self.assertFalse(report["eligible_for_capture_02"])
        self.assertFalse(report["incumbent"]["gates"]["bounded_retry_contract"])
        self.assertTrue(report["incumbent"]["retry_contract_violations"])

    def test_wrong_stale_fact_target_blocks_repeat_even_when_tool_succeeded(self):
        report = self.evaluate_fixture(wrong_invalidation=True)

        self.assertFalse(report["eligible_for_capture_02"])
        self.assertFalse(report["candidate"]["gates"]["exact_stale_fact_invalidation_floor"])

    def test_task_floor_failure_blocks_repeat_despite_all_probes_passing(self):
        report = self.evaluate_fixture(task_failures=2)

        self.assertFalse(report["eligible_for_capture_02"])
        self.assertFalse(report["incumbent"]["gates"]["task_success_floor"])

    def test_artifact_identity_must_match_registered_plan(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        paths, tasks_path, criteria_path, plan_path, fingerprints_path = files(directory)
        candidate_path = paths[("candidate", "paired")]
        candidate = json.loads(candidate_path.read_text(encoding="utf-8"))
        candidate["identity"]["shared"]["model"] = "unregistered"
        candidate_path.write_text(json.dumps(candidate), encoding="utf-8")

        with self.assertRaises(evaluate_c1.EvaluationError):
            evaluate_c1.evaluate(
                paths[("incumbent", "paired")],
                candidate_path,
                paths[("incumbent", "stages")],
                paths[("candidate", "stages")],
                tasks_path,
                criteria_path,
                plan_path,
                fingerprints_path,
            )

    def test_task_manifest_must_match_registered_raw_hash(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        paths, tasks_path, criteria_path, plan_path, fingerprints_path = files(directory)
        tasks = json.loads(tasks_path.read_text(encoding="utf-8"))
        tasks["tasks"][0]["target_write_event_id"] = "tampered-event"
        tasks_path.write_text(json.dumps(tasks), encoding="utf-8")

        with self.assertRaises(evaluate_c1.EvaluationError):
            evaluate_c1.evaluate(
                paths[("incumbent", "paired")],
                paths[("candidate", "paired")],
                paths[("incumbent", "stages")],
                paths[("candidate", "stages")],
                tasks_path,
                criteria_path,
                plan_path,
                fingerprints_path,
            )

    def test_stage_sidecar_identity_must_match_its_paired_artifact(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        paths, tasks_path, criteria_path, plan_path, fingerprints_path = files(directory)
        for arm in ("incumbent", "candidate"):
            sidecar_path = paths[(arm, "stages")]
            sidecar = json.loads(sidecar_path.read_text(encoding="utf-8"))
            sidecar["identity_sha256"] = "d" * 64
            sidecar_path.write_text(json.dumps(sidecar), encoding="utf-8")

        with self.assertRaises(evaluate_c1.EvaluationError):
            evaluate_c1.evaluate(
                paths[("incumbent", "paired")],
                paths[("candidate", "paired")],
                paths[("incumbent", "stages")],
                paths[("candidate", "stages")],
                tasks_path,
                criteria_path,
                plan_path,
                fingerprints_path,
            )

    def test_paired_arms_must_share_one_runner_invocation(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        paths, tasks_path, criteria_path, plan_path, fingerprints_path = files(directory)
        for suffix in ("paired", "stages"):
            path = paths[("candidate", suffix)]
            artifact = json.loads(path.read_text(encoding="utf-8"))
            artifact["run_id"] = "different-run"
            path.write_text(json.dumps(artifact), encoding="utf-8")

        with self.assertRaisesRegex(evaluate_c1.EvaluationError, "one paired run"):
            evaluate_c1.evaluate(
                paths[("incumbent", "paired")],
                paths[("candidate", "paired")],
                paths[("incumbent", "stages")],
                paths[("candidate", "stages")],
                tasks_path,
                criteria_path,
                plan_path,
                fingerprints_path,
            )

    def test_malformed_read_diagnostic_fails_closed(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        directory = Path(temporary.name)
        paths, tasks_path, criteria_path, plan_path, fingerprints_path = files(directory)
        candidate_path = paths[("candidate", "paired")]
        candidate = json.loads(candidate_path.read_text(encoding="utf-8"))
        candidate["receipts"][0]["read_diagnostics"][0] = "malformed"
        candidate_path.write_text(json.dumps(candidate), encoding="utf-8")

        with self.assertRaisesRegex(validate_recovery.ValidationError, "read diagnostic"):
            evaluate_c1.evaluate(
                paths[("incumbent", "paired")],
                candidate_path,
                paths[("incumbent", "stages")],
                paths[("candidate", "stages")],
                tasks_path,
                criteria_path,
                plan_path,
                fingerprints_path,
            )


if __name__ == "__main__":
    unittest.main()
