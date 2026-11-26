"""Integration coverage of the public saved-evidence scoring CLI."""

import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class ReviewedSpanCliTests(unittest.TestCase):
    source_span = '\t[7] button \'Print "Draft" ' + r'\u2602' + "'\n\t[8] button 'Close'"

    def score_context(self, text_items):
        span = self.source_span
        trajectory = {"id": "independent-toolbar", "states": [{
            "state_index": 4, "accessibility_tree": span,
        }]}
        cohort = {"schema": "memory-mcp-longmemeval-v2-text-cohort/v1", "reviews": {
            "source-case": {"trajectory_id": "independent-toolbar", "state_index": 4,
                            "start_line": 1, "end_line": 2,
                            "span_sha256": hashlib.sha256(span.encode()).hexdigest()},
        }}
        rows = [{"question_id": "source-case", "memory_context": [
            {"type": "text", "value": value} for value in text_items
        ]}]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "cohort.json").write_text(json.dumps(cohort))
            (root / "trajectories.json").write_text(json.dumps([trajectory]))
            (root / "rows.jsonl").write_text(json.dumps(rows[0]) + "\n")
            result = subprocess.run([
                sys.executable, "-m", "evals.longmemeval_v2.reviewed_span_coverage",
                "--cohort", str(root / "cohort.json"),
                "--trajectories", str(root / "trajectories.json"),
                "--per-question", str(root / "rows.jsonl"),
            ], capture_output=True, text=True, timeout=10, check=True)
        return json.loads(result.stdout)

    def test_complete_serialized_source_with_literal_backslash_keeps_coverage(self):
        # The episode serializer stores a JSON string inside the source payload.
        # The expected coverage comes from both complete literal source lines.
        report = self.score_context([json.dumps(self.source_span, ensure_ascii=False)[1:-1]])
        self.assertEqual(report["reviewed_span_coverage"], {
            "present": 1, "selected": 1, "rate": 1.0,
        })
        self.assertEqual(report["cases"]["source-case"]["matching_representations"],
                         ["json_string_serialized"])

    def test_truncated_serialized_source_remains_absent(self):
        text = json.dumps(self.source_span, ensure_ascii=False)[1:-1]
        report = self.score_context([text[:-1]])
        self.assertEqual(report["reviewed_span_coverage"]["present"], 0)

    def test_serialized_source_split_between_items_remains_absent(self):
        text = json.dumps(self.source_span, ensure_ascii=False)[1:-1]
        report = self.score_context([text[:20], text[20:]])
        self.assertEqual(report["reviewed_span_coverage"]["present"], 0)

    def test_changed_literal_escape_remains_absent(self):
        different_source = self.source_span.replace(r"\u2602", r"\u2603")
        report = self.score_context([json.dumps(different_source, ensure_ascii=False)[1:-1]])
        self.assertEqual(report["reviewed_span_coverage"]["present"], 0)

    def test_changed_quoted_name_remains_absent(self):
        different_source = self.source_span.replace('"Draft"', '"Final"')
        report = self.score_context([json.dumps(different_source, ensure_ascii=False)[1:-1]])
        self.assertEqual(report["reviewed_span_coverage"]["present"], 0)

    def test_additional_json_escaping_is_not_recursively_decoded(self):
        text = json.dumps(self.source_span, ensure_ascii=False)[1:-1]
        report = self.score_context([json.dumps(text, ensure_ascii=False)[1:-1]])
        self.assertEqual(report["reviewed_span_coverage"]["present"], 0)


if __name__ == "__main__":
    unittest.main()
