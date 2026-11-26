# Extraction expectation contract and native receipt

The extraction suite treats `expected.entities` and `expected.warnings` as complete, closed-world source labels. An omitted list means an empty list. Missing expected predictions and additional predictions fail the case. Entity identity is the set of exact canonical names; warnings match fact type, existing content and new content, each at most once. Duplicate actual warnings therefore count as extras. `expected.fact_types` remains required coverage, not an exhaustive inventory. This is a verdict correction; existing labels and profile thresholds are unchanged.

The nine project-maintained examples in `crates/eval-harness/tests/fixtures/extraction_cases.json` are reviewed development labels, not an official external dataset. This follows the truthful-evaluation specification's distinction between official dataset labels and project-maintained human-reviewed mappings. Their source inputs, expected names, and expected contradictions remain the fixture's provenance; changing extraction output does not authorize relabeling. The corpus is a small development diagnostic and provides no confirmatory generalization claim.

Every outcome retains `end_to_end`, `development`, `reviewed`, including setup failure, primary failure, fixture read/parse failure and deadline interruption. `ExtractionSuite::from_fixture_path` exposes an explicit file-input seam for controlled real formation scenarios without private scorer access. Fault tests use invalid public ingestion inputs and the public suite deadline.

## TDD evidence

Public boundary: `ExtractionSuite` implementing `EvalSuite`, with real in-memory public ingestion/extraction. No mocked private grading helper and no model downloads.

Each focal command was run RED before the corresponding production change and GREEN after it:

```sh
rtk cargo test -p eval-harness --test extraction_suite_contract complete_entity_expectation_rejects_false_positives -- --exact
rtk cargo test -p eval-harness --test extraction_suite_contract complete_warning_expectation_rejects_additional_contradictions -- --exact
rtk cargo test -p eval-harness --test extraction_suite_contract setup_fault_preserves_declared_development_population -- --exact
rtk cargo test -p eval-harness --test extraction_suite_contract deadline_retains_declared_reviewed_development_population -- --exact
```

Observed REDs: entity precision 0.0 with `Passed`; full expected warning recall with no extra-warning failure (two preceding ARR values and one expected contradiction); setup `RetrievalOnly` instead of `EndToEnd`; deadline `Test` instead of `Development`. The warning fixture seam initially required a constructor; an initial multi-metric exploratory control did not reproduce the required warning contract and was replaced by independently stated single-metric updates before the decisive behavioral RED. No production corpus labels were changed.

Final scoped command `rtk cargo test -p eval-harness --test extraction_suite_contract` passed 7/7, including no-prediction positive control, primary invalid input and corrupt fixture controls. Only owned files were formatted directly with rustfmt; root owns combined workspace lint and validation.

## Native run

```sh
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/pr.json --suite extraction \
  --baseline evals/results/2026-11-26-pr-local-candle/run.json \
  --artifact evals/results/extraction-closed-world-2026-11-26.json
```

On native `macos/aarch64`, rustc 1.100.0, this finished with exit 1 and verdict `quality_failed`: 9 cases, 4 passed, 5 quality-failed, 0 invalid, approximately 2.1 seconds. Entity precision is 0.60, recall 1.00, F1 0.75. The unchanged F1 floor 0.70 and baseline budget passed, but the real case failures make the run fail. This receipt is scoped to extraction; it is not a whole-PR run. An initial invocation without a baseline was rejected at preflight because the unchanged profile has regression budgets; it executed no cases.

| Case | Additional entity predictions |
| --- | --- |
| ext-001 | `$3M`, `ARR` |
| ext-003 | `$7M`, `ARR` |
| ext-004 | `$140K`, `MRR` |
| ext-005 | `$3M` |
| ext-009 | `Atlas` |

The `$` amounts/acronyms are entity false positives under the existing reviewed labels. `Atlas` remains an extra under the fixture's closed expectation; this report does not claim that the text makes every possible semantic entity annotation unambiguous. Any future label-policy revision requires independent source review and a new corpus identity, rather than copying current model output.

The run artifact embeds the fixture and grader source SHA-256, expectation policy and label trust. `evals/results/extraction-closed-world-2026-11-26.identity.json` records artifact SHA-256 and per-file input/source hashes, including the production Rust source tree, eval formation/grading modules, lockfile, profile and supplied baseline. The receipt was produced from modified worktree source on the recorded HEAD; the commit alone is not the complete source identity. Fingerprint model/provider fields are null because this local extraction profile does not configure an external model. No external provider was contacted.
