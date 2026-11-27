import unittest
import json
import sys
from contextlib import contextmanager
from pathlib import Path
from tempfile import TemporaryDirectory
from unittest.mock import patch

from evals.paired.skill_workflow import SkillHost


class SkillHostTests(unittest.TestCase):
    def test_reference_loading_denies_unregistered_paths_and_records_attempt(self):
        host = SkillHost('memory-mcp', {'references/mcp-tools.md': 'tool documentation'}, {})

        denial = host.dispatch({'action': 'read_reference', 'path': '../AGENTS.md'})

        self.assertIn('error', denial)
        self.assertEqual(host.trace[0]['request']['path'], '../AGENTS.md')
        self.assertEqual(host.trace[0]['response'], denial)
        self.assertEqual(host.dispatch({'action': 'read_reference', 'path': 'references/mcp-tools.md'}),
                         {'text': 'tool documentation'})

    def test_bare_tool_denied_and_qualified_tool_preserves_flat_arguments(self):
        calls = []

        def extract(arguments):
            calls.append(arguments)
            return {'status': 'partial', 'result': {'facts': []}}

        host = SkillHost('memory-mcp', {}, {'mcp__memory__extract': extract})
        arguments = {'episode_id': 'episode:trace'}
        denied = host.dispatch({'action': 'call', 'name': 'extract', 'arguments': arguments})
        self.assertTrue(denied['denied'])
        result = host.dispatch({'action': 'call', 'name': 'mcp__memory__extract', 'arguments': arguments})
        self.assertEqual(result['status'], 'partial')
        self.assertEqual(calls, [arguments])

    def test_cli_preserves_multiline_stdin_and_separate_process_channels(self):
        calls = []

        def run(argv, stdin):
            calls.append((argv, stdin))
            return {'exit_status': 0, 'stdout': '{"status":"partial"}', 'stderr': 'still pending'}

        host = SkillHost('memory-cli', {}, {}, cli=run)
        argv = ['memory_mcp', 'ingest', '--content-stdin']
        stdin = 'https://example.org/spec\n/work/spec.md'
        denied = host.dispatch({'action': 'run', 'argv': ['/bin/sh', '-c', 'pwd']})
        self.assertTrue(denied['denied'])
        result = host.dispatch({'action': 'run', 'argv': argv, 'stdin': stdin})
        self.assertEqual(calls, [(argv, stdin)])
        self.assertEqual(result, {'exit_status': 0, 'stdout': '{"status":"partial"}', 'stderr': 'still pending'})


