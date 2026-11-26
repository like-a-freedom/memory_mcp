"""Score exact reviewed-source presence in saved LongMemEval reader contexts."""

import argparse
import json
from pathlib import Path
import re
import sys

from .text_cohort import verified_span


_ESCAPED_TEXT = re.compile(r"(?<!\\)\\([tnr])")
_ESCAPES = {"t": "\t", "n": "\n", "r": "\r"}


def decode_saved_text(value):
    """Decode the tab/newline escapes used in saved text-context artifacts."""
    return _ESCAPED_TEXT.sub(lambda match: _ESCAPES[match.group(1)], value)


def score_reviewed_spans(cohort, trajectories, per_question):
    if cohort.get("schema") != "memory-mcp-longmemeval-v2-text-cohort/v1":
        raise ValueError("unsupported text cohort schema")
    reviews = cohort.get("reviews")
    if not isinstance(reviews, dict) or not reviews:
        raise ValueError("cohort must contain a nonempty reviews mapping")

    trajectory_by_id = {}
    for trajectory in trajectories:
        trajectory_id = trajectory.get("id")
        if not isinstance(trajectory_id, str) or trajectory_id in trajectory_by_id:
            raise ValueError("trajectory IDs must be unique strings")
        trajectory_by_id[trajectory_id] = trajectory

    output_by_id = {}
    for row in per_question:
        case_id = row.get("question_id")
        if not isinstance(case_id, str) or case_id in output_by_id:
            raise ValueError("scored question IDs must be unique strings")
        output_by_id[case_id] = row

    if set(output_by_id) != set(reviews):
        raise ValueError("scored output and reviewed cohort case IDs do not match")

    cases = {}
    for case_id, review in reviews.items():
        trajectory_id = review.get("trajectory_id")
        if trajectory_id not in trajectory_by_id:
            raise ValueError(f"reviewed trajectory is missing for {case_id}")
        span = verified_span(trajectory_by_id[trajectory_id], review)
        memory_context = output_by_id[case_id].get("memory_context")
        if not isinstance(memory_context, list):
            raise ValueError(f"memory context is missing or malformed for {case_id}")
        text_items = [
            item["value"]
            for item in memory_context
            if isinstance(item, dict)
            and item.get("type") == "text"
            and isinstance(item.get("value"), str)
        ]
        # ordered-segments-v1 stores source trees inside JSON strings. Match the
        # complete literal serialization, rather than partially decoding escapes
        # (which leaves protected backslashes and quoted labels incomparable).
        # Excerpts need not be complete JSON documents. This is source presence,
        # not a claim of decoded readability or correct interpretation.
        serialized_span = json.dumps(span, ensure_ascii=False)[1:-1]
        representations = [
            {
                name for name, present in (
                    ("raw_text", span in item),
                    ("legacy_saved_text", span in decode_saved_text(item)),
                    ("json_string_serialized", serialized_span in item),
                ) if present
            }
            for item in text_items
        ]
        matches = sum(bool(forms) for forms in representations)
        cases[case_id] = {
            "trajectory_id": trajectory_id,
            "reviewed_span_sha256": review["span_sha256"],
            "present": matches > 0,
            "matching_text_items": matches,
            "matching_representations": sorted(set().union(*representations)),
        }

    present = sum(result["present"] for result in cases.values())
    selected = len(cases)
    return {
        "schema": "memory-mcp-longmemeval-v2-reviewed-span-coverage/v2",
        "comparison_contract": "complete verified span in one text item: raw, legacy newline/tab escapes, or exact JSON-string serialization; not decoded readability or semantic sufficiency",
        "reviewed_span_coverage": {
            "present": present,
            "selected": selected,
            "rate": present / selected,
        },
        "cases": cases,
    }


def read_jsonl(path):
    return [json.loads(line) for line in path.read_text(encoding="utf-8").splitlines() if line]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cohort", type=Path, required=True)
    parser.add_argument("--trajectories", type=Path, required=True)
    parser.add_argument("--per-question", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    report = score_reviewed_spans(
        json.loads(args.cohort.read_text(encoding="utf-8")),
        json.loads(args.trajectories.read_text(encoding="utf-8")),
        read_jsonl(args.per_question),
    )
    rendered = json.dumps(report, indent=2, ensure_ascii=False) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    else:
        sys.stdout.write(rendered)


if __name__ == "__main__":
    main()
