"""Stage a frozen, evidence-checked text cohort from the pinned Small files."""

import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path


def verified_span(trajectory, review):
    states = [state for state in trajectory["states"] if state["state_index"] == review["state_index"]]
    if len(states) != 1:
        raise ValueError("evidence state is missing or ambiguous")
    lines = states[0]["accessibility_tree"].splitlines()
    first, last = review["start_line"], review["end_line"]
    if not 1 <= first <= last <= len(lines):
        raise ValueError("evidence span is out of bounds")
    content = "\n".join(lines[first - 1:last]).encode("utf-8")
    if hashlib.sha256(content).hexdigest() != review["span_sha256"]:
        raise ValueError("evidence span digest does not match")


def stage(dataset, manifest, output):
    if manifest["schema"] != "memory-mcp-longmemeval-v2-text-cohort/v1":
        raise ValueError("unsupported text cohort manifest")
    pins = {
        name: digest
        for digest, name in (
            line.split() for line in Path(__file__).with_name("dataset-small.sha256").read_text().splitlines()
        )
    }

    def read_verified(name):
        payload = (dataset / name).read_bytes()
        if hashlib.sha256(payload).hexdigest() != pins[name]:
            raise ValueError(f"pinned dataset digest mismatch for {name}")
        return payload

    questions = [json.loads(line) for line in read_verified("questions.jsonl").splitlines()]
    mapping = json.loads(read_verified("haystacks/lme_v2_small.json"))
    if len({q["id"] for q in questions}) != len(questions) or {q["id"] for q in questions} != set(mapping):
        raise ValueError("question IDs do not match the pinned haystack mapping")
    selected = []
    reasons = Counter()
    reviews = manifest["reviews"]
    for question in questions:
        case = question["id"]
        if question["domain"] != manifest["domain"]:
            reason = "outside_domain"
        elif question["image"] is not None:
            reason = "question_image"
        elif case in reviews:
            selected.append(question)
            reason = "text_verified"
        else:
            reason = "unadjudicated"
        reasons[(question["domain"], question["question_type"], reason)] += 1
    if not selected or {q["id"] for q in selected} != set(reviews):
        raise ValueError("manifest includes missing or ineligible questions")

    ordered_ids = mapping[selected[0]["id"]]
    if not ordered_ids or any(mapping[q["id"]] != ordered_ids for q in selected):
        raise ValueError("selected questions must share the original ordered haystack")
    selected_ids = set(ordered_ids)
    trajectories = {}
    digest = hashlib.sha256()
    with (dataset / "trajectories.jsonl").open("rb") as stream:
        for line in stream:
            digest.update(line)
            row = json.loads(line)
            if row["id"] in selected_ids:
                trajectories[row["id"]] = row
    if digest.hexdigest() != pins["trajectories.jsonl"] or set(trajectories) != selected_ids:
        raise ValueError("pinned trajectory file or selected haystack mismatch")
    for review in reviews.values():
        verified_span(trajectories[review["trajectory_id"]], review)

    output.mkdir()
    artifacts = {
        "questions": selected,
        "haystack": {q["id"]: ordered_ids for q in selected},
        "trajectories": [trajectories[trajectory] for trajectory in ordered_ids],
        "selection": {
            "dataset_case_count": len(questions),
            "selected_case_count": len(selected),
            "ordered_haystack_count": len(ordered_ids),
            "counts": [
                {"domain": domain, "question_type": kind, "reason": reason, "count": count}
                for (domain, kind, reason), count in sorted(reasons.items())
            ],
        },
    }
    for name, contents in artifacts.items():
        (output / f"{name}.json").write_text(json.dumps(contents) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for key in ("dataset", "manifest", "output"):
        parser.add_argument(f"--{key}", type=Path, required=True)
    args = parser.parse_args()
    stage(args.dataset, json.loads(args.manifest.read_text(encoding="utf-8")), args.output)


if __name__ == "__main__":
    main()
