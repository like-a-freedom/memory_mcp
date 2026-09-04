# Evaluation profiles

Every committed evaluation is run through a profile. The profile fixes the
selected suites, expected coverage, time budget, gates, and (where applicable)
the corpus root. Schema-v2 artifacts reject missing, duplicate, or unexpected
suite-scoped cases.

| Profile | Command | Scope | Status |
|---|---|---|---|
| `pr` | `make eval-pr` | local retrieval, extraction, claims, external mini-corpus | blocking |
| `release` | `make eval-release` | PR suites plus lifecycle checks | blocking |
| `nightly` | `make eval-nightly` | scheduled breadth | blocking when scheduled |
| `response_size` | `make eval-response-size` | compact vs verbose `assemble_context` and `explain` serialization | diagnostic; execution integrity blocking |
| `ner_quality` | `make eval-ner-quality` | five extractor backends on the committed fixture | manual/platform-aware |
| `external_*` | `make eval-external-*` | one pinned classic corpus per profile | requires prepared corpus |

Response-size reports separate `assemble_*` and `explain_*` metrics. It is
intentionally gate-free: the design target is diagnostic, while missing or
empty response classes are invalid execution and fail the run.

## External corpora

The manifests in `evals/corpora/` pin URL, immutable revision, checksum, byte
size, row count, license, adapter version, and any auxiliary files required by
the adapter (PersonaMem uses its pinned CSV plus JSONL context file).
Preparation validates every file, stages atomically, and never overwrites a
previously valid revision. The external suite reports
`answer_presence_proxy_at_5`, a weak lexical diagnostic; it is not document
recall or answer correctness. A corpus-backed benchmark is not release
evidence until its pinned data has been prepared and its artifact is available.

## Platform benchmarks

`make bench-check` compiles all Criterion targets. `make bench-cpu` measures
CPU targets and requires model fixtures. `make bench-metal` is only meaningful
on Apple Silicon; set `MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1` to fail closed when
the Metal model asset is absent.

LongMemEval-V2 remains text-only and uses the pinned adapter in
`evals/longmemeval_v2/`. Run `run_pinned.sh --smoke-only` or
`--integration-smoke BINARY` for contract checks; these are not an official
leaderboard score. The script fails closed without an explicit mode, and the
official upstream launcher must be present before publishing such a result.

The current NER floors and their review rationale are recorded in
[`NER_BASELINE_REVIEW_2026-09-05.md`](NER_BASELINE_REVIEW_2026-09-05.md).
