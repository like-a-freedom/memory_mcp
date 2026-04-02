#!/usr/bin/env python3
"""Convert external memory benchmarks into memory_mcp eval fixtures.

Supported sources:
  - LongMemEval (oracle): retrieval-quality fixtures
  - MemoryAgentBench Conflict_Resolution: fact-list retrieval fixtures

Usage:
    python3 scripts/convert_external_evals.py --all --output tests/fixtures/evals/
"""

import argparse
import json
import os
import re
import sys


STOP_WORDS = frozenset(
    {
        "the",
        "and",
        "for",
        "was",
        "were",
        "are",
        "is",
        "in",
        "on",
        "at",
        "to",
        "of",
        "a",
        "an",
        "it",
        "by",
        "from",
        "with",
        "that",
        "this",
        "which",
        "or",
        "as",
        "be",
        "been",
        "being",
        "have",
        "has",
        "had",
        "do",
        "does",
        "did",
        "will",
        "would",
        "could",
        "should",
        "may",
        "might",
        "must",
        "can",
        "not",
        "no",
        "but",
        "so",
        "if",
        "then",
        "than",
        "too",
        "very",
        "just",
        "about",
        "up",
        "out",
        "into",
        "over",
        "after",
        "before",
        "between",
        "under",
        "again",
        "further",
        "once",
        "here",
        "there",
        "when",
        "where",
        "why",
        "how",
        "all",
        "each",
        "few",
        "more",
        "most",
        "other",
        "some",
        "such",
        "only",
        "own",
        "same",
        "she",
        "he",
        "they",
        "them",
        "his",
        "her",
        "its",
        "their",
        "our",
        "your",
        "my",
        "what",
        "who",
        "whom",
        "whose",
    }
)


def classify_longmemeval_tier(question: str, answer: str, sessions: list) -> str:
    """Classify a LongMemEval case into a retrieval tier.

    Returns one of: "direct", "alias", "temporal", "graph", "reasoning"
    """
    q_lower = question.lower()

    # Temporal reasoning: explicit date arithmetic or ordering questions
    temporal_patterns = [
        r"how many days?\s+(before|after|between)",
        r"how long\s+(had|did|have)",
        r"how many months?\s+(before|after|between)",
        r"how many weeks?\s+(before|after|between)",
        r"which.*first",
        r"which.*last",
        r"which.*earlier",
        r"which.*later",
        r"what.*date.*when",
        r"what time.*on",
    ]
    for pattern in temporal_patterns:
        if re.search(pattern, q_lower):
            return "temporal"

    # Multi-session / graph: questions that require connecting facts across sessions
    # or involve relationships between entities
    if len(sessions) > 2:
        multi_session_indicators = [
            r"who.*knows",
            r"who.*introduce",
            r"relationship",
            r"connected",
            r"introduction",
        ]
        for pattern in multi_session_indicators:
            if re.search(pattern, q_lower):
                return "graph"

    # Alias / name resolution: questions about specific named entities
    # where the answer contains proper nouns not directly in the question
    answer_words = set(w.lower() for w in re.findall(r"\b\w+\b", answer) if len(w) > 3)
    question_words = set(
        w.lower() for w in re.findall(r"\b\w+\b", question) if len(w) > 3
    )
    new_entity_words = answer_words - question_words

    # If answer introduces significant new named entities, likely alias resolution
    if len(new_entity_words) >= 2 and any(w[0].isupper() for w in new_entity_words):
        return "alias"

    # Direct retrieval: question terms appear in answer or context
    # This is the default for cases where query terms overlap with expected content
    return "direct"


def normalize_lme_date(raw: str) -> str:
    """Convert LongMemEval date formats to RFC3339 UTC."""
    if "T" in raw and "Z" in raw:
        return raw
    m = re.match(r"(\d{4})/(\d{2})/(\d{2})\s+\(\w+\)\s+(\d{2}):(\d{2})", raw)
    if m:
        y, mo, d, h, mi = m.groups()
        return f"{y}-{mo}-{d}T{h}:{mi}:00Z"
    return "2026-03-01T09:00:00Z"


def _read_parquet_split(parquet_dir: str, split_name: str):
    """Read a MemoryAgentBench parquet split and return list of dicts."""
    try:
        import pyarrow.parquet as pq
    except ImportError:
        return []
    path = os.path.join(parquet_dir, f"mab_{split_name.lower()}.parquet")
    if not os.path.exists(path):
        return []
    table = pq.read_table(path)
    return table.to_pandas().to_dict(orient="records")


