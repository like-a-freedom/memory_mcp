# Current-master v9 claim rollout replay

**Result: accepted on the frozen v9 deterministic criteria.** The current
product binary was paired with itself under the default `evidence` rollout
stage and the explicit `shadow` control. Each arm was captured twice over the
same eight registered synthetic history families. Both repeats matched within
each arm.

| Metric | Shadow | Product default (`evidence`) |
|---|---:|---:|
| Evidence success | 8/8 | 8/8 |
| Decision success | 1/8 | 5/8 |

There were four decision gains (`peer-conflict`, `prior-knowledge`,
`unverified-correction`, `valid-boundary`), no decision losses, no evidence
regressions and no hard failures. The equal-family gain is 50 percentage
points; the descriptive 95% family interval is [12.5, 87.5] points (8
clusters, 10,000 draws, seed 20261121). The frozen acceptance report records
`accepted: true`.

The product executable SHA-256 is
`572607d17e12b990f06e199f5574706438b8c9e65a484b11fe3c97a2b38cf322`; both
arms used this same executable. The public tool input-schema SHA-256 is
`9ecfa559499e708eb95b7d2fba953218029a970a6ecb1b9edc533b79bb36c0a9` for both
arms. The result verifies the claim-reconciliation `evidence` rollout default
on the registered current-master cohort. It says nothing about whether the
separate `extract.include_fact_evidence` response flag should become
default-on.

## Schema compatibility note

The historical v4 memory-quality baseline is pinned to input-schema SHA
`7b8c60a7d6bdc5d50a71b04e5d2d3dc514516dd69b46627f5b0481cd41c20e61`. The
current schema adds the reviewed optional `extract.include_fact_evidence`
request field and opt-in response fields; the additive change is recorded in
[`schema-change.json`](../extract-fact-evidence-v1/schema-change.json). The
generic acceptance scorer correctly rejects comparisons across those schema
fingerprints. An initial current-gate attempt is preserved in
[`claim-rollout-current-master-2026-11-23/`](../claim-rollout-current-master-2026-11-23/);
it stopped before v9 scoring at that compatibility guard. The v9 replay here
compares only current-schema captures, and existing public extract tests cover
the legacy compact request/response behavior.

## Artifacts

- [Default capture 1](rollout-v9-default-first/)
- [Default capture 2](rollout-v9-default-second/)
- [Shadow capture 1](rollout-v9-shadow-first/)
- [Shadow capture 2](rollout-v9-shadow-second/)
- [Frozen-rules acceptance report](claim-rollout-default-acceptance.json)
