"""Account for native shared-store builds without invoking the official scorer."""

import argparse
import json
from pathlib import Path
from subprocess import CalledProcessError
import sys

from .memory_mcp_backend import FORMATION_METHODS, WHOLE_TRAJECTORY, MemoryMcpBackend


def run_shared_build(
    *, domain: str, questions: Path, haystack: Path, trajectories: Path,
    binary: Path, output: Path, formation_method: str = WHOLE_TRAJECTORY,
) -> int:
    from evaluation.harness import (
        all_haystacks_shared,
        load_haystack_mapping,
        load_questions,
        main,
        validate_question_and_haystack_ids,
    )

    if formation_method not in FORMATION_METHODS:
        raise ValueError("unknown LongMemEval-V2 formation method")

    ids = [item.get("id") for item in load_questions(str(questions))]
    if not ids or any(not isinstance(case, str) or not case for case in ids):
        raise ValueError("expected at least one question with a valid ID")
    if len(set(ids)) != len(ids):
        raise ValueError("duplicate question ID")
    mapping = load_haystack_mapping(str(haystack))
    validate_question_and_haystack_ids(ids, mapping)
    if not mapping[ids[0]] or not all_haystacks_shared(ids, mapping):
        raise ValueError("expected one shared nonempty ordered haystack")

    executable = str(binary.resolve(strict=True))
    output = output.resolve()
    output.mkdir()
    config = output / "memory-config.json"
    config.write_text(json.dumps({
        "memory_type": MemoryMcpBackend.memory_type,
        "memory_params": {
            "binary": executable, "db_path": str(output / "working-store"),
            "formation_method": formation_method,
        },
    }), encoding="utf-8")
    argv = [
        "harness.py", "--domain", domain, "--output-dir", str(output),
        "--questions-path", str(questions), "--haystack-path", str(haystack),
        "--trajectories-path", str(trajectories), "--memory-config-path", str(config),
        "--save-memory", "--skip-evaluation",
    ]
    failed_step = None
    original_argv = sys.argv
    try:
        sys.argv = argv
        main()
    except CalledProcessError as error:
        cmd = error.cmd
        if not isinstance(cmd, list) or len(cmd) < 2:
            raise
        if cmd[0] != executable or cmd[1] not in {"ingest", "extract"}:
            raise
        failed_step = cmd[1]
    finally:
        sys.argv = original_argv

    report = {
        "schema": "memory-mcp-longmemeval-v2-build-preflight/v1",
        "formation_method": formation_method,
        "selected_case_ids": ids,
        "selected_case_count": len(ids),
        "affected_case_ids": ids if failed_step else [],
        "build_status": "failed" if failed_step else "saved",
        "failed_step": failed_step,
        "retry_attempted": False,
        "scored_case_count": 0,
        "official_metrics_status": "unavailable",
    }
    (output / "build-coverage.json").write_text(
        json.dumps(report, indent=2) + "\n", encoding="utf-8",
    )
    return int(failed_step is not None)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--domain", required=True)
    parser.add_argument("--formation-method", choices=FORMATION_METHODS, default=WHOLE_TRAJECTORY)
    for name in ("questions", "haystack", "trajectories", "binary", "output"):
        parser.add_argument(f"--{name}", required=True, type=Path)
    raise SystemExit(run_shared_build(**vars(parser.parse_args())))
