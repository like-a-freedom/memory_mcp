# Ornith HTTP-status diagnostic — run 01

**Plan:** `source-authority-local-ornith-35b-a3b-2026-11-24-http-status-diagnostic`  
**Run ID:** `94435-1795476629396870000-0`  
**Window:** 2026-11-23 23:30:29–23:32:17 UTC (107.753 s)  
**Interpretation:** valid one-capture development diagnostic; frozen usability gates failed.

## Frozen outcome

| Measure | Incumbent | Candidate | Gate |
|---|---:|---:|---|
| Task-oracle success | 1/4 | 1/4 | At least 3/4 per arm — fail |
| Independent fixed-query probes | 1/4 | 3/4 | 4/4 per arm — fail |
| Candidate source-file tasks | — | 1/2 | At least 1/2 — pass |
| Candidate operator imports and extractions | — | 2/2 | 2/2 — pass |
| Hard violations | 0 | 0 | 0 — pass |
| Budget-exceeded trials | 0 | 0 | 0 — pass |

The task-success difference is zero; its descriptive family-cluster 95%
interval is `[-0.75, 0.75]`. Because both arms used the same product binary,
this contrast is not a memory treatment effect.

Each arm completed one formation phase and had three `model_failure` formation
phases normalized as `transport`. No captured formation failure had a
non-success HTTP status code. That rules out a returned non-2xx response as the
observed cause, but sidecar v4 did not distinguish request-send failure from a
response-body read failure. It also cannot identify a server-side cause from
the stored evidence.

The offline managed comparison accepted the registered artifacts. A separate
offline replay to a temporary output was byte-identical to the registered
comparison. The registered cohort must remain immutable and must not be
replayed as a new model capture. No answer or provider response content is
retained.

## Decision

**NO-GO for this exact Ornith model/runner/sampler configuration.** The frozen
task and independent-probe gates failed. This single four-family diagnostic
does not qualify an agent workflow, estimate a memory effect, or establish
general agent benefit. A later diagnostic with transport-stage telemetry
requires a new registration and disjoint family IDs.
