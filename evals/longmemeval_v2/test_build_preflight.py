import json
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
from types import ModuleType
import unittest
from unittest.mock import patch

from evals.longmemeval_v2.build_preflight import run_shared_build
from evals.longmemeval_v2.memory_mcp_backend import MemoryMcpBackend


class BuildPreflightTests(unittest.TestCase):
    def build(self, root, main):
        harness = ModuleType('evaluation.harness')
        harness.all_haystacks_shared = lambda *_: True
        harness.load_haystack_mapping = lambda _: {'q': ['trajectory']}
        harness.load_questions = lambda _: [{'id': 'q'}]
        harness.validate_question_and_haystack_ids = lambda *_: None
        harness.main = main
        with patch.dict(sys.modules, {'evaluation.harness': harness}):
            return run_shared_build(domain='web', questions=root/'questions', haystack=root/'haystack',
                                    trajectories=root/'trajectories', binary=Path(sys.executable), output=root/'build')

    def test_extract_response_failure_and_timeout_emit_failed_coverage(self):
        for error_type in (None, subprocess.TimeoutExpired, subprocess.CalledProcessError):
            with self.subTest(error_type=error_type), TemporaryDirectory() as directory:
                root = Path(directory)
                output = root/'build'

                def main():
                    params = json.loads((output/'memory-config.json').read_text())['memory_params']

                    def runner(command, *_):
                        if command == 'ingest':
                            return '{"result":"episode:issued"}'
                        if error_type:
                            raise error_type(1, [params['binary'], 'extract']) if error_type is subprocess.CalledProcessError \
                                else error_type([params['binary'], 'extract'], 1)
                        return '{"status":"success","result":null}'

                    MemoryMcpBackend(params, runner=runner).insert({'content':'A test source'})

                try:
                    status = self.build(root, main)
                except subprocess.TimeoutExpired:
                    self.fail('extract timeout escaped without build coverage')
                self.assertEqual(status, 1)
                report = json.loads((output/'build-coverage.json').read_text())
                self.assertEqual(report['build_status'], 'failed')
                self.assertEqual(report['failed_step'], 'extract')
                self.assertEqual(report['affected_case_ids'], ['q'])
                journal = output/'formation-attempts.jsonl'
                self.assertEqual(report['formation_attempts_file'], journal.name)
                finished = json.loads(journal.read_text().splitlines()[-1])
                self.assertEqual(finished['outcome'], 'failed')

    def test_harness_return_without_a_snapshot_cannot_report_saved(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            self.assertEqual(self.build(root, lambda: None), 1)
            report = json.loads((root/'build'/'build-coverage.json').read_text())
            self.assertEqual(report['build_status'], 'failed')
            self.assertEqual(report['failed_step'], 'save')
