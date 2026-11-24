import argparse
import asyncio
import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest

from evals.longmemeval_v2.scored_coverage import loopback_endpoint, run_observed


class ScoredCoverageTests(unittest.TestCase):
    def test_reader_fallback_is_excluded_from_coverage_but_native_score_is_preserved(self):
        with TemporaryDirectory() as directory:
            output = Path(directory)

            async def reader(client, args, messages):
                raise TimeoutError()

            async def generate(args, rows):
                try:
                    await harness.call_reader_model_async(None, args, rows[0]["messages"])
                except TimeoutError:
                    return {}

            async def reader_phase():
                await harness.generate_all_reader_outputs(None, [
                    {"question_id": "a", "messages": []},
                ])

            def main():
                asyncio.run(reader_phase())
                (output / "per_question.jsonl").write_text('{"question_id":"a","score":1}\n')
                (output / "aggregated_metrics.json").write_text('{"overall":{"overall_full_set":1,"count_all_questions":1}}')

            harness = SimpleNamespace(main=main, call_reader_model_async=reader,
                                      generate_all_reader_outputs=generate,
                                      score_prediction=None, LLM_EVAL_FUNCTIONS=set())
            self.assertEqual(run_observed(harness, [], ["a"], output), 1)
            report = json.loads((output / "score-coverage.json").read_text())
            self.assertEqual(report["cases"], {"a": "reader_timeout"})
            self.assertEqual(report["covered_case_count"], 0)
            self.assertEqual(report["native_scored_case_count"], 1)
            self.assertEqual(report["coverage_adjusted_success_rate"], 0.0)
            self.assertEqual(report["official_metrics_status"], "emitted")

    def test_judge_abort_records_exact_case_and_preserves_unscored_suffix(self):
        with TemporaryDirectory() as directory:
            output = Path(directory)

            def score(row, config):
                raise ValueError("malformed judge response")

            def main():
                (output / "per_question.jsonl").write_text('{"question_id":"a","score":1}\n')
                harness.score_prediction({"question_id": "b", "eval_name": "llm"}, {})

            harness = SimpleNamespace(main=main, call_reader_model_async=None,
                                      generate_all_reader_outputs=None,
                                      score_prediction=score, LLM_EVAL_FUNCTIONS={"llm"})
            with self.assertRaises(ValueError):
                run_observed(harness, [], ["a", "b", "c"], output)
            report = json.loads((output / "score-coverage.json").read_text())
            self.assertEqual(report["cases"], {"a": "scored", "b": "judge_error", "c": "unscored"})
            self.assertEqual(report["official_metrics_status"], "unavailable")

    def test_judge_failure_after_reader_fallback_records_both_stages(self):
        with TemporaryDirectory() as directory:
            output = Path(directory)
            messages = []

            async def reader(client, args, messages):
                raise ConnectionError()

            async def generate(args, rows):
                with self.assertRaises(ConnectionError):
                    await harness.call_reader_model_async(None, args, messages)

            def score(row, config):
                raise TimeoutError()

            def main():
                asyncio.run(harness.generate_all_reader_outputs(None, [
                    {"question_id": "a", "messages": messages},
                ]))
                harness.score_prediction({"question_id": "a", "eval_name": "llm"}, {})

            harness = SimpleNamespace(main=main, call_reader_model_async=reader,
                                      generate_all_reader_outputs=generate,
                                      score_prediction=score, LLM_EVAL_FUNCTIONS={"llm"})
            with self.assertRaises(TimeoutError):
                run_observed(harness, [], ["a"], output)
            report = json.loads((output / "score-coverage.json").read_text())
            self.assertEqual(report["cases"], {"a": "reader_error"})
            self.assertEqual(report["failures"], {"a": ["reader_error", "judge_timeout"]})

    def test_remote_reader_or_judge_endpoint_is_refused(self):
        with self.assertRaisesRegex(argparse.ArgumentTypeError, "loopback"):
            loopback_endpoint("http://192.0.2.1:8000/v1")
