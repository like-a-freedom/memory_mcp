# Claim Reconciliation Evaluation

## Metrics

| Metric | Description | Success Criteria |
|--------|-------------|-----------------|
| Projection precision | Fraction of projected claims that participate in a relation | > 80% |
| Contradiction recall | Fraction of seeded contradictions detected by the claim reconciliation suite (gate metric `claim_recall`) | > 90% |
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

# Run the claim reconciliation suite unit tests (parsing, schema, reducer)
cargo test -p eval-harness suites::claims

# Run the thin compatibility launcher
cargo test -p eval-harness --test eval_claim_reconciliation
```

## Persisted Evidence Metric Contract

The suite scores each case against what the claim worker **actually stored**,
not only the in-process extraction warnings. This section fixes the contract
so denominators and pass rules are reproducible.

### Corpus

- Corpus version: `claim-reconciliation/v1`
- Fixture: `crates/eval-harness/tests/fixtures/claim_reconciliation_cases.json`
- Splits: `development` (31 cases), `test` (10 cases) — 41 namespaced outcomes
- Cases carrying expected relations: 14 (8 cross-lineage, 6 self-lineage)

### Evidence seam

Persisted relations are read through the feature-gated, read-only seam
`memory_mcp::eval_support::ClaimEvidenceReader` (feature `eval-support`,
disabled by default). It exposes immutable views over the `claim_relations`
table only — no SurrealDB queries, no mutation, never reachable through MCP.
Rows missing either source fact ID (pre-migration rows) are skipped.

### Lineage mapping

Identity is exact source lineage, never source-ID substrings or warning
content. During setup and source ingestion the suite records
`source_id -> fact_ids` from `ExtractResult.facts[].fact_id`. An expected
relation matches a persisted relation only when the persisted
`left_fact_id`/`right_fact_id` pair maps onto the expected
`setup_source_id`/`source_id` fact sets (the reversed pair is accepted because
persisted relations are unordered) **and** the outcomes agree.

### Positive outcomes and the self-lineage rule

Expected relations split by persistability:

- **Cross-lineage** (`setup_source_id != source_id`): gate persisted quality.
  A case passes persisted quality only when every cross-lineage expected
  relation is matched and the persisted count equals the cross-lineage count.
- **Self-lineage** (`setup_source_id == source_id`): never persisted by
  design. Production's source gate (ADR-0008) refuses automatic
  correction/supersession within a single source lineage, so these relations
  cannot materialize. They are scored through the in-process warning path and
  reported in the diagnostic `self_lineage_relations` metric; they do not
  gate persisted quality.

### Negative boundaries (isolation)

A violation is counted only when a persisted relation crosses the Active
Namespace or the policy fingerprint. Different fact IDs inside a valid
same-boundary relation are expected and are not violations. Missing boundary
metadata makes the case `invalid`.

### Confusion matrix and gate metrics

Gate metrics `claim_precision` and `claim_recall` are computed by the reducer
over the warning-based classification evidence aggregated across all cases
(per ADR-0025 they are not rendered per-case). Per-case warning counts
(`expected_contradictions`, `matched_warnings`, `predicted_warnings`) are
**diagnostic only**: they explain why a case failed and never share a formula
with a gate metric. The persisted gate (`persisted_quality`) is a separate
per-case pass condition layered on top of the warning gate.

Per-case diagnostic metrics: `persisted_relations`,
`matched_persisted_relations`, `self_lineage_relations`,
`isolation_violations`, `unresolved_lineage`.

## Promotion Gate

The default rollout stage is `Shadow` (projects claims but does not expose relations in `assemble_context`). Promotion to `Evidence` requires:

1. All `development` split cases pass with precision ≥ 80 % and recall ≥ 85 %
2. All `test` split cases pass with precision ≥ 75 % and recall ≥ 80 %
3. Held-out adversarial cases produce zero silent false positives (contradictions where none exist)
4. Projection latency p50 < 50 ms over 100 repeated projections
5. Reconciliation latency p99 < 500 ms over 100 repeated pages

## Test Fixtures

Fixture files live in `crates/eval-harness/tests/fixtures/claim_reconciliation_cases.json` as a single JSON corpus; the case schema is defined by `ClaimCase` in `crates/eval-harness/tests/eval_claim_reconciliation.rs`. To add a new case:

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
