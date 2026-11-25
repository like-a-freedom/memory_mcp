import hashlib
import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import validate_recovery


TRUNCATED_PARTIAL_GUIDANCE = (
    "The historical candidate scan was truncated and returned no items. You may retry once "
    "with a narrower query while keeping the snapshot and all other arguments unchanged. "
    "An empty retry still does not establish absence; verify against the source."
)
OMITTED_CHANNELS_PARTIAL_GUIDANCE = (
    "No matching items were returned from this partial historical snapshot. You may retry "
    "once with a more specific query while keeping the snapshot and all other arguments "
    "unchanged. A second empty result still does not establish absence because some selector "
    "channels were omitted; verify against the source."
)
AUTHORIZED_GUIDANCE = {
    hashlib.sha256(TRUNCATED_PARTIAL_GUIDANCE.encode()).hexdigest(),
    hashlib.sha256(OMITTED_CHANNELS_PARTIAL_GUIDANCE.encode()).hexdigest(),
}


def read(query: str, other: str, *, outcome: str = "empty", guidance: str | None = None,
         completeness: str = "partial") -> dict:
    return {
        "outcome": outcome,
        "request_query_sha256": query * 64,
        "non_query_arguments_sha256": other * 64,
        "response_guidance_sha256": guidance,
        "completeness": completeness,
    }


def artifact(arm: str, reads: list[dict], *, status: str = "observed") -> dict:
    return {
        "schema_version": validate_recovery.ARTIFACT_SCHEMA,
        "arm": arm,
        "receipts": [
            {
                "key": {"case_id": "case-1"},
                "read_capture_status": status,
                "read_diagnostics": reads,
            }
        ],
    }


class BoundedRetryValidationTests(unittest.TestCase):
    def write_artifact(self, arm: str, reads: list[dict], *, status: str = "observed") -> Path:
        temporary = tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", delete=False)
        with temporary:
            json.dump(artifact(arm, reads, status=status), temporary)
        self.addCleanup(Path(temporary.name).unlink, missing_ok=True)
        return Path(temporary.name)

    def test_accepts_one_empty_partial_read_without_retry_exposure_floor(self):
        report = validate_recovery.validate_arm(
            self.write_artifact("candidate", [read("a", "c")]), {"case-1"}
        )

        self.assertEqual(report["guidance_authorized_retry_trials"], 0)
        self.assertEqual(report["retry_contract_violations"], [])

    def test_accepts_one_query_only_retry_for_registered_partial_guidance(self):
        first = read("a", "c", guidance=next(iter(AUTHORIZED_GUIDANCE)))
        retry = read("b", "c", outcome="nonempty")

        report = validate_recovery.validate_arm(
            self.write_artifact("candidate", [first, retry]), {"case-1"}
        )

        self.assertEqual(report["guidance_authorized_retry_trials"], 1)
        self.assertEqual(report["retry_contract_violations"], [])

    def test_rejects_retry_after_unregistered_guidance(self):
        report = validate_recovery.validate_arm(
            self.write_artifact("candidate", [read("a", "c", guidance="f" * 64), read("b", "c")]),
            {"case-1"},
        )

        self.assertEqual(len(report["retry_contract_violations"]), 1)
        self.assertIn("guidance", report["retry_contract_violations"][0])

    def test_rejects_retry_after_failed_or_nonempty_read(self):
        report = validate_recovery.validate_arm(
            self.write_artifact(
                "candidate",
                [read("a", "c", outcome="failed", guidance=next(iter(AUTHORIZED_GUIDANCE))), read("b", "c")],
            ),
            {"case-1"},
        )

        self.assertEqual(len(report["retry_contract_violations"]), 1)
        self.assertIn("empty partial", report["retry_contract_violations"][0])

    def test_rejects_retry_that_changes_non_query_arguments(self):
        guidance = next(iter(AUTHORIZED_GUIDANCE))
        report = validate_recovery.validate_arm(
            self.write_artifact("candidate", [read("a", "c", guidance=guidance), read("b", "d")]),
            {"case-1"},
        )

        self.assertEqual(len(report["retry_contract_violations"]), 1)
        self.assertIn("non-query", report["retry_contract_violations"][0])

    def test_rejects_retry_that_does_not_change_query(self):
        guidance = next(iter(AUTHORIZED_GUIDANCE))
        report = validate_recovery.validate_arm(
            self.write_artifact("candidate", [read("a", "c", guidance=guidance), read("a", "c")]),
            {"case-1"},
        )

        self.assertEqual(len(report["retry_contract_violations"]), 1)
        self.assertIn("query", report["retry_contract_violations"][0])

    def test_requires_at_least_one_observed_agent_read_per_trial(self):
        with self.assertRaisesRegex(validate_recovery.ValidationError, "at least one read"):
            validate_recovery.validate_arm(self.write_artifact("candidate", []), {"case-1"})

    def test_rejects_more_than_one_retry_as_contract_violation(self):
        guidance = next(iter(AUTHORIZED_GUIDANCE))
        report = validate_recovery.validate_arm(
            self.write_artifact(
                "candidate",
                [read("a", "c", guidance=guidance), read("b", "c"), read("d", "c")],
            ),
            {"case-1"},
        )

        self.assertEqual(len(report["retry_contract_violations"]), 1)
        self.assertIn("more than one", report["retry_contract_violations"][0])

    def test_rejects_incomplete_read_capture(self):
        with self.assertRaisesRegex(validate_recovery.ValidationError, "not complete"):
            validate_recovery.validate_arm(
                self.write_artifact("candidate", [read("a", "c")], status="partial"), {"case-1"}
            )


if __name__ == "__main__":
    unittest.main()
