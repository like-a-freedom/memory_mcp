import json
import os
from pathlib import Path
import subprocess
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch


def build_shared_memory(root, questions, content):
    from evals.longmemeval_v2.memory_mcp_backend import MemoryMcpBackend
    from evaluation.harness import main

    trajectory_id = "synthetic-trajectory"
    inputs = {
        "questions": [
            {
                "id": question_id, "question_type": "static-environment",
                "question": question, "answer": "tea",
                "eval_function": "norm_phrase_set_match",
            }
            for question_id, question in questions.items()
        ],
        "haystack": {question_id: [trajectory_id] for question_id in questions},
        "trajectories": [{"id": trajectory_id, "content": content}],
        "memory-config": {
            "memory_type": MemoryMcpBackend.memory_type,
            "memory_params": {
                "binary": os.environ["LONGMEMEVAL_V2_RESTORE_SMOKE_BINARY"],
                "db_path": str(root / "original"),
            },
        },
    }
    argv = ["harness.py", "--domain", "web", "--output-dir", str(root / "build"),
            "--save-memory", "--skip-evaluation"]
    for name, payload in inputs.items():
        path = root / f"{name}.json"
        path.write_text(json.dumps(payload), encoding="utf-8")
        argv.extend([f"--{name}-path", str(path)])
    with patch.object(sys, "argv", argv):
        main()


@unittest.skipUnless(
    os.getenv("LONGMEMEVAL_V2_RESTORE_SMOKE_BINARY"),
    "requires pinned upstream checkout on PYTHONPATH and a native product binary",
)
class UpstreamRestoreTests(unittest.TestCase):
    def test_saved_store_reopens_after_original_store_moves(self):
        from memory_modules.memory import load_memory

        with TemporaryDirectory() as directory:
            root = Path(directory)
            original = root / "original"
            build_shared_memory(root, {"question-a": "What do I prefer?"}, "I prefer tea")
            snapshot = root / "build" / "memory_state"
            original.rename(root / "moved")

            restored = load_memory(snapshot)
            self.addCleanup(restored._restored_store.cleanup)
            items = restored.query("What do I prefer?")

            self.assertTrue(any("tea" in item["value"].lower() for item in items))
            self.assertNotEqual(Path(restored.db_path), original)
            self.assertFalse(original.exists())

    def test_shared_build_extraction_failure_aborts_before_successful_reads(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            questions = {"question-a": "What do I prefer?", "question-b": "Which drink?"}

            with self.assertRaises(subprocess.CalledProcessError) as caught:
                build_shared_memory(root, questions, "x" * 1_048_577)

            self.assertEqual(caught.exception.cmd[1], "extract")
            self.assertIn(b"entity extraction input too large", caught.exception.stderr)
            output = root / "build"
            self.assertFalse((output / "memory_state" / "store").exists())
            self.assertFalse((output / "prompt_rows.jsonl").exists())
            self.assertFalse((output / "per_question.jsonl").exists())
            self.assertFalse((output / "aggregated_metrics.json").exists())