class CaptureEvidenceTests(unittest.TestCase):
    def test_literal_mcp_capture_may_complete_extraction_of_the_returned_episode(self):
        from evals.paired.skill_workflow import grade_observed, LITERAL
        trace = [
            {'request': {'action': 'call', 'name': 'mcp__memory__ingest', 'arguments': LITERAL},
             'response': {'status': 'success', 'result': 'episode:literal'}},
            {'request': {'action': 'call', 'name': 'mcp__memory__extract',
                         'arguments': {'episode_id': 'episode:literal'}},
             'response': {'status': 'success', 'result': {'episode_id': 'episode:literal', 'facts': []}}},
        ]
        grade = grade_observed({'skill': 'memory-mcp', 'id': 4}, {'terminal': 'answered'}, trace)
        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertEqual(grade['pending_vs_completed_oracle'], 'episode-only')

    def test_literal_mcp_capture_cannot_extract_an_unrelated_episode(self):
        from evals.paired.skill_workflow import grade_observed, LITERAL
        trace = [
            {'request': {'action': 'call', 'name': 'mcp__memory__ingest', 'arguments': LITERAL},
             'response': {'status': 'success', 'result': 'episode:literal'}},
            {'request': {'action': 'call', 'name': 'mcp__memory__extract',
                         'arguments': {'episode_id': 'episode:other'}},
             'response': {'status': 'success', 'result': {'episode_id': 'episode:other', 'facts': []}}},
        ]
        grade = grade_observed({'skill': 'memory-mcp', 'id': 4}, {'terminal': 'answered'}, trace)
        self.assertFalse(grade['operation_and_exact_arguments'])

    def test_pending_extract_exit_zero_is_never_episode_only(self):
        from evals.paired.skill_workflow import capture_status
        result = {'status': 'partial', 'result': {'episode_id': 'episode:a', 'facts': []},
                  'guidance': 'Extraction is pending. Retry is safe.'}
        self.assertEqual(capture_status(result), 'pending')

    def test_complete_empty_and_complete_nonempty_are_distinct(self):
        from evals.paired.skill_workflow import capture_status
        empty = {'status': 'success', 'result': {'episode_id': 'episode:a', 'facts': []},
                 'guidance': 'episode-only. Try a structured summary with ## Pending items.'}
        self.assertEqual(capture_status(empty), 'episode-only')
        result = {'status': 'success', 'result': {'episode_id': 'episode:a', 'facts': [{'fact_id': 'fact:a'}],
                                                 'fact_evidence_omitted': True}}
        self.assertEqual(capture_status(result), 'verified')

    def test_unbound_or_unusable_fact_evidence_cannot_verify_capture(self):
        from evals.paired.skill_workflow import capture_status
        status = {'status': 'success', 'result': {'episode_id': 'episode:wrong', 'facts': [{}]}}
        self.assertEqual(capture_status(status), 'pending')

    def test_cli_process_failure_is_retained_separately_from_pending_classification(self):
        from evals.paired.skill_workflow import grade_observed, process_result, SOURCE
        host = SkillHost('memory-cli', {}, {}, cli=lambda argv, stdin: process_result(
            {'status': 'success', 'result': 'episode:bound'} if argv[1] == 'ingest' else
            {'status': 'success', 'result': {'episode_id': 'episode:bound', 'facts': [{'fact_id': 'fact:a'}]}},
            exit_status=2 if argv[1] == 'extract' else 0))
        from evals.paired.skill_workflow import cli_ingest_args
        host.dispatch({'action': 'run', 'argv': cli_ingest_args(SOURCE, stdin=True), 'stdin': SOURCE['content']})
        host.dispatch({'action': 'run', 'argv': ['memory_mcp', 'extract', '--episode-id', 'episode:bound']})
        grade = grade_observed({'skill': 'memory-cli', 'id': 1}, {'terminal': 'answered'}, host.trace)
        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertEqual(grade['process_faults'], [{'operation': 'extract', 'exit_status': 2}])
        self.assertEqual(grade['pending_vs_completed_oracle'], 'pending')


class CaptureReadbackTests(unittest.TestCase):
    def capture_grade(self, *, fact_id='fact:decision', source_id='cedar-release-minutes-2026-11-20',
                      extract_episode='episode:captured', requested_episode='episode:captured',
                      readback_episode='episode:captured',
                      commands=('ingest', 'extract', 'assemble-context'), readback_exit=0):
        from evals.paired.skill_workflow import SOURCE, cli_ingest_args, grade_observed, process_result

        episode = 'episode:captured'
        receipts = {
            'ingest': process_result({'status': 'success', 'result': episode}),
            'extract': process_result({'status': 'success', 'result': {
                'episode_id': extract_episode, 'facts': [{'fact_id': 'fact:decision'}]}}),
            'assemble-context': process_result({'status': 'success', 'result': [{
                'fact_id': fact_id, 'source_episode': readback_episode,
                'provenance': {'source_id': source_id, 'source_episode_id': readback_episode},
            }]}, exit_status=readback_exit),
            'resolve': process_result({'status': 'success', 'result': 'entity:created'}),
        }
        actions = {
            'ingest': cli_ingest_args(SOURCE),
            'extract': ['memory_mcp', 'extract', '--episode-id', requested_episode],
            'assemble-context': ['memory_mcp', 'assemble-context', '--query', 'Cedar Index decision'],
            'resolve': ['memory_mcp', 'resolve', '--entity-type', 'person', '--canonical-name', 'Alice Smith'],
        }
        host = SkillHost('memory-cli', {}, {}, cli=lambda argv, stdin: receipts[argv[1]])
        for name in commands:
            host.dispatch({'action': 'run', 'argv': actions[name]})
        return grade_observed({'skill': 'memory-cli', 'id': 1}, {'terminal': 'error'}, host.trace)

    def test_linked_capture_with_source_bound_readback_passes_operation_grade(self):
        grade = self.capture_grade()
        self.assertTrue(grade['operation_and_exact_arguments'])
        self.assertTrue(grade['source_binding_observed'])
        self.assertEqual(grade['pending_vs_completed_oracle'], 'verified')

    def test_readback_of_wrong_fact_or_source_cannot_verify_capture(self):
        for overrides in ({'fact_id': 'fact:unrelated'}, {'source_id': 'unrelated-source'},
                          {'readback_episode': 'episode:unrelated'}):
            with self.subTest(overrides=overrides):
                grade = self.capture_grade(**overrides)
                self.assertFalse(grade['operation_and_exact_arguments'])

    def test_readback_cannot_rescue_extract_from_different_episode(self):
        for field in ('extract_episode', 'requested_episode'):
            with self.subTest(field=field):
                grade = self.capture_grade(**{field: 'episode:unrelated'})
                self.assertFalse(grade['operation_and_exact_arguments'])

    def test_out_of_order_readback_and_additional_mutation_still_fail(self):
        for commands in (('ingest', 'assemble-context', 'extract'),
                         ('ingest', 'extract', 'assemble-context', 'resolve')):
            with self.subTest(commands=commands):
                grade = self.capture_grade(commands=commands)
                self.assertFalse(grade['operation_and_exact_arguments'])

    def test_readback_with_failed_process_cannot_verify_capture(self):
        grade = self.capture_grade(readback_exit=2)
        self.assertFalse(grade['operation_and_exact_arguments'])
        self.assertEqual(grade['process_faults'], [{'operation': 'assemble-context', 'exit_status': 2}])


