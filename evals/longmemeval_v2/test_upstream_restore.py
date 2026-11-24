import json
import os
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest


def build_shared_memory(root, questions, content, *, haystack=None):
    from evals.longmemeval_v2.build_preflight import run_shared_build

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
        "haystack": haystack if haystack is not None else {
            question_id: [trajectory_id] for question_id in questions
        },
        "trajectories": [{"id": trajectory_id, "content": content}],
    }
    for name, payload in inputs.items():
        (root / f"{name}.json").write_text(json.dumps(payload), encoding="utf-8")
    return run_shared_build(
        domain="web", questions=root / "questions.json",
        haystack=root / "haystack.json", trajectories=root / "trajectories.json",
        binary=Path(os.environ["LONGMEMEVAL_V2_RESTORE_SMOKE_BINARY"]),
        output=root / "build",
    )


@unittest.skipUnless(
    os.getenv("LONGMEMEVAL_V2_RESTORE_SMOKE_BINARY"),
    "requires pinned upstream checkout on PYTHONPATH and a native product binary",
)
class UpstreamRestoreTests(unittest.TestCase):
    def test_saved_store_reopens_after_original_store_moves(self):
        from memory_modules.memory import load_memory

        with TemporaryDirectory() as directory:
            root = Path(directory)
            original = root / "build" / "working-store"
            status = build_shared_memory(root, {"question-a": "What do I prefer?"}, "I prefer tea")
            self.assertEqual(status, 0)
            report = json.loads((root / "build" / "build-coverage.json").read_text())
            self.assertEqual(report["build_status"], "saved")
            self.assertEqual(report["affected_case_ids"], [])
            self.assertEqual(report["scored_case_count"], 0)
            self.assertEqual(report["official_metrics_status"], "unavailable")
            snapshot = root / "build" / "memory_state"
            original.rename(root / "moved")

            restored = load_memory(snapshot)
            self.addCleanup(restored._restored_store.cleanup)
            queries = ("What do I prefer?", "Which drink do I prefer?")
            forward = {query: restored.query(query) for query in queries}
            backward = {query: restored.query(query) for query in reversed(queries)}

            self.assertEqual(forward, backward)
            self.assertTrue(any("tea" in item["value"].lower() for item in forward[queries[0]]))
            self.assertNotEqual(Path(restored.db_path), original)
            self.assertFalse(original.exists())

    def test_retry_after_restart_preserves_the_retrieved_context(self):
        from evals.longmemeval_v2.memory_mcp_backend import MemoryMcpBackend

        with TemporaryDirectory() as directory:
            params = {
                "binary": os.environ["LONGMEMEVAL_V2_RESTORE_SMOKE_BINARY"],
                "db_path": directory,
            }
            trajectory = {"id": "synthetic-trajectory", "content": "I prefer tea"}
            first = MemoryMcpBackend(params)
            first.insert(trajectory)
            before = first.query("What do I prefer?")

            restarted = MemoryMcpBackend(params)
            restarted.insert(trajectory)
            after = restarted.query("What do I prefer?")

            self.assertTrue(any("tea" in item["value"].lower() for item in before))
            self.assertEqual(after, before)

    def test_shared_build_extraction_failure_aborts_before_successful_reads(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            questions = {"question-a": "What do I prefer?", "question-b": "Which drink?"}

            status = build_shared_memory(root, questions, "x" * 1_048_577)

            self.assertEqual(status, 1)
            output = root / "build"
            report = json.loads((output / "build-coverage.json").read_text())
            self.assertEqual(report["failed_step"], "extract")
            self.assertEqual(report["affected_case_ids"], ["question-a", "question-b"])
            self.assertEqual(report["scored_case_count"], 0)
            self.assertEqual(report["official_metrics_status"], "unavailable")
            self.assertFalse((output / "memory_state" / "store").exists())
            self.assertFalse((output / "prompt_rows.jsonl").exists())
            self.assertFalse((output / "per_question.jsonl").exists())
            self.assertFalse((output / "aggregated_metrics.json").exists())


    def test_rejects_different_haystack_order_before_creating_output(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, "shared nonempty ordered haystack"):
                build_shared_memory(
                    root, {"a": "What?", "b": "Which?"}, "I prefer tea",
                    haystack={"a": ["first", "second"], "b": ["second", "first"]},
                )
            self.assertFalse((root / "build").exists())
