"""Run the pinned upstream scorer with separate, failure-inclusive coverage."""

import argparse
from collections import defaultdict
import json
import math
import os
from pathlib import Path
import time
from unittest.mock import patch
from urllib.parse import urlsplit


def failure_status(role, error):
    timeout = isinstance(error, TimeoutError) or type(error).__name__ == "APITimeoutError"
    return f"{role}_{'timeout' if timeout else 'error'}"


def run_observed(harness, argv, case_ids, output, preflight=None):
    failures = defaultdict(list)
    reader_cases = {}
    duration = {"reader": 0.0, "judge_scoring": 0.0}
    generate = harness.generate_all_reader_outputs
    reader = harness.call_reader_model_async
    score = harness.score_prediction

    async def observe_generation(args, rows):
        reader_cases.update((id(row["messages"]), row["question_id"]) for row in rows)
        return await generate(args, rows)

    async def observe_reader(client, args, messages):
        start = time.monotonic()
        try:
            return await reader(client, args, messages)
        except Exception as error:
            failures[reader_cases[id(messages)]].append(failure_status("reader", error))
            raise
        finally:
            duration["reader"] += time.monotonic() - start

    def observe_score(row, config):
        judge = row["eval_name"] in harness.LLM_EVAL_FUNCTIONS
        start = time.monotonic()
        try:
            return score(row, config)
        except Exception as error:
            role = "judge" if judge else "scoring"
            failures[row["question_id"]].append(failure_status(role, error))
            raise
        finally:
            if judge:
                duration["judge_scoring"] += time.monotonic() - start

    start = time.monotonic()
    local_env = {
        name: os.environ.get(name) or "EMPTY"
        for name in ("LME_LOCAL_READER_TOKEN", "LME_LOCAL_JUDGE_TOKEN")
    }
    try:
        if preflight is not None:
            try:
                preflight()
            except Exception as error:
                for case_id in case_ids:
                    failures[case_id].append("preflight_error")
                raise RuntimeError("LongMemEval scoring preflight failed") from error
        with (
            patch("sys.argv", argv),
            patch.dict(os.environ, {"NO_PROXY": "*", "no_proxy": "*", **local_env}),
            patch.object(harness, "generate_all_reader_outputs", observe_generation),
            patch.object(harness, "call_reader_model_async", observe_reader),
            patch.object(harness, "score_prediction", observe_score),
        ):
            harness.main()
    finally:
        cases = write_coverage(case_ids, failures, output, time.monotonic() - start, duration)
    return int(any(status != "scored" for status in cases.values()))


def write_coverage(case_ids, failures, output, elapsed, duration):
    native = {}
    per_question = output / "per_question.jsonl"
    if per_question.is_file():
        for line in per_question.read_text(encoding="utf-8").splitlines():
            row = json.loads(line)
            case_id = row["question_id"]
            if case_id not in case_ids or case_id in native:
                raise ValueError("native per-question output does not match the selected cohort")
            native[case_id] = row["score"]

    metrics = output / "aggregated_metrics.json"
    verified = None
    if metrics.is_file():
        overall = json.loads(metrics.read_text(encoding="utf-8"))["overall"]
        verified = (
            bool(native) and overall["count_all_questions"] == len(native)
            and math.isclose(overall["overall_full_set"], sum(native.values()) / len(native))
        )

    cases = {
        case: failures[case][0] if case in failures else ("scored" if case in native else "unscored")
        for case in case_ids
    }
    covered = [case for case, status in cases.items() if status == "scored"]
    report = {
        "schema": "memory-mcp-longmemeval-v2-score-coverage/v1",
        "selected_case_count": len(case_ids),
        "native_scored_case_count": len(native),
        "covered_case_count": len(covered),
        "coverage_adjusted_success_rate": sum(native[case] for case in covered) / len(case_ids),
        "official_metrics_status": "emitted" if metrics.is_file() else "unavailable",
        "official_overall_recomputed_matches": verified,
        "elapsed_seconds": round(elapsed, 3),
        "stage_seconds": {stage: round(value, 3) for stage, value in duration.items()},
        "cases": cases,
        "failures": failures,
    }
    (output / "score-coverage.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    if verified is False:
        raise ValueError("native aggregate does not match the case-level scores")
    return cases


def loopback_endpoint(value):
    url = urlsplit(value)
    if (url.scheme != "http" or url.hostname not in {"127.0.0.1", "::1"}
            or url.username is not None or url.query or url.fragment):
        raise argparse.ArgumentTypeError("use an explicit loopback HTTP endpoint")
    return value


def main():
    os.environ.update({"HF_HUB_OFFLINE": "1", "TRANSFORMERS_OFFLINE": "1"})
    from evaluation import harness
    from .memory_mcp_backend import MemoryMcpBackend  # registers with upstream

    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("questions", "haystack", "trajectories", "memory-state", "output"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    parser.add_argument("--domain", required=True, choices=("web", "enterprise"))
    parser.add_argument("--prompt-workers", type=int, default=1)
    for role in ("reader", "judge"):
        parser.add_argument(f"--{role}-url", required=True, type=loopback_endpoint)
        parser.add_argument(f"--{role}-model", required=True)
    args = parser.parse_args()
    if args.prompt_workers < 1:
        parser.error("--prompt-workers must be positive")
    questions = harness.load_questions(str(args.questions))
    case_ids = [row["id"] for row in questions]
    if not case_ids or len(set(case_ids)) != len(case_ids):
        raise ValueError("expected a nonempty cohort with unique IDs")
    if any(row.get("image") is not None or not isinstance(row["question"], str)
           or row.get("domain", args.domain) != args.domain for row in questions):
        raise ValueError("scored runner expects text-only questions in the selected domain")
    mapping = harness.load_haystack_mapping(str(args.haystack))
    harness.validate_question_and_haystack_ids(case_ids, mapping)
    build = json.loads((args.memory_state.parent / "build-coverage.json").read_text(encoding="utf-8"))
    if build["build_status"] != "saved" or any(mapping[case] != build["ordered_haystack_ids"] for case in case_ids):
        raise ValueError("cohort haystack does not match the saved build")
    args.output.mkdir()
    return run_observed(harness, [
        "harness.py", "--domain", args.domain, "--questions-path", str(args.questions),
        "--haystack-path", str(args.haystack), "--trajectories-path", str(args.trajectories),
        "--load-memory-dir", str(args.memory_state), "--output-dir", str(args.output),
        "--base-url", args.reader_url, "--model", args.reader_model,
        "--evaluator-base-url", args.judge_url, "--evaluator-model", args.judge_model,
        "--api-key-env", "LME_LOCAL_READER_TOKEN", "--evaluator-api-key-env", "LME_LOCAL_JUDGE_TOKEN",
        "--reader-max-concurrent-requests", "1", "--memory-context-max-tokens", "4096",
        "--prompt-build-max-workers", str(args.prompt_workers),
        "--temperature", "0", "--max-completion-tokens", "256", "--timeout-seconds", "30",
        "--evaluator-max-completion-tokens", "256", "--evaluator-timeout-seconds", "30",
    ], case_ids, args.output, preflight=harness.get_memory_context_processor)


if __name__ == "__main__":
    raise SystemExit(main())
