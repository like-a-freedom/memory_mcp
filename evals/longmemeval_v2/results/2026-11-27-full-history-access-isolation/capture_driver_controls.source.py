import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

path = Path('/tmp/memory-quality-v5-paired-capture.py')
spec = importlib.util.spec_from_file_location('paired_driver', path)
driver = importlib.util.module_from_spec(spec)
spec.loader.exec_module(driver)

CASE = 'case'
SPAN = 'complete reviewed span'
SOURCE = 'episode:probe'
START = dict(case_id=CASE, episode_count=151, carrier_span_present=True, source_episode=SOURCE)
META = dict(exit_code=101, elapsed_seconds=31.0, external_timeout=False,
            strict_span_assertion=True, malformed_events=[], process_fault=None)


def receipt(case=CASE, elapsed=29_000, content='a sufficient alternate span'):
    return dict(case_id=case, elapsed_ms=elapsed, receipt=dict(stages=[], items=[
        {'source_episode': SOURCE, 'fact_id': 'fact:probe', 'content': content}]))


class ReadClassification(unittest.TestCase):
    def classify(self, final=None, meta=None):
        events = [('FULL_HISTORY_START', START)] + [
            ('FULL_HISTORY_RECEIPT', item) for item in ([receipt()] if final is None else final)
        ]
        return driver.classify_read({**META, **(meta or {})}, events, CASE, SPAN)

    def test_known_101_strict_span_failure_has_a_delivered_context(self):
        result = self.classify()
        self.assertIsNone(result['retrieval_fault'])
        self.assertFalse(result['strict_original_source_span'])
        self.assertTrue(result['runtime_completed'])
        self.assertEqual(result['delivered_items'][0]['content'], 'a sufficient alternate span')

    def test_full_span_and_exit_zero_delivers_context(self):
        result = self.classify([receipt(content=SPAN)], {'exit_code': 0, 'strict_span_assertion': False})
        self.assertIsNone(result['retrieval_fault'])
        self.assertTrue(result['strict_original_source_span'])

    def test_unexplained_101_preserves_observation_without_delivered_context(self):
        result = self.classify(meta={'strict_span_assertion': False})
        self.assertTrue(result['receipt_observed'])
        self.assertIsNone(result['delivered_items'])
        self.assertEqual(result['retrieval_fault'], 'unexpected_process_exit')

    def test_timeout_is_a_fault_even_when_a_receipt_was_printed(self):
        result = self.classify(meta={'exit_code': -9, 'external_timeout': True, 'elapsed_seconds': 45.1})
        self.assertTrue(result['receipt_observed'])
        self.assertEqual(result['retrieval_fault'], 'external_deadline')
        self.assertIsNone(result['delivered_items'])

    def test_natural_late_exit_is_a_process_deadline_fault(self):
        result = self.classify(meta={'exit_code': 0, 'external_timeout': False, 'elapsed_seconds': 45.01})
        self.assertEqual(result['retrieval_fault'], 'external_deadline')

    def test_completed_read_above_internal_30_second_budget_is_a_fault(self):
        result = self.classify([receipt(elapsed=30000.1)])
        self.assertEqual(result['retrieval_fault'], 'assembly_deadline')
        self.assertIsNone(result['delivered_items'])

    def test_wrong_case_receipt_is_a_fault(self):
        self.assertEqual(self.classify([receipt(case='other')])['retrieval_fault'], 'invalid_receipt')

    def test_duplicate_receipt_is_a_fault(self):
        self.assertEqual(self.classify([receipt(), receipt()])['retrieval_fault'], 'invalid_receipt')

    def test_malformed_receipt_is_a_fault(self):
        self.assertEqual(self.classify([{'case_id': CASE}])['retrieval_fault'], 'invalid_receipt')

    def test_missing_receipt_does_not_turn_exit_zero_into_success(self):
        self.assertEqual(self.classify([], {'exit_code': 0})['retrieval_fault'], 'missing_receipt')

    def test_strict_match_must_come_from_registered_source(self):
        row = receipt(content=SPAN)
        row['receipt']['items'][0]['source_episode'] = 'episode:other'
        result = self.classify([row])
        self.assertFalse(result['strict_original_source_span'])
        self.assertTrue(result['runtime_completed'])

    def test_raw_child_parser_retains_later_lines_after_truncated_utf8(self):
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp) / 'child.log'
            full = 'FULL_HISTORY_RECEIPT ' + json.dumps(receipt()) + '\n'
            payload = full.encode() + b'FULL_HISTORY_PHASE {"x":"\xe2\x82'  # truncated UTF-8 and frame
            code = f'import sys; sys.stdout.buffer.write({payload!r}); sys.exit(0)'
            meta, events = driver.run_child([sys.executable, '-c', code], {}, raw, 5)
            self.assertEqual(meta['exit_code'], 0)
            self.assertEqual([x[0] for x in events], ['FULL_HISTORY_RECEIPT'])
            self.assertEqual(meta['malformed_events'], [{'line': 2, 'event': 'FULL_HISTORY_PHASE'}])
            self.assertEqual(raw.read_bytes(), payload)

    def test_controlled_child_timeout_is_killed_and_frames_still_parsed(self):
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp) / 'child.log'
            # The process spins after flushing its start, so it cannot exit early.
            code = f'print({("FULL_HISTORY_START " + json.dumps(START))!r}, flush=True)\nwhile True: pass'
            meta, events = driver.run_child([sys.executable, '-c', code], {}, raw, 0.5)
            self.assertTrue(meta['external_timeout'])
            self.assertEqual(meta['exit_code'], -9)
            self.assertEqual(events, [('FULL_HISTORY_START', START)])
            self.assertTrue(raw.is_file())


    def test_nonfinite_json_is_a_malformed_frame_with_later_events_retained(self):
        with tempfile.TemporaryDirectory() as tmp:
            raw = Path(tmp) / 'child.log'
            raw.write_bytes(b'FULL_HISTORY_PHASE {"elapsed_ms":NaN}\n'
                            b'FULL_HISTORY_PHASE {"elapsed_ms":3e500}\n'
                            b'FULL_HISTORY_PHASE {"elapsed_ms":3.5}\n')
            rows, malformed, _ = driver.parse_raw(raw)
            self.assertEqual(rows, [('FULL_HISTORY_PHASE', {'elapsed_ms': 3.5})])
            self.assertEqual([row['line'] for row in malformed], [1, 2])

    def test_cooperative_timeout_preserves_partial_elapsed_and_missing_delivery(self):
        failure = dict(case_id=CASE, elapsed_ms=32_100, error='Err(Elapsed(()))')
        partial = dict(case_id=CASE, elapsed_ms=32_090, completed=False, stages=[])
        events = [('FULL_HISTORY_START', START), ('FULL_HISTORY_PARTIAL_RECEIPT', partial),
                  ('FULL_HISTORY_FAILURE', failure)]
        result = driver.classify_read(META, events, CASE, SPAN)
        self.assertEqual(result['retrieval_fault'], 'assembly_deadline')
        self.assertFalse(result['receipt_observed'])
        self.assertEqual(result['partial_elapsed_ms'], 32_090)

    def test_failed_one_arm_keeps_all_fourteen_attempts_and_later_pairs(self):
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp)
            initial = {'manifest': 'candidate_hash'}
            original = {'manifest': 'original_hash'}
            reads = {'cases': {case: {'query': 'read', 'required_serialized_span': SPAN}
                               for case in driver.CASES}}
            driver.save(out / 'snapshot-admission.json', {
                'source_snapshot_unchanged': True, 'source_segments': 151,
                'source_verification': {'complete': True, 'episode_count': 151},
                'upgraded_file_sha256': initial,
            })

            def observe(binary, snapshot, test, registration, raw, seconds, case):
                self.assertEqual(seconds, 45)
                if binary == driver.NEW and case == driver.CASES[3]:
                    raise OSError('controlled clone error')
                return {**META, 'exit_code': 0, 'strict_span_assertion': False}, [
                    ('FULL_HISTORY_START', {**START, 'case_id': case}),
                    ('FULL_HISTORY_RECEIPT', receipt(case, content=SPAN)),
                ]

            with patch.multiple(driver, OUT=out, RAW=out / 'raw',
                                preflight=lambda: ({}, reads, original), invoke_clone=observe,
                                hashes=lambda root: initial if root == driver.UPGRADED else original):
                driver.capture()

            summary = driver.load(out / 'reads-summary.json')
            self.assertEqual(summary['scheduled'], 14)
            self.assertEqual(len(summary['rows']), 14)
            self.assertEqual([row['arm'] for row in summary['rows'][:4]],
                             ['legacy', 'candidate', 'candidate', 'legacy'])
            self.assertEqual(sum(row['retrieval_fault'] == 'capture_fault' for row in summary['rows']), 1)
            self.assertTrue(summary['rows'][-1]['runtime_completed'])
            attempts = [json.loads(line) for line in (out / 'attempts.jsonl').read_text().splitlines()]
            self.assertEqual([row['event'] for row in attempts], ['start', 'terminal'] * 14)
            reader_rows = [json.loads(line) for line in (out / 'reader-rows.jsonl').read_text().splitlines()]
            self.assertEqual(len(reader_rows), 7)
            self.assertEqual(reader_rows[3]['retrieval_fault'], 'capture_fault')


if __name__ == '__main__':
    unittest.main()