class ExecutionTests(unittest.IsolatedAsyncioTestCase):
    async def test_stated_command_without_call_never_counts_as_capture(self):
        from evals.paired.skill_workflow import grade_observed, play

        async def completes_without_tools(messages, deadline):
            return {'choices': [{'message': {'content':
                '{"action":"finish","answer":"I ran ingest and extract. Status verified."}'},
                'finish_reason': 'stop'}]}

        host = SkillHost('memory-mcp', {}, {})
        result = await play([], host, completes_without_tools)
        grade = grade_observed({'skill': 'memory-mcp', 'id': 1}, result, host.trace)
        self.assertEqual(result['terminal'], 'answered')
        self.assertFalse(grade['operation_and_exact_arguments'])

    async def test_malformed_action_is_not_repaired_or_dispatched(self):
        from evals.paired.skill_workflow import play

        async def markdown_wrapped_action(messages, deadline):
            return {'choices': [{'message': {'content':
                '```json\n{"action":"read_reference","path":"references/mcp-tools.md"}\n```'},
                'finish_reason': 'stop'}]}

        host = SkillHost('memory-mcp', {'references/mcp-tools.md': 'document'}, {})
        result = await play([], host, markdown_wrapped_action)
        self.assertEqual(result['terminal'], 'error')
        self.assertEqual(host.trace, [])

    async def test_preceding_setup_and_dispatch_share_the_case_deadline(self):
        from evals.paired.skill_workflow import play

        async def unexpected_request(messages, deadline):
            self.fail('Inference request consumed already expired case budget')

        host = SkillHost('memory-mcp', {}, {})
        result = await play([], host, unexpected_request, started=0, deadline=0)
        self.assertEqual(result['terminal'], 'case_deadline')

    async def test_client_startup_fault_still_records_every_scheduled_arm(self):
        from evals.paired.skill_workflow import BUDGET, capture_trials

        @contextmanager
        def host_context(*args, **kwargs):
            yield SkillHost('memory-mcp', {}, {}), 'instructions', {}

        with TemporaryDirectory() as work, patch('evals.paired.skill_workflow.trial_host', host_context), patch(
                'evals.paired.skill_workflow.local_async_client', side_effect=RuntimeError('transport unavailable')):
            output = Path(work)
            plan = {'cases': [dict(skill='memory-mcp', id=1, binding='native'),
                              dict(skill='memory-cli', id=1, binding='native')],
                    'budget': BUDGET, 'endpoint': 'http://127.0.0.1:9/v1'}
            await capture_trials(plan, output)
            summary = json.loads((output / 'summary.json').read_text())
            self.assertEqual(summary['scheduled'], 4)
            self.assertEqual([row['terminal'] for row in summary['rows']], ['client_error'] * 4)
            self.assertEqual(len(list(output.glob('*/trial.json'))), 4)


