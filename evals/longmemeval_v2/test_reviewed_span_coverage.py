import hashlib
import unittest

from evals.longmemeval_v2.reviewed_span_coverage import decode_saved_text, score_reviewed_spans


class ReviewedSpanCoverageTests(unittest.TestCase):
    def test_rejects_an_unknown_cohort_schema(self):
        with self.assertRaisesRegex(ValueError, "unsupported text cohort schema"):
            score_reviewed_spans({"schema": "other", "reviews": {"case-a": {}}}, [], [])

    def test_does_not_decode_a_literal_escaped_backslash_sequence(self):
        self.assertEqual(decode_saved_text(r"literal\\n marker\nnext"), "literal\\\\n marker\nnext")

    def test_decodes_saved_text_escapes_and_verifies_the_source_span(self):
        span = "\t[1] button 'Create Order'\n\t[2] button 'Reset Password'"
        trajectory = {
            "id": "toolbar",
            "states": [{"state_index": 38, "accessibility_tree": f"before\n{span}\nafter"}],
        }
        cohort = {"schema": "memory-mcp-longmemeval-v2-text-cohort/v1", "reviews": {"case-a": {
            "trajectory_id": "toolbar", "state_index": 38,
            "start_line": 2, "end_line": 3,
            "span_sha256": hashlib.sha256(span.encode()).hexdigest(),
        }}}
        per_question = [{"question_id": "case-a", "memory_context": [{
            "type": "text", "value": "before\\n\\t[1] button 'Create Order'\\n\\t[2] button 'Reset Password'\\nafter",
        }]}]

        report = score_reviewed_spans(cohort, [trajectory], per_question)

        self.assertEqual(report["reviewed_span_coverage"], {
            "present": 1, "selected": 1, "rate": 1.0,
        })
        self.assertTrue(report["cases"]["case-a"]["present"])

    def test_does_not_join_different_context_items_to_fake_a_complete_span(self):
        span = "first line\nsecond line"
        trajectory = {
            "id": "source",
            "states": [{"state_index": 0, "accessibility_tree": span}],
        }
        cohort = {"schema": "memory-mcp-longmemeval-v2-text-cohort/v1", "reviews": {"case-a": {
            "trajectory_id": "source", "state_index": 0,
            "start_line": 1, "end_line": 2,
            "span_sha256": hashlib.sha256(span.encode()).hexdigest(),
        }}}
        per_question = [{"question_id": "case-a", "memory_context": [
            {"type": "text", "value": "first line"},
            {"type": "text", "value": "second line"},
        ]}]

        report = score_reviewed_spans(cohort, [trajectory], per_question)

        self.assertEqual(report["reviewed_span_coverage"], {
            "present": 0, "selected": 1, "rate": 0.0,
        })
        self.assertFalse(report["cases"]["case-a"]["present"])

    def test_missing_scored_case_is_an_error_instead_of_lowering_denominator_silently(self):
        cohort = {"schema": "memory-mcp-longmemeval-v2-text-cohort/v1", "reviews": {"case-a": {
            "trajectory_id": "source", "state_index": 0,
            "start_line": 1, "end_line": 1,
            "span_sha256": hashlib.sha256(b"fact").hexdigest(),
        }}}

        with self.assertRaisesRegex(ValueError, "case IDs do not match"):
            score_reviewed_spans(cohort, [], [])


if __name__ == "__main__":
    unittest.main()
