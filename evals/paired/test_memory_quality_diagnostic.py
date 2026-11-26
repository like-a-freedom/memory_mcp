"""Public CLI grading controls for the bounded local diagnostic."""
import json
import hashlib
import pathlib
import subprocess
import sys
import tempfile
import unittest

SCRIPT = pathlib.Path(__file__).with_name("memory_quality_diagnostic.py")

class DiagnosticGrading(unittest.TestCase):
    def score(self, expected, answer):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "case.json").write_text(json.dumps(expected))
            (root / "answer.json").write_text(json.dumps(answer))
            result = subprocess.run([sys.executable, str(SCRIPT), "score", "--case", str(root / "case.json"), "--answer", str(root / "answer.json")], capture_output=True, text=True, timeout=10)
            self.assertEqual(result.returncode, 0, result.stderr)
            return json.loads(result.stdout)

    def test_numeric_prefix_is_not_exact_value(self):
        result = self.score({"answer":"47", "source_ids":["episode:source"], "abstain":False}, {"answer":"470", "source_ids":["episode:source"], "abstain":False})
        self.assertFalse(result["value_correct"])
        self.assertFalse(result["passed"])

    def test_correct_value_with_wrong_source_fails(self):
        result = self.score({"answer":"47", "source_ids":["episode:source"], "abstain":False}, {"answer":"47", "source_ids":["episode:other"], "abstain":False})
        self.assertFalse(result["passed"])
        self.assertFalse(result["sources_correct"])

    def test_missing_answer_is_not_justified_abstention(self):
        result = self.score({"answer":None, "source_ids":["episode:pending"], "abstain":True}, {"source_ids":["episode:pending"], "abstain":True})
        self.assertFalse(result["passed"])

    def test_exact_answer_and_evidence_are_accepted(self):
        expected = {"answer":"47", "source_ids":["episode:source"], "abstain":False}
        self.assertTrue(self.score(expected, expected)["passed"])

    def test_supported_false_premise_abstention_is_accepted(self):
        expected = {"answer":None, "source_ids":["episode:pending"], "abstain":True}
        self.assertTrue(self.score(expected, expected)["passed"])

    def test_startup_failure_preserves_all_scheduled_arms(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            plan = {"script_sha256":hashlib.sha256(SCRIPT.read_bytes()).hexdigest(), "product_binary":sys.executable, "product_binary_sha256":hashlib.sha256(pathlib.Path(sys.executable).read_bytes()).hexdigest(), "model_budget_seconds":1, "launch_env":{}, "cases":[{"id":"ordinary"},{"id":"correction"},{"id":"false-premise"}], "arms":["registered_query","selected_query","fixed_sufficient_reply"]}
            (root / "frozen-plan.json").write_text(json.dumps(plan))
            result = subprocess.run([sys.executable,str(SCRIPT),"capture","--output",str(root)],capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            report = json.loads((root / "result.json").read_text())
            self.assertEqual(len(report["captures"]),1)
            rows = report["captures"][0]["rows"]
            self.assertEqual(len(rows),9)
            self.assertTrue(all(row["status"] == "invalid" for row in rows))
            self.assertFalse(report["c2_eligible"])

    def test_freeze_accepts_opaque_model_id_without_local_model_files(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            config = root / "config.json"
            config.write_text(json.dumps({"model":"opaque-model-alias", "endpoint":"http://127.0.0.1:18767/v1/chat/completions", "launch_env":{}}))
            result = subprocess.run([sys.executable,str(SCRIPT),"freeze","--output",str(root / "frozen"),"--binary",sys.executable,"--config",str(config)],capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            plan = json.loads((root / "frozen/frozen-plan.json").read_text())
            self.assertEqual(plan["model"],"opaque-model-alias")
            self.assertIsNone(plan["model_config_sha256"])

    def test_existing_capture_result_cannot_be_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            config = root / "config.json"
            config.write_text(json.dumps({"model":"opaque-model-alias", "endpoint":"http://127.0.0.1:18767/v1/chat/completions", "launch_env":{}}))
            frozen = root / "frozen"
            result = subprocess.run([sys.executable,str(SCRIPT),"freeze","--output",str(frozen),"--binary",sys.executable,"--config",str(config)],capture_output=True,text=True,timeout=10)
            self.assertEqual(result.returncode,0,result.stderr)
            saved = b'{"preserved_prior_attempt": true}\n'
            (frozen / "result.json").write_bytes(saved)
            (frozen / "capture-01").mkdir()
            result = subprocess.run([sys.executable,str(SCRIPT),"capture","--output",str(frozen)],capture_output=True,text=True,timeout=10)
            self.assertNotEqual(result.returncode,0)
            self.assertEqual((frozen / "result.json").read_bytes(),saved)

if __name__ == "__main__":
    unittest.main()
