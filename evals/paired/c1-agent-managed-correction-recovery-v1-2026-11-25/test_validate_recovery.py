import json
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import validate_recovery


def read(query: str, other: str, *, outcome: str = "empty", guidance: str | None = None) -> dict:
    return {
        "outcome": outcome,
        "request_query_sha256": query * 64,
        "non_query_arguments_sha256": other * 64,
        "response_guidance_sha256": guidance,
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
    def test_accepts_one_query_only_retry_after_registered_empty_guidance(self):
        first = read(
            "a",
            "c",
            guidance=validate_recovery.EMPTY_REFINEMENT_GUIDANCE_SHA256,
        )
        retry = read("b", "c", outcome="nonempty")

        report = validate_recovery.validate_arm(
            self.write_artifact("candidate", [first, retry]), {"case-1"}
        )

        self.assertEqual(report["guidance_authorized_retry_trials"], 1)
        self.assertEqual(report["reads"], 2)

    def test_rejects_retry_that_changes_non_query_arguments(self):
        first = read(
            "a",
            "c",
            guidance=validate_recovery.EMPTY_REFINEMENT_GUIDANCE_SHA256,
        )
        retry = read("b", "d", outcome="nonempty")

        with self.assertRaisesRegex(validate_recovery.ValidationError, "non-query arguments"):
            validate_recovery.validate_arm(
                self.write_artifact("candidate", [first, retry]), {"case-1"}
            )

    def test_rejects_retry_after_failed_or_nonempty_read(self):
        first = read(
            "a",
            "c",
            outcome="failed",
            guidance=validate_recovery.EMPTY_REFINEMENT_GUIDANCE_SHA256,
        )

        with self.assertRaisesRegex(validate_recovery.ValidationError, "non-empty or failed"):
            validate_recovery.validate_arm(
                self.write_artifact("candidate", [first, read("b", "c")]), {"case-1"}
            )

    def test_rejects_retry_without_exact_registered_guidance(self):
        with self.assertRaisesRegex(validate_recovery.ValidationError, "registered refinement"):
            validate_recovery.validate_arm(
                self.write_artifact("candidate", [read("a", "c", guidance="f" * 64), read("b", "c")]),
                {"case-1"},
            )

    def test_rejects_incomplete_read_capture(self):
        with self.assertRaisesRegex(validate_recovery.ValidationError, "not complete"):
            validate_recovery.validate_arm(
                self.write_artifact("candidate", [read("a", "c")], status="partial"),
                {"case-1"},
            )

    def write_artifact(
        self, arm: str, reads: list[dict], *, status: str = "observed"
    ) -> Path:
        temporary = tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", delete=False)
        with temporary:
            json.dump(artifact(arm, reads, status=status), temporary)
        self.addCleanup(Path(temporary.name).unlink, missing_ok=True)
        return Path(temporary.name)


if __name__ == "__main__":
    unittest.main()
