import json
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest

from evals.longmemeval_v2.scored_coverage import run_observed


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
                import asyncio
                asyncio.run(reader_phase())
                (output / "per_question.jsonl").write_text('{"question_id":"a","score":0}\n')
                (output / "aggregated_metrics.json").write_text('{"overall":{"overall_full_set":0,"count_all_questions":1}}')

            harness = SimpleNamespace(main=main, call_reader_model_async=reader,
                                      generate_all_reader_outputs=generate,
                                      score_prediction=None, LLM_EVAL_FUNCTIONS=set())
            self.assertEqual(run_observed(harness, [], ["a"], output), 1)
            report = json.loads((output / "score-coverage.json").read_text())
            self.assertEqual(report["cases"], {"a": "reader_timeout"})
            self.assertEqual(report["covered_case_count"], 0)
            self.assertEqual(report["native_scored_case_count"], 1)
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
