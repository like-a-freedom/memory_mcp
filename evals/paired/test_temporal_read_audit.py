"""Public artifact-audit CLI regression, using missing scheduled observations."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name('temporal_read_audit.py')


class AuditCaptureCli(unittest.TestCase):
    def run_fixture(self, *, bad_selector=False, blank_query=False, optimize=False, late=False, excess_turns=False, excess_output=False):
        import hashlib
        digest = lambda value: hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',', ':')).encode()).hexdigest()
        fields = ['fact_types', 'as_of', 'valid_at', 'known_at', 'known_revision', 'budget', 'view_mode', 'window_start', 'window_end', 'compact']
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory); traces = root/'traces'; traces.mkdir()
            tasks = []
            for i in range(4):
                key = {'stratum_id': 'ordinary', 'family_id': str(i), 'case_id': str(i), 'repetition': 1, 'incumbent_first': True}
                tasks.append({'key': key, 'oracle': {'read_expectation': {'selector_arguments': {'budget': 8, 'compact': True}}}})
            (root/'tasks.json').write_text(json.dumps({'tasks': tasks}))
            for arm in ['incumbent', 'candidate']:
                receipts = []; probes = []
                for i, task in enumerate(tasks):
                    arguments = {'query': '  ' if blank_query and arm == 'incumbent' and i == 0 else 'source location', 'budget': 8, 'compact': True}
                    response = {'result': {'structuredContent': {'status': 'success', 'result': [{'content': 'source location', 'source_episode': 'ep'}]}}}
                    read = {'request_sequence': 1, 'outcome': 'nonempty', 'item_count': 1, 'selector_expectation_registered': True,
                            'selector_argument_matches': dict.fromkeys(fields, True), 'source_coverage': True,
                            'request_query_sha256': digest(arguments['query']), 'non_query_arguments_sha256': digest({k:v for k,v in arguments.items() if k != 'query'})}
                    if bad_selector and arm == 'incumbent' and i == 0:read['selector_argument_matches']['known_at'] = False
                    receipts.append({'key': task['key'], 'status': 'completed', 'task_oracle_passed': True, 'direct_memory_success': True,
                                     'hard_violations': [], 'read_capture_status': 'observed', 'tool_calls': 1, 'read_diagnostics': [read],
                                     'wall_time_ms': 25001 if late and arm == 'incumbent' and i == 0 else 1000,
                                     'turns_used': 4 if excess_turns and arm == 'incumbent' and i == 0 else 2,
                                     'output_tokens': 385 if excess_output and arm == 'incumbent' and i == 0 else 100})
                    probes.append({'key': task['key'], 'score': {'evidence_success': True}})
                    trace = {'schema_version': 'memory-mcp-public-synthetic-trace/v1', 'identity': {'arm': arm, 'key': task['key'], 'phase': 'recall', 'product_binary_sha256': '0'*64},
                             'calls': [{'tool': 'assemble_context', 'arguments': arguments, 'arguments_sha256': digest(arguments), 'response': response, 'response_sha256': digest(response), 'call_failed': False}]}
                    (traces/(arm+str(i)+'.json')).write_text(json.dumps(trace))
                artifact = {'agent_observed': {'arm': arm, 'identity': {'product': {'binary_sha256': '0'*64}}, 'receipts': receipts}, 'independent_memory': probes}
                (root/('independent-'+arm+'.json')).write_text(json.dumps(artifact))
            output = subprocess.run([sys.executable, *(['-O'] if optimize else []), str(SCRIPT), '--tasks', str(root/'tasks.json'), '--capture', str(root), '--traces', str(traces)], capture_output=True, text=True, timeout=5)
            return output.returncode, json.loads(output.stdout)

    def test_incorrect_incumbent_selector_rejects_passing_candidate(self):
        code, report = self.run_fixture(bad_selector=True)
        self.assertEqual(code, 1)
        self.assertEqual(report['passing_arm_trials'], 7)

    def test_blank_model_query_rejects_even_when_selector_diagnostics_pass(self):
        code, report = self.run_fixture(blank_query=True)
        self.assertEqual(code, 1)
        self.assertEqual(report['passing_arm_trials'], 7)

    def test_optimized_interpreter_still_rejects_incorrect_incumbent_selector(self):
        code, report = self.run_fixture(bad_selector=True, optimize=True)
        self.assertEqual(code, 1)
        self.assertEqual(report['passing_arm_trials'], 7)

    def test_empty_task_manifest_cannot_qualify(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/'tasks.json').write_text('{"tasks": []}')
            output = subprocess.run([sys.executable, str(SCRIPT), '--tasks', str(root/'tasks.json'),
                '--capture', str(root), '--traces', str(root)], capture_output=True, text=True, timeout=5)
            self.assertEqual(output.returncode, 1)
            self.assertFalse(json.loads(output.stdout)['eligible'])

    def test_late_completed_trial_rejects_despite_success_status(self):
        code, report = self.run_fixture(late=True)
        self.assertEqual(code, 1)
        self.assertEqual(report['passing_arm_trials'], 7)

    def test_optimized_interpreter_rejects_late_completed_trial(self):
        code, report = self.run_fixture(late=True, optimize=True)
        self.assertEqual(code, 1)
        self.assertEqual(report['passing_arm_trials'], 7)

    def test_extra_turn_rejects_completed_trial(self):
        code, report = self.run_fixture(excess_turns=True)
        self.assertEqual(code, 1)
        self.assertEqual(report['passing_arm_trials'], 7)

    def test_output_over_budget_rejects_completed_trial(self):
        code, report = self.run_fixture(excess_output=True)
        self.assertEqual(code, 1)
        self.assertEqual(report['passing_arm_trials'], 7)

    def test_two_complete_correct_arms_pass(self):
        code, report = self.run_fixture()
        self.assertEqual(code, 0)
        self.assertEqual(report['passing_arm_trials'], 8)

    def test_missing_both_arm_observations_keep_scheduled_denominator_and_reject(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root/'tasks.json').write_text(json.dumps({'tasks': [
                {'key': {'case_id': str(i)}} for i in range(4)]}))
            output = subprocess.run([sys.executable, str(SCRIPT), '--tasks', str(root/'tasks.json'),
                '--capture', str(root), '--traces', str(root)], capture_output=True, text=True, timeout=5)
            self.assertEqual(output.returncode, 1, output.stdout + output.stderr)
            report = json.loads(output.stdout)
            self.assertFalse(report['eligible'])
            self.assertEqual(report['scheduled_arm_trials'], 8)
            self.assertEqual(report['passing_arm_trials'], 0)


if __name__ == '__main__':
    unittest.main()
