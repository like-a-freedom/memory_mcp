# Extract quality: production telemetry observation

**Status:** read-only aggregate snapshot; not a matched before/after evaluation.
**Evaluated:** 2026-11-25 18:00 UTC.
**Window:** 2026-11-20 18:00 UTC through 2026-11-25 18:00 UTC.
**Source:** VictoriaMetrics through `gcx`, context `grafana-pushkino`, datasource `edr4onptd6hogd`, filtered to `env="prod",job="memory_mcp"`.

## Observations

An instant query of `memory_build_info` at the window start returns
`version="1.41.0"`. This identifies the reported semantic version at that
timestamp; it does not attest the running image digest or source commit.
The metric implementation exposes only the Cargo package version, not a commit
or deployment/configuration fingerprint. The repository tag `v1.41.0` points to
`1ae1da4a4e3120f2de94753fc1a949caee3aced7`, but telemetry alone cannot prove
that exact artifact or configuration was running.

| Measure | Result in five-day window | Denominator / limitation |
| --- | ---: | --- |
| Extract operation outcomes | 2 success observations; no error outcome series returned | Only two observations; absence of an error series is not a population guarantee |
| Mean extract duration | 1.295 s | Two operation observations |
| Facts / entities / links | 28 / 54 / 54 | Aggregate result counters |
| Warnings | 0 | Warning count is not a false-relation label |
| `embedding_provider` stage | 1,776 observations; 27.19 ms mean | Stage observations are not extract requests |
| Reconciliation outcomes / candidate count | No series returned | Not evidence that either count is zero |
| Scrape availability (`up`) | 0.9990 average; minimum 0 | At least one failed scrape sample in the window |

## What this does and does not establish

The snapshot establishes two successful extract observations, zero recorded
warning outcomes, and a 1.295 s mean duration across those two observations.
There is no service-level latency target in this comparison. The sample does
not establish a semantic quality rate, entity recall, or that the local
extraction changes caused a production improvement.

The historical 2026-11-02 baseline recorded nine extract operations (seven
successes and two errors), a 19.51 s mean, 30,116 warning outcomes, and four
extraction-stage observations averaging 30.38 s. That is useful historical
context, but it is not a qualified comparison: its exact deployed artifact and
configuration are not attested here, the traffic denominators differ sharply,
and warning outcomes were not adjudicated as true or false relations.

Aggregate counters cannot determine whether a relation is false or whether an
entity was eligible but missed. Those require a frozen source-grounded oracle
and deterministic evaluation. A population claim about deployed semantic
quality additionally requires an approved, privacy-preserving sample with
reviewed source labels. The current metrics do not provide those labels.

## Claim telemetry contract audit

A separate read-only query at `2026-11-25T23:00:00Z` covered the preceding
22 days. It returned an estimated 229 project pipeline events, all labeled
`outcome="other"` (reason labels: 36 `ambiguous_subject`, 4 `missing_subject`,
1 `other`, 43 `unknown_cardinality`, and 145 `unstructured_colon_value`). It
also returned one quantity reconciliation page-duration observation labeled
`outcome="other"` and one matching candidate-count observation. The exact
aggregate responses are in the [telemetry audit receipt](extract-quality-telemetry-contract-audit-2026-11-26-query-receipt.json).

Source inspection found that the claim telemetry mapper accepted relation
decisions but collapsed the known processing statuses `persisted`, `skipped`,
`failed`, and `completed` to `other`. The page counter is emitted once per
persisted relation, while page duration is emitted even for an empty/skipped
page; therefore an absent reconciliation counter does not establish that no
page ran. Since the production metric reports only version `1.41.0`, without an
image digest or source revision, the aggregate is consistent with the mapper
defect but does not attest the exact deployed artifact.

The local fix gives `persisted`, `skipped` and `failed` distinct bounded
outcomes, replaces the backfill's misleading `completed` label with `ensured`,
and uses `reason_code="none"` when a successful event has no reason. Production
history cannot be reclassified after the fact. The fix requires a deployed
build and a fresh post-deployment window before these labels can qualify any
comparison; it does not establish a semantic-quality or latency improvement.
The same source audit found that the backfill counter increments when an
idempotent projection-job ensure succeeds for a fact with an identifier, not
when fact projection completes. Its local outcome is now `ensured`; the metric
help says it counts ensure attempts, including repeats, and not unique facts or
completed projections. The dashboard recording rule and panel now label this
as job-ensure attempt rate; they explicitly avoid treating it as worker
throughput.

## Reproduction queries

The following PromQL expressions were evaluated at the fixed window end,
2026-11-25 18:00 UTC. The five-day range begins at 2026-11-20 18:00 UTC, where
the instant version query reports 1.41.0.

The exact `gcx` query expressions, evaluation timestamps and aggregate JSON
responses are preserved in the [query receipt](extract-quality-production-observation-2026-11-25-query-receipt.json)
(SHA-256 `ecff02b3e464731526ea19d0b3b8bbc08d76d3c95a9cd40fa6de96999ac9a0dc`).
The receipt redacts the production instance label while retaining aggregate
values and query identities.
`increase()` values are estimates from sampled counters; scrape gaps and
range-edge extrapolation limit exact request counts. The minimum `up` value
was zero during this window.

```promql
sum by (outcome) (increase(memory_operation_duration_seconds_count{job="memory_mcp",env="prod",operation="extract"}[5d]))
sum(increase(memory_operation_duration_seconds_sum{job="memory_mcp",env="prod",operation="extract"}[5d])) / sum(increase(memory_operation_duration_seconds_count{job="memory_mcp",env="prod",operation="extract"}[5d]))
sum by (result) (increase(memory_operation_results_total{job="memory_mcp",env="prod",operation="extract"}[5d]))
sum by (stage) (increase(memory_pipeline_stage_duration_seconds_count{job="memory_mcp",env="prod",operation="extract"}[5d]))
sum by (stage) (increase(memory_pipeline_stage_duration_seconds_sum{job="memory_mcp",env="prod",operation="extract"}[5d])) / sum by (stage) (increase(memory_pipeline_stage_duration_seconds_count{job="memory_mcp",env="prod",operation="extract"}[5d]))
sum by (outcome) (increase(memory_claim_pipeline_total{job="memory_mcp",env="prod",stage="reconcile"}[5d]))
sum by (schema,match_mode) (increase(memory_claim_candidates_considered_count{job="memory_mcp",env="prod"}[5d]))
avg_over_time(up{job="memory_mcp",env="prod"}[5d])
min_over_time(up{job="memory_mcp",env="prod"}[5d])
```

For a qualified production comparison, record the deployment image/source
digest and a non-sensitive configuration fingerprint for each window, require
adequate eligible-operation denominators, and preserve per-stage counts and
latency. Keep semantic false-relation and entity-precision/recall gates in the
deterministic eval harness. Do not infer them from warning or entity counters.
