# Ornith clarification-trace fix diagnostic — run 01

## Validity

- The one preregistered capture completed all four cases per arm. The paired receipt validator did not reject a counter/trace mismatch.
- `compare-agent-managed` accepted the paired artifacts. A second offline comparison produced a byte-identical report.
- The registered sidecar-v5 diagnostics identify all six formation `transport` failures (three per arm) as `request_send`. None was a successful-response body-read failure; no HTTP status was recorded. This localizes failure before usable response headers, but does not identify whether LM Studio, its runtime, or the local connection caused it.
- The public clarification regression separately exercises `needs_clarification: true` and verifies its matching trace event. This model capture had zero clarification counters, so it does not independently exercise that branch. New families mean the successful capture cannot prove the trace fix was the exact cause of the earlier invalid cohort.

## Frozen development-screen results

| Measure | Incumbent | Candidate | Gate | Result |
|---|---:|---:|---:|---|
| Task-oracle success | 0/4 | 2/4 | at least 3/4 per arm | Fail |
| Independent fixed-query probes | 0/4 | 3/4 | 4/4 per arm | Fail |
| Candidate source-file tasks | — | 2/2 | at least 1/2 | Pass |
| Candidate operator imports and extractions | — | 2/2 each | 2/2 each | Pass |
| Hard violations | 1 (`unsupported_evidence`) | 0 | 0 total | Fail |
| Budget-exceeded trials | 0 | 0 | 0 | Pass |

The capture was valid as a diagnostic and is a **NO-GO** for this exact agent/model/runner configuration. The descriptive 2/4 versus 0/4 task score is based on four synthetic families and both arms used the same product binary. It is neither a memory treatment effect nor evidence of general agent benefit. The single incumbent unsupported-evidence violation also fails the safety gate.

## Interpretation and limits

The trace validator now completes a fresh capture, while the transport-stage diagnostic narrows this run's six formation failures to request send. It does not show a model-quality improvement, explain the underlying transport cause, or establish that a different memory implementation would help. Do not replay this model cohort, tune it in place, or reuse its family IDs. Source-authority usability acceptance remains open; a configuration must first pass a fresh disjoint qualification before a separately preregistered powered comparison is meaningful.

## Reproducibility

- Plan: [`../../plan.json`](../../plan.json)
- Preregistration and frozen input manifest: [`../../preregistration.md`](../../preregistration.md), [`../../freeze-manifest.sha256`](../../freeze-manifest.sha256)
- Product SHA-256 (both arms): `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`
- Evaluator SHA-256: `c9299fdf3425f4b356784e027020407689cbdac2c0c32bf6d8a6248aee89f533`
- Run ID: `3607-1795478909008772000-0`
- Runtime: approximately 2m03s
- Receipts retain normalized outcomes, diagnostic enums and counters only; no model answer or response body was retained.

