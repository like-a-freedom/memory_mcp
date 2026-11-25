#!/usr/bin/env python3
"""Deterministically validate the preregistered bounded-retry contract."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path
from typing import Any


ARTIFACT_SCHEMA = "memory-mcp-agent-paired/v8"
EMPTY_REFINEMENT_GUIDANCE = (
    "No sufficient matching context was found. Refine the query or verify primary sources."
)
EMPTY_REFINEMENT_GUIDANCE_SHA256 = "da0f34b581e67719d4359517a266dfbec313b6550e6568ee6a954a7610883961"
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


class ValidationError(ValueError):
    pass


def guidance_sha256() -> str:
    return hashlib.sha256(EMPTY_REFINEMENT_GUIDANCE.encode("utf-8")).hexdigest()


def validate_arm(artifact_path: Path, expected_cases: set[str]) -> dict[str, int | str]:
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    if artifact.get("schema_version") != ARTIFACT_SCHEMA:
        raise ValidationError(f"{artifact_path}: requires {ARTIFACT_SCHEMA}")

    receipts = artifact.get("receipts")
    if not isinstance(receipts, list):
        raise ValidationError(f"{artifact_path}: receipts must be an array")
    actual_cases: set[str] = set()
    total_reads = 0
    retry_trials = 0
    for receipt in receipts:
        key = receipt.get("key", {})
        case_id = key.get("case_id")
        if not isinstance(case_id, str) or case_id in actual_cases:
            raise ValidationError(f"{artifact_path}: invalid or duplicate case_id")
        actual_cases.add(case_id)
        if receipt.get("read_capture_status") != "observed":
            raise ValidationError(f"{artifact_path}:{case_id}: read capture is not complete")
        reads = receipt.get("read_diagnostics")
        if not isinstance(reads, list):
            raise ValidationError(f"{artifact_path}:{case_id}: missing read diagnostics")
        if len(reads) > 2:
            raise ValidationError(f"{artifact_path}:{case_id}: more than one retry")
        total_reads += len(reads)

        for read in reads:
            for name in ("request_query_sha256", "non_query_arguments_sha256"):
                digest = read.get(name)
                if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
                    raise ValidationError(f"{artifact_path}:{case_id}: missing or invalid {name}")
            guidance = read.get("response_guidance_sha256")
            if guidance is not None and (
                not isinstance(guidance, str) or not SHA256_RE.fullmatch(guidance)
            ):
                raise ValidationError(f"{artifact_path}:{case_id}: invalid guidance digest")

        if len(reads) == 2:
            first, retry = reads
            if first.get("outcome") != "empty":
                raise ValidationError(f"{artifact_path}:{case_id}: retry followed a non-empty or failed read")
            if first.get("response_guidance_sha256") != EMPTY_REFINEMENT_GUIDANCE_SHA256:
                raise ValidationError(f"{artifact_path}:{case_id}: retry lacked registered refinement guidance")
            if first.get("request_query_sha256") == retry.get("request_query_sha256"):
                raise ValidationError(f"{artifact_path}:{case_id}: retry did not change query wording")
            if first.get("non_query_arguments_sha256") != retry.get("non_query_arguments_sha256"):
                raise ValidationError(f"{artifact_path}:{case_id}: retry changed non-query arguments")
            retry_trials += 1

    if actual_cases != expected_cases:
        missing = sorted(expected_cases - actual_cases)
        unexpected = sorted(actual_cases - expected_cases)
        raise ValidationError(
            f"{artifact_path}: case coverage differs (missing={missing}, unexpected={unexpected})"
        )
    return {
        "arm": artifact.get("arm", "unknown"),
        "trials": len(receipts),
        "reads": total_reads,
        "guidance_authorized_retry_trials": retry_trials,
    }


def validate(
    incumbent_path: Path,
    candidate_path: Path,
    tasks_path: Path,
    criteria_path: Path,
) -> dict[str, Any]:
    if guidance_sha256() != EMPTY_REFINEMENT_GUIDANCE_SHA256:
        raise ValidationError("frozen refinement-guidance text does not match its registered digest")
    tasks = json.loads(tasks_path.read_text(encoding="utf-8"))
    expected_cases = {task["key"]["case_id"] for task in tasks["tasks"]}
    if len(expected_cases) != len(tasks["tasks"]):
        raise ValidationError("registered tasks contain duplicate case IDs")
    criteria = json.loads(criteria_path.read_text(encoding="utf-8"))
    if criteria.get("expected_trials") != len(expected_cases):
        raise ValidationError("criteria trial count differs from the registered task manifest")
    minimum_retry_exposure = criteria["minimum_guidance_authorized_retry_trials_per_arm"]
    if not isinstance(minimum_retry_exposure, int) or minimum_retry_exposure < 0:
        raise ValidationError("minimum retry exposure criterion must be a non-negative integer")
    incumbent = validate_arm(incumbent_path, expected_cases)
    candidate = validate_arm(candidate_path, expected_cases)
    if incumbent["arm"] != "incumbent" or candidate["arm"] != "candidate":
        raise ValidationError("artifact arms are mislabeled")
    return {
        "schema_version": "c1-bounded-read-recovery-validation/v1",
        "guidance_sha256": EMPTY_REFINEMENT_GUIDANCE_SHA256,
        "incumbent": incumbent,
        "candidate": candidate,
        "minimum_retry_exposure_met": (
            incumbent["guidance_authorized_retry_trials"] >= minimum_retry_exposure
            and candidate["guidance_authorized_retry_trials"] >= minimum_retry_exposure
        ),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--incumbent", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--tasks", type=Path, default=Path(__file__).with_name("tasks.json"))
    parser.add_argument("--criteria", type=Path, default=Path(__file__).with_name("criteria.json"))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    try:
        report = validate(args.incumbent, args.candidate, args.tasks, args.criteria)
        encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
        if args.output:
            args.output.write_text(encoded, encoding="utf-8")
        sys.stdout.write(encoded)
        if not report["minimum_retry_exposure_met"]:
            raise ValidationError("registered retry exposure threshold was not met")
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValidationError) as error:
        sys.stderr.write(f"C1 recovery validation failed: {error}\n")
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
