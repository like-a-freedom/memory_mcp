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
AUTHORIZED_PARTIAL_GUIDANCE_SHA256S = {
    hashlib.sha256(text.encode("utf-8")).hexdigest()
    for text in (TRUNCATED_PARTIAL_GUIDANCE, OMITTED_CHANNELS_PARTIAL_GUIDANCE)
}
SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


class ValidationError(ValueError):
    pass


def validate_arm(artifact_path: Path, expected_cases: set[str]) -> dict[str, Any]:
    artifact = json.loads(artifact_path.read_text(encoding="utf-8"))
    if not isinstance(artifact, dict):
        raise ValidationError(f"{artifact_path}: artifact must be an object")
    if artifact.get("schema_version") != ARTIFACT_SCHEMA:
        raise ValidationError(f"{artifact_path}: requires {ARTIFACT_SCHEMA}")

    receipts = artifact.get("receipts")
    if not isinstance(receipts, list):
        raise ValidationError(f"{artifact_path}: receipts must be an array")
    actual_cases: set[str] = set()
    total_reads = 0
    retry_trials = 0
    retry_contract_violations: list[str] = []
    for receipt in receipts:
        if not isinstance(receipt, dict):
            raise ValidationError(f"{artifact_path}: receipt must be an object")
        key = receipt.get("key", {})
        if not isinstance(key, dict):
            raise ValidationError(f"{artifact_path}: receipt key must be an object")
        case_id = key.get("case_id")
        if not isinstance(case_id, str) or case_id in actual_cases:
            raise ValidationError(f"{artifact_path}: invalid or duplicate case_id")
        actual_cases.add(case_id)
        if receipt.get("read_capture_status") != "observed":
            raise ValidationError(f"{artifact_path}:{case_id}: read capture is not complete")
        reads = receipt.get("read_diagnostics")
        if not isinstance(reads, list):
            raise ValidationError(f"{artifact_path}:{case_id}: missing read diagnostics")
        if not reads:
            raise ValidationError(f"{artifact_path}:{case_id}: at least one read must be observed")
        total_reads += len(reads)

        for read in reads:
            if not isinstance(read, dict):
                raise ValidationError(f"{artifact_path}:{case_id}: read diagnostic must be an object")
            for name in ("request_query_sha256", "non_query_arguments_sha256"):
                digest = read.get(name)
                if not isinstance(digest, str) or not SHA256_RE.fullmatch(digest):
                    raise ValidationError(f"{artifact_path}:{case_id}: missing or invalid {name}")
            guidance = read.get("response_guidance_sha256")
            if guidance is not None and (
                not isinstance(guidance, str) or not SHA256_RE.fullmatch(guidance)
            ):
                raise ValidationError(f"{artifact_path}:{case_id}: invalid guidance digest")

        if len(reads) > 2:
            retry_contract_violations.append(f"{case_id}: more than one retry")
        if len(reads) >= 2:
            first, retry = reads[:2]
            violations_before = len(retry_contract_violations)
            if first.get("outcome") != "empty" or first.get("completeness") != "partial":
                retry_contract_violations.append(
                    f"{case_id}: retry did not follow an empty partial read"
                )
            if first.get("response_guidance_sha256") not in AUTHORIZED_PARTIAL_GUIDANCE_SHA256S:
                retry_contract_violations.append(
                    f"{case_id}: retry lacked exact registered partial-read guidance"
                )
            if first.get("request_query_sha256") == retry.get("request_query_sha256"):
                retry_contract_violations.append(f"{case_id}: retry did not change the query")
            if first.get("non_query_arguments_sha256") != retry.get("non_query_arguments_sha256"):
                retry_contract_violations.append(
                    f"{case_id}: retry changed non-query arguments"
                )
            if len(retry_contract_violations) == violations_before:
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
        "retry_contract_violations": retry_contract_violations,
    }


def validate(
    incumbent_path: Path,
    candidate_path: Path,
    tasks_path: Path,
    criteria_path: Path,
) -> dict[str, Any]:
    tasks = json.loads(tasks_path.read_text(encoding="utf-8"))
    if tasks.get("schema_version") != "memory-mcp-agent-managed-tasks/v1":
        raise ValidationError("registered task manifest schema is unsupported")
    expected_cases = {task["key"]["case_id"] for task in tasks["tasks"]}
    if len(expected_cases) != len(tasks["tasks"]):
        raise ValidationError("registered tasks contain duplicate case IDs")
    criteria = json.loads(criteria_path.read_text(encoding="utf-8"))
    if criteria.get("schema_version") != "memory-mcp-agent-managed-screen-criteria/v2":
        raise ValidationError("registered criteria schema is unsupported")
    if criteria.get("expected_trials") != len(expected_cases):
        raise ValidationError("criteria trial count differs from the registered task manifest")
    if criteria.get("maximum_bounded_retry_contract_violations_per_arm") != 0:
        raise ValidationError("registered criteria must reject every bounded-retry violation")
    incumbent = validate_arm(incumbent_path, expected_cases)
    candidate = validate_arm(candidate_path, expected_cases)
    if incumbent["arm"] != "incumbent" or candidate["arm"] != "candidate":
        raise ValidationError("artifact arms are mislabeled")
    return {
        "schema_version": "c1-bounded-read-recovery-validation/v1",
        "authorized_partial_guidance_sha256s": sorted(AUTHORIZED_PARTIAL_GUIDANCE_SHA256S),
        "incumbent": incumbent,
        "candidate": candidate,
        "bounded_retry_contract_passed": not (
            incumbent["retry_contract_violations"]
            or candidate["retry_contract_violations"]
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
        if not report["bounded_retry_contract_passed"]:
            raise ValidationError("an observed bounded-retry contract violation was found")
    except (OSError, json.JSONDecodeError, KeyError, TypeError, ValidationError) as error:
        sys.stderr.write(f"C1 recovery validation failed: {error}\n")
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