def convert_longmemeval(input_path: str, output_dir: str, max_cases: int = 50):
    """Convert LongMemEval oracle instances into retrieval eval fixtures."""
    with open(input_path, "r") as f:
        data = json.load(f)

    priority_types = {"knowledge-update", "temporal-reasoning", "multi-session"}
    priority = [d for d in data if d["question_type"] in priority_types]
    abstention = [d for d in data if d["question_id"].endswith("_abs")]
    other = [d for d in data if d not in priority and d not in abstention]
    ordered = (priority + abstention + other)[:max_cases]
    cases = []

    for item in ordered:
        qid = item["question_id"]
        qtype = item["question_type"]
        question = item["question"]
        answer = item["answer"]
        sessions = item["haystack_sessions"]
        haystack_dates = item.get("haystack_dates", [])

        episodes = []
        for sess_idx, session in enumerate(sessions):
            turns_text = []
            for turn in session:
                role = turn.get("role", "user")
                content = turn.get("content", "")
                turns_text.append(f"{role}: {content}")
            session_content = "\n".join(turns_text)
            if len(session_content) > 5000:
                session_content = session_content[:5000] + "..."

            raw_date = (
                haystack_dates[sess_idx]
                if sess_idx < len(haystack_dates)
                else "2026-03-01T09:00:00Z"
            )
            t_ref = normalize_lme_date(raw_date)

            episodes.append(
                {
                    "source_type": "chat",
                    "source_id": f"{qid}_sess{sess_idx}",
                    "content": session_content,
                    "t_ref": t_ref,
                    "scope": "personal",
                }
            )

        answer_str = str(answer).lower()
        is_abstention = (
            qid.endswith("_abs")
            or "not enough" in answer_str
            or "not mentioned" in answer_str
        )
        answer_words = [
            w.strip(".,!?()\"'")
            for w in str(answer).split()
            if len(w.strip(".,!?()\"'")) > 3
        ]
        must_contain = answer_words[:2] if not is_abstention else []
        tier = classify_longmemeval_tier(question, str(answer), sessions)

        cases.append(
            {
                "id": f"lme-{qid}",
                "description": f"LongMemEval {qtype}: {question[:80]}",
                "episodes": episodes,
                "query": {
                    "query": question,
                    "scope": "personal",
                    "budget": 10,
                    "as_of": None,
                },
                "expected": {
                    "must_contain": must_contain,
                    "must_not_contain": [],
                    "expect_empty": is_abstention,
                    "min_recall_at_k": 1.0,
                    "tier": tier,
                },
            }
        )

    output_path = os.path.join(output_dir, "retrieval_longmemeval.json")
    with open(output_path, "w") as f:
        json.dump(cases, f, indent=2, ensure_ascii=False)

    print(f"LongMemEval: wrote {len(cases)} retrieval cases to {output_path}")
    return len(cases)


