"""Public diagnostic CLI contracts; all source/reader values are controlled."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


class LongHistoryDiagnosisCliTests(unittest.TestCase):
    def run_diagnostic(self, mutate=lambda inputs: None):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            span = "Tabs: Forums, Tags, Alphabetical"
            digest = hashlib.sha256(span.encode()).hexdigest()
            inputs = {
                'cohort': {'schema': 'memory-mcp-longmemeval-v2-text-cohort/v1', 'reviews': {'case': {'trajectory_id': 'trajectory', 'state_index': 0, 'start_line': 1, 'end_line': 1, 'span_sha256': digest}}},
                'trajectories': [{'id': 'trajectory', 'states': [{'state_index': 0, 'accessibility_tree': span}]}],
                'truth': {'schema': 'memory-mcp-long-history-truth/v1', 'cases': {'case': {'reviewed_span_sha256': digest, 'answer_kind': 'closed_list', 'accepted_answers': ['Forums, Tags, Alphabetical'], 'support_markers': ['Forums', 'Tags', 'Alphabetical']}}},
                'prompts': [{'question_id': 'case', 'memory_context': [{'type': 'text', 'value': span}], 'memory_context_token_count': 8, 'memory_context_was_truncated': False, 'memory_query_duration_seconds': 3.5}],
                'readers': [{'question_id': 'case', 'response_parsed_boxed': 'Forums, Tags, Alphabetical, Search'}],
            }
            inputs['readers'][0]['memory_context_sha256'] = hashlib.sha256(json.dumps(inputs['prompts'][0]['memory_context'], ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
            mutate(inputs)
            for name, value in inputs.items():
                (root / name).write_text(json.dumps(value) if name not in ('prompts', 'readers') else '\n'.join(json.dumps(x) for x in value))
            result = subprocess.run([sys.executable, '-m', 'evals.longmemeval_v2.long_history_diagnosis', *[arg for name in inputs for arg in ('--' + name, str(root / name))]], capture_output=True, text=True)
            return result

    def test_complete_source_delivered_but_wrong_reader_is_localized(self):
        result = self.run_diagnostic()
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        case = report['cases']['case']
        self.assertEqual(case['reader_verdict'], 'incorrect')
        self.assertEqual(case['failure_localization'], 'reader_use_with_complete_reviewed_evidence')
        self.assertEqual(case['formation'], 'unknown_no_intermediate_receipt')
        self.assertEqual(case['evidence_selection'], 'unknown_no_intermediate_receipt')
        self.assertEqual(case['packaging'], 'unknown_no_prepackaging_receipt')
        self.assertEqual(report['latency_seconds']['median'], 3.5)
        self.assertEqual(len(report['input_sha256']['prompts']), 64)

    def test_mismatched_reader_context_receipt_is_rejected(self):
        result = self.run_diagnostic(lambda inputs: inputs['readers'][0].update(memory_context_sha256='0' * 64))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('reader context digest mismatch', result.stderr)

    def test_correct_reordered_list_is_accepted(self):
        result = self.run_diagnostic(lambda inputs: inputs['readers'][0].update(response_parsed_boxed='Alphabetical; Forums; Tags'))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['cases']['case']['reader_verdict'], 'correct')

    def test_missing_reader_remains_unknown(self):
        result = self.run_diagnostic(lambda inputs: inputs.update(readers=[]))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['cases']['case']['failure_localization'], 'unknown_missing_reader')

    def test_all_markers_without_complete_span_do_not_localize_stage(self):
        def shortened(inputs):
            context = [{'type': 'text', 'value': 'Forums. Tags. Alphabetical.'}]
            inputs['prompts'][0]['memory_context'] = context
            inputs['readers'][0]['memory_context_sha256'] = hashlib.sha256(json.dumps(context, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        result = self.run_diagnostic(shortened)
        self.assertEqual(result.returncode, 0, result.stderr)
        case = json.loads(result.stdout)['cases']['case']
        self.assertFalse(case['delivered_complete_reviewed_span'])
        self.assertEqual(case['delivered_support_markers'], {'Forums': True, 'Tags': True, 'Alphabetical': True})
        self.assertEqual(case['failure_localization'], 'end_to_end_failure_stage_unknown')

    def test_modified_source_is_rejected(self):
        result = self.run_diagnostic(lambda inputs: inputs['trajectories'][0]['states'][0].update(accessibility_tree='Tabs: Search'))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('evidence span digest does not match', result.stderr)

    def test_empty_answer_is_failed_observation(self):
        result = self.run_diagnostic(lambda inputs: inputs['readers'][0].update(response_parsed_boxed=''))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['cases']['case']['reader_verdict'], 'incorrect')

    def test_invalid_latency_is_rejected(self):
        result = self.run_diagnostic(lambda inputs: inputs['prompts'][0].update(memory_query_duration_seconds=float('nan')))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('query latency must be finite and nonnegative', result.stderr)

    def test_reader_without_context_link_cannot_localize_use_failure(self):
        result = self.run_diagnostic(lambda inputs: inputs['readers'][0].pop('memory_context_sha256'))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['cases']['case']['failure_localization'], 'end_to_end_failure_stage_unknown')


if __name__ == '__main__':
    unittest.main()
