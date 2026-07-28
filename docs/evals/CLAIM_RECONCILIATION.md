# Claim Reconciliation Evaluation

## Metrics

| Metric | Description | Success Criteria |
|--------|-------------|-----------------|
| Projection precision | Fraction of projected claims that participate in a relation | > 80% |
| Contradiction recall | Fraction of seeded contradictions detected by the 8-gate engine | > 90% |
| Supersession recall | Fraction of seeded supersession pairs with correct outcome label | > 85% |
| Temporal ambiguity rate | Fraction of valid-time incomparable pairs that receive `TemporalAmbiguity` | < 5% false negatives |
| Projection latency (p50) | Time to extract claims from a fact payload | < 50 ms |
| Reconciliation latency (p99) | Time to evaluate one candidate page | < 500 ms |
| Invalidated relation count | Relations marked `t_invalid_ingested` after source-fact invalidation | = count of active relations before invalidation |

## Corpus

### Sources

| Split | Origin | Count | Description |
|-------|--------|-------|-------------|
| `development` | `anonymized_real` | 20 | Anonymized source samples from production |
| `development` | `synthetic_adversarial` | 15 | Edge cases: missing keys, set values, overlapping validity |
| `test` | `synthetic_adversarial` | 10 | Held-out adversarial cases |
| `test` | `external_public` | 5 | Public-domain knowledge-base snippets |

### Schema Coverage

Each case exercises at least one of the four built-in schemas:

- `AttributeV1` — `dimension=value` pairs (e.g. `Height=180`)
- `QuantityV1` — `measure=value unit` triples (e.g. `Weight=75 kg`)
- `RelationV1` — `subject predicate object` links
- `CommitmentV1` — promise/obligation statements

## Evaluation Runner

Claim reconciliation is evaluated as part of the `eval-harness` profile-driven
system. The harness implements a `ClaimReconciliationSuite` that runs all
fixture cases and reports per-split confusion counts and case outcomes.

```bash
# Run claim reconciliation as part of the PR profile
make eval-pr

# Run claim reconciliation suite directly
cargo test -p eval-harness suites::claims

# Run the thin compatibility launcher
cargo test --test eval_claim_reconciliation
```

## Promotion Gate

The default rollout stage is `Shadow` (projects claims but does not expose relations in `assemble_context`). Promotion to `Evidence` requires:

1. All `development` split cases pass with precision ≥ 80 % and recall ≥ 85 %
2. All `test` split cases pass with precision ≥ 75 % and recall ≥ 80 %
3. Held-out adversarial cases produce zero silent false positives (contradictions where none exist)
4. Projection latency p50 < 50 ms over 100 repeated projections
5. Reconciliation latency p99 < 500 ms over 100 repeated pages

## Test Fixtures

Fixture files live in `tests/fixtures/claim_reconciliation/` as YAML with the schema defined by `ClaimCase` in `tests/eval_claim_reconciliation.rs`. To add a new case:

1. Choose `origin` (`anonymized_real`, `external_public`, `synthetic_adversarial`)
2. Place it in the correct `split` (`development` or `test`)
3. Define `setup` (pre-existing facts), `source` (the fact being extracted), and `expected` (anticipated claims and relations)
4. List `coverage` tags matching schema families exercised

## Bi-temporal Assertions

The eval framework verifies that:

- Every expected relation has a matching `ClaimRelation` record with the correct `outcome`
- `t_invalid_ingested` is `NONE` for active relations
- After source-fact invalidation, the corresponding relation `t_invalid_ingested` is set
- Facts with disjoint validity ranges produce `TemporalAmbiguity` or `Coexist`, never `Contradiction`
