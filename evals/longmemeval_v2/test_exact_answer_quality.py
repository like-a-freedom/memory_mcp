import unittest

from evals.longmemeval_v2.exact_answer_quality import (
    exact_phrase_set_match,
    score_exact_phrase_rows,
)


class ExactPhraseSetMatchTests(unittest.TestCase):
    def test_accepts_the_same_options_independent_of_order_and_punctuation(self):
        self.assertTrue(
            exact_phrase_set_match(
                "All time; Past month, Past week, Past year, Past 24 hours",
                "Past 24 hours, Past week, Past month, Past year, All time",
            )
        )

    def test_rejects_an_extra_option_even_when_all_gold_options_are_present(self):
        self.assertFalse(
            exact_phrase_set_match(
                "Past hour, Past 24 hours, Past week, Past month, Past year, All time",
                "Past 24 hours, Past week, Past month, Past year, All time",
            )
        )

    def test_rejects_a_missing_option(self):
        self.assertFalse(
            exact_phrase_set_match("Past 24 hours, Past week", "Past 24 hours, Past week, All time")
        )

    def test_rejects_empty_prediction_and_empty_reference(self):
        self.assertFalse(exact_phrase_set_match("", "All time"))
        self.assertFalse(exact_phrase_set_match("All time", ""))

    def test_scores_only_phrase_set_cases_and_counts_unscored_as_failures(self):
        report = score_exact_phrase_rows(
            [
                {
                    "question_id": "complete",
                    "eval_function": "norm_phrase_set_match|lower=true",
                    "answer_gold": "A, B",
                    "response_parsed_boxed": "A, B",
                    "status": "scored",
                },
                {
                    "question_id": "extra",
                    "eval_function": "norm_phrase_set_match|lower=true",
                    "answer_gold": "A, B",
                    "response_parsed_boxed": "A, B, C",
                    "status": "scored",
                },
                {
                    "question_id": "incomplete",
                    "eval_function": "norm_phrase_set_match|lower=true",
                    "answer_gold": "A, B",
                    "response_parsed_boxed": "A, B",
                    "status": "reader_incomplete",
                },
                {
                    "question_id": "judge",
                    "eval_function": "llm_abstention_checker|require_non_empty=true",
                    "answer_gold": "flawed premise",
                    "response_parsed_boxed": "UNKNOWN",
                    "status": "scored",
                },
            ]
        )

        self.assertEqual(report["selected"], 3)
        self.assertEqual(report["correct"], 1)
        self.assertEqual(report["failure_inclusive_accuracy"], 1 / 3)
        self.assertEqual(report["cases"]["incomplete"], {
            "status": "reader_incomplete", "score": False,
        })
        self.assertNotIn("judge", report["cases"])


if __name__ == "__main__":
    unittest.main()