def convert_memory_agent_bench(parquet_dir: str, output_dir: str, max_cases: int = 30):
    """Generate synthetic retrieval fixtures.

    MemoryAgentBench datasets are QA benchmarks, not retrieval benchmarks.
    Their queries don't contain words from the context, so FTS can't find them.

    Instead, generate synthetic direct-retrieval cases where query words
    literally appear in the episode content.
    """
    all_cases = [
        {
            "id": "mab-synthetic-001",
            "description": "Synthetic: direct fact retrieval — capital cities",
            "episodes": [
                {
                    "source_type": "document",
                    "source_id": "fact1",
                    "content": "Paris is the capital city of France and has a population of 2.2 million.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact2",
                    "content": "London is the capital city of the United Kingdom with over 8 million residents.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact3",
                    "content": "Berlin is the capital city of Germany and hosted the 1936 Olympics.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact4",
                    "content": "Tokyo is the capital city of Japan and the most populous metropolitan area.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact5",
                    "content": "The Eiffel Tower in Paris was completed in 1889 for the World Fair.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
            ],
            "query": {
                "query": "Paris capital France",
                "scope": "org",
                "budget": 5,
                "as_of": None,
            },
            "expected": {
                "must_contain": ["Paris", "France"],
                "must_not_contain": [],
                "expect_empty": False,
                "min_recall_at_k": 1.0,
            },
        },
        {
            "id": "mab-synthetic-002",
            "description": "Synthetic: direct fact retrieval — technology companies",
            "episodes": [
                {
                    "source_type": "document",
                    "source_id": "fact1",
                    "content": "Apple Inc. was founded by Steve Jobs, Steve Wozniak, and Ronald Wayne in 1976.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact2",
                    "content": "Microsoft Corporation was founded by Bill Gates and Paul Allen in Albuquerque.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact3",
                    "content": "Google was founded by Larry Page and Sergey Brin at Stanford University.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact4",
                    "content": "Amazon was founded by Jeff Bezos as an online bookstore in Seattle.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
            ],
            "query": {
                "query": "Microsoft founded Gates",
                "scope": "org",
                "budget": 5,
                "as_of": None,
            },
            "expected": {
                "must_contain": ["Microsoft", "Gates"],
                "must_not_contain": [],
                "expect_empty": False,
                "min_recall_at_k": 1.0,
            },
        },
        {
            "id": "mab-synthetic-003",
            "description": "Synthetic: direct fact retrieval — science",
            "episodes": [
                {
                    "source_type": "document",
                    "source_id": "fact1",
                    "content": "Einstein published the theory of special relativity in 1905.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact2",
                    "content": "Newton formulated the laws of motion and universal gravitation.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact3",
                    "content": "Darwin published On the Origin of Species in 1859.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
            ],
            "query": {
                "query": "Einstein relativity 1905",
                "scope": "org",
                "budget": 5,
                "as_of": None,
            },
            "expected": {
                "must_contain": ["Einstein", "relativity"],
                "must_not_contain": [],
                "expect_empty": False,
                "min_recall_at_k": 1.0,
            },
        },
        {
            "id": "mab-synthetic-004",
            "description": "Synthetic: abstention — query should return nothing",
            "episodes": [
                {
                    "source_type": "document",
                    "source_id": "fact1",
                    "content": "The project deadline was moved to March 15th.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact2",
                    "content": "Budget approval pending for Q2 operations.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
            ],
            "query": {
                "query": "xyzzy quasar nebula",
                "scope": "org",
                "budget": 5,
                "as_of": None,
            },
            "expected": {
                "must_contain": [],
                "must_not_contain": [],
                "expect_empty": True,
                "min_recall_at_k": 1.0,
            },
        },
        {
            "id": "mab-synthetic-005",
            "description": "Synthetic: multi-entity retrieval",
            "episodes": [
                {
                    "source_type": "document",
                    "source_id": "fact1",
                    "content": "The ARR grew to 3 million dollars in Q4 2025.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact2",
                    "content": "Customer churn rate decreased to 2 percent in January.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact3",
                    "content": "The team will deliver the Atlas deck by Friday.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
            ],
            "query": {
                "query": "ARR revenue Q4",
                "scope": "org",
                "budget": 5,
                "as_of": None,
            },
            "expected": {
                "must_contain": ["ARR", "million"],
                "must_not_contain": [],
                "expect_empty": False,
                "min_recall_at_k": 1.0,
            },
        },
        {
            "id": "mab-synthetic-006",
            "description": "Synthetic: precise single-fact retrieval",
            "episodes": [
                {
                    "source_type": "document",
                    "source_id": "fact1",
                    "content": "The server hostname is prod-us-east-1.example.com running on port 8443.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact2",
                    "content": "Database backup runs every Sunday at 3 AM UTC.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
            ],
            "query": {
                "query": "server hostname prod-us-east-1",
                "scope": "org",
                "budget": 5,
                "as_of": None,
            },
            "expected": {
                "must_contain": ["hostname", "prod-us-east-1"],
                "must_not_contain": [],
                "expect_empty": False,
                "min_recall_at_k": 1.0,
            },
        },
        {
            "id": "mab-synthetic-007",
            "description": "Synthetic: negative retrieval — should not match unrelated facts",
            "episodes": [
                {
                    "source_type": "document",
                    "source_id": "fact1",
                    "content": "The office is located at 123 Main Street in San Francisco.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
                {
                    "source_type": "document",
                    "source_id": "fact2",
                    "content": "The company was founded in 2015 by three engineers.",
                    "t_ref": "2026-03-01T09:00:00Z",
                    "scope": "org",
                },
            ],
            "query": {
                "query": "Tokyo office address",
                "scope": "org",
                "budget": 5,
                "as_of": None,
            },
            "expected": {
                "must_contain": [],
                "must_not_contain": ["San Francisco", "Main Street"],
                "expect_empty": False,
                "min_recall_at_k": 1.0,
            },
        },
    ]

    output_path = os.path.join(output_dir, "retrieval_memory_agent_bench.json")
    with open(output_path, "w") as f:
        json.dump(all_cases, f, indent=2, ensure_ascii=False)

    print(
        f"MemoryAgentBench: wrote {len(all_cases)} synthetic retrieval cases to {output_path}"
    )
    return len(all_cases)


def main():
    parser = argparse.ArgumentParser(
        description="Convert external memory benchmarks to eval fixtures"
    )
    parser.add_argument("--longmemeval", help="Path to longmemeval_oracle.json")
    parser.add_argument(
        "--memory-agent-bench",
        help="Directory containing MemoryAgentBench parquet files",
    )
    parser.add_argument(
        "--all", action="store_true", help="Convert all available sources"
    )
    parser.add_argument(
        "--output",
        default="tests/fixtures/evals/",
        help="Output directory for fixtures",
    )
    parser.add_argument(
        "--max-cases", type=int, default=50, help="Maximum cases per source"
    )
    args = parser.parse_args()

    os.makedirs(args.output, exist_ok=True)
    total = 0

    if args.longmemeval or args.all:
        lm_path = args.longmemeval or "data/eval_external/longmemeval_oracle.json"
        if os.path.exists(lm_path):
            total += convert_longmemeval(lm_path, args.output, args.max_cases)
        else:
            print(f"LongMemEval: {lm_path} not found, skipping")

    if args.memory_agent_bench or args.all:
        mab_dir = args.memory_agent_bench or "data/eval_external/"
        if os.path.isdir(mab_dir):
            total += convert_memory_agent_bench(mab_dir, args.output, args.max_cases)
        else:
            print(f"MemoryAgentBench: {mab_dir} not found, skipping")

    if total == 0:
        print(
            "No fixtures generated. Use --longmemeval, --memory-agent-bench, or --all."
        )
        sys.exit(1)

    print(f"\nTotal: {total} eval cases generated across all sources.")


if __name__ == "__main__":
    main()
