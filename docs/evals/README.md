# Evaluation profiles

Every committed evaluation is run through a profile. The profile fixes the
selected suites, expected coverage, time budget, gates, and (where applicable)
the corpus root. Schema-v2 artifacts reject missing, duplicate, or unexpected
suite-scoped cases.

| Profile | Command | Scope | Status |
|---|---|---|---|
| `pr` | `cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json --artifact target/evals/pr.json` | local retrieval, extraction, claims, external mini-corpus | blocking |
| `release` | `cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/release.json --artifact target/evals/release.json` | PR suites plus lifecycle checks | blocking |
| `nightly` | `cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/nightly.json --artifact target/evals/nightly.json` | scheduled breadth | blocking when scheduled |
| `response_size` | `make eval-response-size` | compact vs verbose `assemble_context` and `explain` serialization | diagnostic; execution integrity blocking |
| `ner_quality` | `make eval-ner-quality` | five extractor backends on the committed fixture | manual/platform-aware |
| `perf_weekly` | `make perf-weekly` | ingest/assemble latency and throughput gates vs the committed baseline | manual, local machine only |
| `external_*` | `cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/external_<name>.json --artifact target/evals/external_<name>.json` | one pinned classic corpus per profile | requires prepared corpus |

The `pr`, `release`, `nightly`, and `external_*` profiles have no Make
target — run the `cargo run` command above directly. CI inlines the same
invocation.

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

## Perf

Perf gates live outside CI by design (ADR-0082): `make perf-weekly` compares
the weekly profile against
[`evals/perf/baselines/perf-weekly-local.json`](../../evals/perf/baselines/perf-weekly-local.json)
on the maintainer's local machine, and dated artifacts are archived under
[`evals/perf/results/`](../../evals/perf/results/). Commands, the host-tag
policy, artifact layout, and the baseline-update ritual are documented in
[`docs/performance/PERF_HARNESS.md`](../performance/PERF_HARNESS.md).
