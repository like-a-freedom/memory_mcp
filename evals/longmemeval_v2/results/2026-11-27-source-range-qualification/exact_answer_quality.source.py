"""Deterministic strict scoring for LongMemEval list-valued answers.

The pinned upstream ``norm_phrase_set_match`` checks that every gold phrase is
present in the prediction, but intentionally permits extra phrases. This
supplemental metric requires the normalized phrase sets to be equal.
"""

import argparse
import json
from pathlib import Path
import re
import sys


def _normalize_phrase(value):
    text = str(value).lower().replace("-", " ").replace("_", " ")
    text = re.sub(r"[,;]", " ", text)
    text = re.sub(r"[^\w\s]", "", text)
    return re.sub(r"\s+", " ", text).strip()


def _phrase_set(value):
    return {
        normalized
        for part in re.split(r",|;", value or "")
        if (normalized := _normalize_phrase(part))
    }


def exact_phrase_set_match(prediction, reference):
    """Return true only when both nonempty normalized phrase sets are equal."""
    predicted = _phrase_set(prediction)
    expected = _phrase_set(reference)
    return bool(predicted and expected and predicted == expected)


def score_exact_phrase_rows(rows):
    """Score list-valued cases; count incomplete selected rows as incorrect."""
    cases = {}
    for row in rows:
        eval_function = row.get("eval_function")
        if not isinstance(eval_function, str):
            raise ValueError("each row must contain an eval_function string")
        if eval_function.split("|", 1)[0] != "norm_phrase_set_match":
            continue

        question_id = row.get("question_id")
        if not isinstance(question_id, str) or question_id in cases:
            raise ValueError("selected question IDs must be unique strings")
        status = row.get("status", "scored")
        correct = status == "scored" and exact_phrase_set_match(
            row.get("response_parsed_boxed", ""), row.get("answer_gold", "")
        )
        cases[question_id] = {"status": status, "score": correct}

    selected = len(cases)
    correct = sum(result["score"] for result in cases.values())
    return {
        "schema": "memory-mcp-longmemeval-v2-exact-phrase-set-accuracy/v1",
        "metric": "strict normalized phrase-set equality; extra phrases fail",
        "selected": selected,
        "correct": correct,
        "failure_inclusive_accuracy": correct / selected if selected else None,
        "cases": cases,
    }


def read_jsonl(path):
    return [
        json.loads(line)
        for line in path.read_text(encoding="utf-8").splitlines()
        if line
    ]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--per-question", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    rendered = json.dumps(score_exact_phrase_rows(read_jsonl(args.per_question)), indent=2) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    else:
        sys.stdout.write(rendered)


if __name__ == "__main__":
    main()
