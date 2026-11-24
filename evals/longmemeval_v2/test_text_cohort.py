import hashlib
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from evals.longmemeval_v2.text_cohort import stage, verified_span


class TextCohortEvidenceTests(unittest.TestCase):
    def test_unreviewed_text_question_remains_visible_in_selection_denominator(self):
        with TemporaryDirectory() as directory:
            dataset = Path(directory)
            (dataset / "haystacks").mkdir()
            question = {"domain": "web", "question_type": "static-environment", "image": None}
            inputs = {
                "questions.jsonl": "\n".join(json.dumps({**question, "id": case})
                                            for case in ("reviewed", "unreviewed")),
                "haystacks/lme_v2_small.json": json.dumps({"reviewed": ["source"], "unreviewed": ["source"]}),
                "trajectories.jsonl": json.dumps({
                    "id": "source", "states": [{"state_index": 0, "accessibility_tree": "abc"}],
                }),
            }
            for name, content in inputs.items():
                (dataset / name).write_bytes(content.encode())
            pins = "\n".join(f"{hashlib.sha256(content.encode()).hexdigest()} {name}"
                             for name, content in inputs.items())
            manifest = {
                "schema": "memory-mcp-longmemeval-v2-text-cohort/v1",
                "domain": "web",
                "reviews": {"reviewed": {
                    "trajectory_id": "source", "state_index": 0, "start_line": 1, "end_line": 1,
                    "span_sha256": hashlib.sha256(b"abc").hexdigest(),
                }},
            }
            output = dataset / "output"
            with patch.object(Path, "read_text", return_value=pins):
                stage(dataset, manifest, output)
            selection = json.loads((output / "selection.json").read_text())
            self.assertEqual(selection["dataset_case_count"], 2)
            self.assertEqual(selection["selected_case_count"], 1)
            self.assertIn({**{key: question[key] for key in ("domain", "question_type")},
                           "reason": "unadjudicated", "count": 1}, selection["counts"])

    def test_modified_evidence_cannot_satisfy_the_frozen_manifest(self):
        trajectory = {
            "states": [{"state_index": 4, "accessibility_tree": "before\nchanged\nafter"}],
        }
        review = {
            "state_index": 4, "start_line": 2, "end_line": 2,
            "span_sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        }
        with self.assertRaisesRegex(ValueError, "digest does not match"):
            verified_span(trajectory, review)

    def test_span_indices_count_lines_inside_the_named_state(self):
        trajectory = {
            "states": [{"state_index": 4, "accessibility_tree": "before\nabc\nafter"}],
        }
        verified_span(trajectory, {
            "state_index": 4, "start_line": 2, "end_line": 2,
            "span_sha256": "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        })
