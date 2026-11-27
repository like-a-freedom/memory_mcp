import argparse
import asyncio
import json
import os
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from evals.longmemeval_v2.scored_coverage import loopback_endpoint, run_observed


class ScoredCoverageTests(unittest.TestCase):
    def test_processor_preflight_failure_is_reported_before_prompt_building(self):
        with TemporaryDirectory() as directory:
            output = Path(directory)
            calls = []

            def preflight():
                calls.append("preflight")
                raise OSError("processor assets are unavailable")

            def main():
                calls.append("harness.main")

            harness = SimpleNamespace(main=main, call_reader_model_async=None,
                                      generate_all_reader_outputs=None, score_prediction=None,
                                      LLM_EVAL_FUNCTIONS=set())
            with self.assertRaisesRegex(RuntimeError, "preflight"):
                run_observed(harness, [], ["a", "b"], output, preflight=preflight)

            self.assertEqual(calls, ["preflight"])
            report = json.loads((output / "score-coverage.json").read_text())
            self.assertEqual(report["cases"], {"a": "preflight_error", "b": "preflight_error"})
            self.assertEqual(report["failures"], {
                "a": ["preflight_error"], "b": ["preflight_error"],
            })
            self.assertEqual(report["covered_case_count"], 0)
            self.assertEqual(report["official_metrics_status"], "unavailable")

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

    def test_reader_outputs_survive_a_later_judge_abort(self):
        with TemporaryDirectory() as directory:
            output = Path(directory)
            rows = [
                {
                    "question_id": case,
                    "messages": [{"role": "user", "content": f"prompt {case}"}],
                    "memory_context": [{"type": "text", "value": f"evidence {case}"}],
                }
                for case in ("a", "b")
            ]

            async def reader(_client, _args, messages):
                case = messages[0]["content"].removeprefix("prompt ")
                return f"answer {case}", {"total_tokens": 3}

            async def generate(_args, prompt_rows):
                observed = {}
                for row in prompt_rows:
                    response, usage = await harness.call_reader_model_async(
                        None, None, row["messages"]
                    )
                    observed[row["question_id"]] = {
                        "response_raw": response,
                        "response_parsed_boxed": response,
                        "is_unknown": False,
                        "usage": usage,
                    }
                return observed

            def score(_row, _config):
                raise ValueError("malformed judge response")

            def main():
                asyncio.run(harness.generate_all_reader_outputs(None, rows))
                harness.score_prediction({"question_id": "a", "eval_name": "llm"}, {})

            harness = SimpleNamespace(
                main=main,
                call_reader_model_async=reader,
                generate_all_reader_outputs=generate,
                score_prediction=score,
                LLM_EVAL_FUNCTIONS={"llm"},
            )
            with self.assertRaises(ValueError):
                run_observed(harness, [], ["a", "b"], output)

            captured = [
                json.loads(line)
                for line in (output / "reader_outputs.jsonl").read_text().splitlines()
            ]
            self.assertEqual([row["question_id"] for row in captured], ["a", "b"])
            self.assertEqual([row["status"] for row in captured], ["captured", "captured"])
            self.assertEqual([row["response_raw"] for row in captured], ["answer a", "answer b"])
            self.assertTrue(all(row["prompt_sha256"] for row in captured))
            self.assertTrue(all(row["memory_context_sha256"] for row in captured))
            self.assertNotIn("evidence a", (output / "reader_outputs.jsonl").read_text())

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
            captured = json.loads((output / "reader_outputs.jsonl").read_text())
            self.assertEqual(captured["question_id"], "a")
            self.assertEqual(captured["status"], "reader_error")
            self.assertEqual(captured["error_type"], "ConnectionError")
            self.assertNotIn("response_raw", captured)

    def test_remote_reader_or_judge_endpoint_is_refused(self):
        with self.assertRaisesRegex(argparse.ArgumentTypeError, "loopback"):
            loopback_endpoint("http://192.0.2.1:8000/v1")

    def test_loopback_defaults_do_not_fall_back_to_cloud_credentials(self):
        with TemporaryDirectory() as directory:
            def main():
                self.assertEqual(os.environ["LME_LOCAL_READER_TOKEN"], "EMPTY")
                self.assertEqual(os.environ["LME_LOCAL_JUDGE_TOKEN"], "EMPTY")

            harness = SimpleNamespace(main=main, call_reader_model_async=None,
                                      generate_all_reader_outputs=None, score_prediction=None)
            with patch.dict(os.environ, {"OPENAI_API_KEY": "ambient-cloud-key"}, clear=True):
                run_observed(harness, [], ["a"], Path(directory))

    def test_observed_transport_keeps_finish_reason_and_missing_usage_without_changing_score(self):
        from evals.longmemeval_v2.inference_attempts import ACTIVE_CALL, AttemptRecorder
        with TemporaryDirectory() as directory:
            output = Path(directory)
            attempts = AttemptRecorder(output, max_retries=0, timeout_seconds=1)

            async def reader(_client, _args, _messages):
                self.assertEqual(ACTIVE_CALL.get(), ('a', 'reader'))
                await attempts.call_async('a', 'reader', completed)
                return 'answer', {'total_tokens': 0}

            async def completed():
                return {'choices': [{'message':{'content':'answer'}, 'finish_reason': 'length'}]}

            async def generate(args, rows):
                await harness.call_reader_model_async(None, args, rows[0]['messages'])

            def main():
                asyncio.run(harness.generate_all_reader_outputs(None, [
                    {'question_id': 'a', 'messages': [], 'memory_context': []},
                ]))
                (output / 'per_question.jsonl').write_text('{"question_id":"a","score":1}\n')

            harness = SimpleNamespace(main=main, call_reader_model_async=reader,
                                      generate_all_reader_outputs=generate,
                                      score_prediction=None, LLM_EVAL_FUNCTIONS=set())
            self.assertEqual(run_observed(harness, [], ['a'], output, attempts=attempts), 0)
            captured = json.loads((output / 'reader_outputs.jsonl').read_text())
            self.assertEqual(captured['finish_reason'], 'length')
            self.assertIsNone(captured['usage'])
            self.assertIsNone(ACTIVE_CALL.get())

    def test_complete_seven_case_artifact_replays_original_three_correct(self):
        # Saved native output is an evaluator input: replay, not a new model run.
        with TemporaryDirectory() as directory:
            output = Path(directory)
            def main():
                rows = [{'question_id':str(i),'score':int(i < 3)} for i in range(7)]
                (output/'per_question.jsonl').write_text(''.join(json.dumps(r)+'\n' for r in rows))
                (output/'aggregated_metrics.json').write_text(json.dumps({'overall':{
                    'count_all_questions':7,'overall_full_set':3/7}}))
            harness = SimpleNamespace(main=main, call_reader_model_async=None,
                                      generate_all_reader_outputs=None, score_prediction=None)
            self.assertEqual(run_observed(harness, [], list(map(str,range(7))), output), 0)
            report = json.loads((output/'score-coverage.json').read_text())
            self.assertEqual(report['coverage_adjusted_success_rate'], 3/7)
            self.assertEqual(report['covered_case_count'], 7)
            self.assertTrue(report['official_overall_recomputed_matches'])
