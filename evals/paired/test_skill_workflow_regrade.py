import json
from pathlib import Path
import runpy
from tempfile import TemporaryDirectory
import unittest

from evals.paired.skill_workflow import MCP_HOST, SOURCE, grade_observed
from evals.paired.skill_workflow_regrade import regrade_trial

ARCHIVED_SCORE = runpy.run_path(str(Path(__file__).with_name('skill-qualification-2026-11-27-dynamic-v3')
                                   / 'v3runner.source.py'))['grade_observed']


def recorded_capture(extract_arguments, terminal='answered'):
    episode = 'episode:observed'
    trace = [
        {'request': {'action': 'call', 'name': MCP_HOST + 'ingest', 'arguments': SOURCE},
         'response': {'status': 'success', 'result': episode}},
        {'request': {'action': 'call', 'name': MCP_HOST + 'extract', 'arguments': extract_arguments},
         'response': {'status': 'success', 'result': {'episode_id': episode,
                     'facts': [{'fact_id': 'fact:observed'}]}}},
    ]
    case = {'skill': 'memory-mcp', 'id': 1}
    result = {'terminal': terminal}
    trial = {**case, 'trace': trace, 'result': result, 'grade': ARCHIVED_SCORE(case, result, trace)}
    return case, trial


class OfflineRegradeTests(unittest.TestCase):
    def test_observed_operation_before_terminal_error_is_reported_but_row_stays_failed(self):
        case, trial = recorded_capture({'episode_id': 'episode:observed'}, 'error')
        self.assertFalse(trial['grade']['operation_and_exact_arguments'])

        regraded = regrade_trial(case, trial, ARCHIVED_SCORE)

        self.assertTrue(regraded['grade']['operation_and_exact_arguments'])
        self.assertEqual(regraded['terminal'], 'error')
        self.assertFalse(regraded['full_row_pass'])

    def test_explicit_documented_default_does_not_change_episode_binding(self):
        case, trial = recorded_capture({'episode_id': 'episode:observed', 'include_fact_evidence': True})

        regraded = regrade_trial(case, trial, ARCHIVED_SCORE)

        self.assertTrue(regraded['grade']['operation_and_exact_arguments'])
        self.assertFalse(trial['grade']['operation_and_exact_arguments'])
        self.assertTrue(trial['trace'][1]['request']['arguments']['include_fact_evidence'])
        self.assertIsNone(regraded['full_row_pass'])

    def test_other_values_unknown_fields_and_wrong_episode_still_fail(self):
        for arguments in (
            {'episode_id': 'episode:observed', 'include_fact_evidence': False},
            {'episode_id': 'episode:observed', 'include_fact_evidence': 1},
            {'episode_id': 'episode:observed', 'include_fact_evidence': True, 'unknown_field': 'value'},
            {'episode_id': 'episode:wrong', 'include_fact_evidence': True},
        ):
            with self.subTest(arguments=arguments):
                case, trial = recorded_capture(arguments)
                regraded = regrade_trial(case, trial, ARCHIVED_SCORE)
                self.assertFalse(regraded['grade']['operation_and_exact_arguments'])
                self.assertFalse(regraded['full_row_pass'])

    def test_unanswered_question_only_case_without_operations_cannot_gain_a_pass(self):
        case = {'skill': 'memory-mcp', 'id': 7}
        result = {'terminal': 'setup_error'}
        trial = {**case, 'trace': [], 'result': result, 'grade': grade_observed(case, result, [])}

        regraded = regrade_trial(case, trial, ARCHIVED_SCORE)

        self.assertFalse(regraded['grade']['operation_and_exact_arguments'])
        self.assertFalse(regraded['full_row_pass'])

    def test_partial_capture_cannot_be_published_as_fixed_denominator_regrade(self):
        from evals.paired.skill_workflow_regrade import regrade_capture

        with TemporaryDirectory() as work:
            capture = Path(work)
            (capture / 'summary.json').write_text(json.dumps({'scheduled': 2, 'rows': []}))

            with self.assertRaisesRegex(ValueError, 'complete scheduled capture'):
                regrade_capture({'cases': [{'skill': 'memory-mcp', 'id': 1}],
                                 'arms': ['previous', 'current']}, capture, grade_observed)


class DefaultGradeTests(unittest.TestCase):
    def test_valid_observed_operations_survive_terminal_answer_error(self):
        case, trial = recorded_capture({'episode_id': 'episode:observed'}, 'error')

        grade = grade_observed(case, trial['result'], trial['trace'])

        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertEqual(trial['result']['terminal'], 'error')

    def test_documented_default_true_is_equivalent_to_omission(self):
        case, trial = recorded_capture({'episode_id': 'episode:observed', 'include_fact_evidence': True})

        grade = grade_observed(case, trial['result'], trial['trace'])

        self.assertTrue(grade['operation_and_exact_arguments'])

    def test_nondefault_wrong_and_unknown_fields_remain_invalid(self):
        for arguments in (
            {'episode_id': 'episode:observed', 'include_fact_evidence': False},
            {'episode_id': 'episode:observed', 'include_fact_evidence': 'true'},
            {'episode_id': 'episode:wrong', 'include_fact_evidence': True},
            {'episode_id': 'episode:observed', 'include_fact_evidence': True, 'extra': 'x'},
        ):
            with self.subTest(arguments=arguments):
                case, trial = recorded_capture(arguments)

                grade = grade_observed(case, trial['result'], trial['trace'])

                self.assertFalse(grade['operation_and_exact_arguments'])
