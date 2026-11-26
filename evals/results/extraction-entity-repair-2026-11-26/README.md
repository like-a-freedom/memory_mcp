# Source-reviewed extraction entity repair

The default Anno extraction path now keeps numeric/temporal annotations as values instead of converting their labels to durable `concept` identities. An occurrence of an isolated uppercase label immediately connected to the nearest typed numeric annotation by `is` or `grew to` is treated as a measurement dimension. Admission uses each occurrence's source span, not a blacklist of metric/currency names. A separate named-actor occurrence of the same spelling still establishes an entity. The persisted extractor fingerprint is `anno|identity-admission=v2`.

## Label review before prediction inspection

`docs/evals/EXTRACTION_ENTITY_LABEL_POLICY_V2.md` records the source-based decision made before this task inspected backend predictions or ran new evaluation. The earlier audit's predictions were already known, so the review is explicitly not blinded. In `Subject: Atlas follow-up`, Atlas denotes the named project/topic. The complete v1 expected entity list omitted it. Revision 2 adds Atlas to ext-009 and makes no other label/content/warning change. ARR/MRR in the source sentences are measures, and monetary amounts are their values; the named people remain entities. Names containing digits are not rejected by shape.

The original `crates/eval-harness/tests/fixtures/extraction_cases.json` and previous raw receipts are untouched. `extraction_cases_v2.json` is the new reviewed development corpus and the default suite input. The same revision-2 corpus is used on both sides below, separating the production repair from the annotation correction.

## Matched native results

| Measurement | Before production repair, v2 | After production repair, v2 |
| --- | --- | --- |
| Cases passed / total | 5 / 9 | 9 / 9 |
| Quality failed / invalid | 4 / 0 | 0 / 0 |
| Entity TP / FP / FN | 13 / 7 / 0 | 13 / 0 / 0 |
| Precision | 0.65 | 1.00 |
| Recall | 1.00 | 1.00 |
| F1 | 0.787878787878788 | 1.00 |
| Run verdict | quality_failed | passed |
| Duration | 2.106 seconds | 3.148 seconds |

The frozen v1 historical result remains 4/9 and precision .60/F1 .75. Moving that historical result to v2 labels would change the denominator and Atlas verdict; it is not the matched production comparison. No thresholds were weakened: the existing .70 F1 floor and .10 regression budget remain unchanged. Latency observations are single scoped runs, not a performance comparison.

Both runs used native `macos/aarch64`, rustc 1.100.0, the production extraction implementation through `ExtractionSuite`/`EvalSuite`, and real isolated in-memory formation. No model calls, downloads, dependency changes, migration changes or retrieval implementation changes were made for this repair.

```sh
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json \
  --suite extraction --baseline evals/results/extraction-closed-world-2026-11-26.json \
  --artifact evals/results/extraction-entity-repair-2026-11-26/before-v2.json
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json \
  --suite extraction --baseline evals/results/extraction-entity-repair-2026-11-26/before-v2.json \
  --artifact evals/results/extraction-entity-repair-2026-11-26/after-v2.json
```

The first command executed before the production change and returned exit 1 with real quality failures. The second returned exit 0. Both JSON receipts and console logs remain in this directory. `identity.json` hashes the source snapshots, receipts, old/new fixture inputs, grader, label policy, lockfile and profile. `anno.before.rs` and `anno.after.rs` retain the concrete production source delta; HEAD alone does not identify this modified worktree.

## Public-boundary TDD

The following integration commands were each observed RED before their corresponding production change, then GREEN afterward:

```sh
rtk cargo test -p memory_mcp --test extract_quality_e2e automatic_currency_amount_does_not_create_an_entity -- --exact
rtk cargo test -p memory_mcp --test extract_quality_e2e automatic_reported_metric_dimension_does_not_create_an_entity -- --exact
rtk cargo test -p memory_mcp --test extract_quality_e2e automatic_growing_metric_dimension_does_not_create_an_entity -- --exact
```

The observed failures were a durable `$7M` concept, an ARR company alongside the expected Alice Smith, and an ARR company in the growing-measure sentence. Tests use real public ingest/extract, not private helpers or mocks. Each names the production break it catches.

Independent retained controls cover Atlas in the email subject, Studio 54, 3M, NASA as a reporting actor, manually resolved `$7M`, existing scalar `95%` manual resolution, multilingual names, and a previously unregistered dimension/currency (`GTV is €42.50`). A separate `ARR is $3M. ARR announced a launch.` control proves that a name's measurement occurrence does not globally blacklist its actor occurrence. Existing public suite grading tests now use an intentionally incomplete expectation for a clearly named person, preserving their false-positive-rejection purpose after this production defect is repaired.

Scoped validation:

```sh
rtk cargo test -p memory_mcp --test extract_quality_e2e
rtk cargo test -p memory_mcp --lib knowledge::entity_extraction::anno
rtk cargo test -p eval-harness --test extraction_suite_contract
```

Final scoped results: public extraction E2E 26 passed / 2 intentionally ignored; Anno module 22 passed; public suite contract 7 passed. Root coordinates full package/workspace testing, strict clippy and fmt for the combined repairs.

The combined native production run subsequently caught the intentionally changed extractor identity in the public `extractor_fingerprint_projection` regression: actual `anno|identity-admission=v2|candidate-policy=scalar-grammar-v1`, expected the old token without `identity-admission=v2`. After that observed RED, the explicit expected version token was updated (not derived from the implementation). `rtk cargo test -p memory_mcp --test service_integration extractor_fingerprint_projection` passed one test, 50 filtered out. This is a test-contract update; it changes no production behavior or archived evaluation receipt.

## Boundaries

This repairs the shipped default Anno adapter's promotion of typed values and the demonstrated English scalar-predicate patterns. It is a bounded heuristic, not complete semantic entity classification across all languages or predicates. Other providers retain their own label contracts. A named organization used in an unusual direct numeric copula can remain ambiguous; the repaired examples and controls do not establish open-world precision. Nine reviewed development fixtures and these regressions do not support a population-level accuracy claim. Existing stored entities are not deleted or relabeled; only new automatic projections use the corrected candidate admission and versioned fingerprint.
