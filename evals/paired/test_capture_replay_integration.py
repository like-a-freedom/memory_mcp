"""Frozen report replay at its subprocess boundary, without inference."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory
import unittest

REPO = Path(__file__).resolve().parents[2]
PAIRED = REPO/'evals/paired/source-action-qualification-2026-11-27'
LEGACY = REPO/'evals/paired/skill-completion-qualification-2026-11-27'
READER = REPO/'evals/longmemeval_v2/results/2026-11-27-source-item-qualification'


class FrozenCaptureReplayTests(unittest.TestCase):
    def test_reader_replay_uses_archived_matcher_despite_live_matcher_drift(self):
        script = (
            'import runpy; '
            'import evals.longmemeval_v2.exact_answer_quality as live; '
            'live.exact_phrase_set_match = lambda *args, **kwargs: False; '
            f'runpy.run_path({str(READER/"replay_capture.py")!r}, run_name="__main__")')

        result = subprocess.run([sys.executable,'-B','-c',script],cwd=REPO,
                                capture_output=True,text=True,timeout=30)

        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['whole_item_strict'],49)

    def test_source_action_replay_rejects_missing_request_evidence(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)/'capture'
            shutil.copytree(PAIRED,root)
            (root/'registered/capture/inference-events.jsonl').write_text('')
            (root/'registered/capture/inference-attempts.jsonl').write_text('')
            script = f'import runpy; runpy.run_path({str(root/"replay_capture.py")!r}, run_name="__main__")'

            result = subprocess.run([sys.executable,'-B','-c',script],cwd=REPO,
                                    capture_output=True,text=True,timeout=30)

            self.assertNotEqual(result.returncode,0,result.stdout)
            self.assertIn('request accounting',result.stderr)

    def test_earlier_workflow_replay_rejects_missing_request_evidence(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)/'capture'
            shutil.copytree(LEGACY,root)
            (root/'fresh-native/capture/inference-events.jsonl').write_text('')
            (root/'fresh-native/capture/inference-attempts.jsonl').write_text('')
            script = f'import runpy; runpy.run_path({str(root/"replay_capture.py")!r}, run_name="__main__")'

            result = subprocess.run([sys.executable,'-B','-c',script],cwd=REPO,
                                    capture_output=True,text=True,timeout=30)

            self.assertNotEqual(result.returncode,0,result.stdout)
            self.assertIn('request accounting',result.stderr)


if __name__ == '__main__':
    unittest.main()