class ControlledSourceTests(unittest.TestCase):
    def test_short_continuation_does_not_verify_the_whole_quote(self):
        from evals.paired.skill_workflow import Controlled, LONG_EPISODE, QUOTE
        host = Controlled('memory-mcp', 6)
        item_request = json.dumps([{'source_episode': LONG_EPISODE, 'quote': QUOTE}])
        result = host.mcp('explain', {'context_items': item_request,
                                     'source_offset': 1200, 'source_limit_scalars': 2})
        item = result['result'][0]
        self.assertTrue(item['quote_verified'])
        self.assertTrue(item['quote_complete'])
        self.assertFalse(item['quote_contained_in_window'])
        self.assertEqual(len(item['citation_context']), 2)

    def test_bare_episode_reads_text_without_inventing_verified_quote(self):
        from evals.paired.skill_workflow import Controlled, LONG_EPISODE, QUOTE
        result = Controlled('memory-mcp', 6).mcp('explain', {
            'context_items': json.dumps([LONG_EPISODE]), 'source_offset': 1200})
        item = result['result'][0]
        self.assertIn(QUOTE, item['citation_context'])
        self.assertFalse(item['quote_verified'])
        self.assertNotIn('quote', item)

    def test_containing_slice_starting_before_quote_is_accepted(self):
        from evals.paired.skill_workflow import Controlled, LONG_EPISODE, QUOTE, grade_observed
        controlled = Controlled('memory-mcp', 6)
        host = SkillHost('memory-mcp', {}, {'mcp__memory__explain': lambda args: controlled.mcp('explain', args)})
        for offset, item in ((0, LONG_EPISODE), (1190, {'source_episode': LONG_EPISODE, 'quote': QUOTE})):
            host.dispatch({'action': 'call', 'name': 'mcp__memory__explain', 'arguments': {
                'context_items': json.dumps([item]), 'source_offset': offset}})
        grade = grade_observed({'skill': 'memory-mcp', 'id': 6}, {'terminal': 'answered'}, host.trace)
        self.assertTrue(grade['source_binding_observed'])
        self.assertTrue(grade['operation_and_exact_arguments'])

    def test_invalid_selected_source_request_cannot_return_evidence(self):
        from evals.paired.skill_workflow import Controlled, LONG_EPISODE, LONG_FACT, QUOTE
        controlled = Controlled('memory-mcp', 6)
        source = {'source_episode': LONG_EPISODE, 'fact_id': LONG_FACT, 'quote': QUOTE}
        baseline = {'context_items': json.dumps([source]), 'source_offset': 1200}
        for bad in ({'source_offset': 1200.5}, {'provenance_offset': False}, {'item_offset': -1},
                    {'unknown': 4}, {'context_items': json.dumps([{**source, 'fact_id': 'fact:other'}])}):
            with self.subTest(bad=bad):
                self.assertEqual(controlled.mcp('explain', {**baseline, **bad})['status'], 'error')

    def test_compact_extract_does_not_attach_omission_flag(self):
        from evals.paired.skill_workflow import Controlled, LONG_EPISODE
        response = Controlled('memory-cli', 7).cli(
            ['memory_mcp', 'extract', '--episode-id', LONG_EPISODE, '--compact'], '')
        result = json.loads(response['stdout'])['result']
        self.assertTrue(result['facts'])
        self.assertNotIn('fact_evidence_omitted', result)


class FrozenExecutionTests(unittest.TestCase):
    def test_portable_plan_detects_changed_stdio_binding(self):
        from evals.paired.skill_workflow import freeze, verify_plan
        with TemporaryDirectory() as work:
            plan = freeze(Path(work) / 'freeze', Path(sys.executable), 'http://127.0.0.1:9/v1',
                          'fixture-model', profile='portable')
            verify_plan(plan)
            plan['mcp_binding_sha256'] = 'changed'
            with self.assertRaisesRegex(ValueError, 'MCP binding'):
                verify_plan(plan)

    def test_portable_plan_detects_changed_inference_observer(self):
        from evals.paired.skill_workflow import freeze, verify_plan
        with TemporaryDirectory() as work:
            plan = freeze(Path(work) / 'freeze', Path(sys.executable), 'http://127.0.0.1:9/v1',
                          'fixture-model', profile='portable')
            plan['attempt_observer_sha256'] = 'changed'
            with self.assertRaisesRegex(ValueError, 'inference observer'):
                verify_plan(plan)
