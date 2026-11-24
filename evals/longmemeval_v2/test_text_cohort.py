import unittest

from evals.longmemeval_v2.text_cohort import verified_span


class TextCohortEvidenceTests(unittest.TestCase):
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
