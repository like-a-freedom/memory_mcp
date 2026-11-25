# C1 recovery v6 — capture 01 report

**Decision: NO-GO; capture 02 was not run.** The frozen task-success floor
failed in both arms. This is a workflow qualification result on four fresh
synthetic families, not a memory treatment estimate.

## Frozen setup

The capture ran locally on native Apple Silicon with the registered Codex CLI
and model alias. LM Studio was not used. Both arms used the identical product
binary, product configuration, MCP schema, prompts, task manifest, and
resource budgets. Only synthetic histories were sent to the model. No personal
or production memory was used. The four cases and model-mediated outcomes are
one observation per family; do not treat reads or arms as independent samples.

## Results

| Measure | Incumbent | Candidate |
| --- | ---: | ---: |
| Exact cited task success | 0/4 | 0/4 |
| Independent fixed-query probes | 4/4 | 4/4 |
| Exact correction extraction | 4/4 | 4/4 |
| Exact stale-fact invalidation | 4/4 | 4/4 |
| Evidence verified before invalidation | 4/4 | 4/4 |
| Agent reads | 8/8 empty, partial | 8/8 empty, partial |
| Read source coverage | 0/8 | 0/8 |
| Requested and returned `valid_at` match | 8/8 each | 8/8 each |
| Hard violations / resource or timeout exits | 0 / 0 | 0 / 0 |
| Mean recall latency | 19,531 ms | 19,844.75 ms |
| Median recall latency | 19,414 ms | 19,546.5 ms |
| Maximum recall latency | 20,089 ms | 21,462 ms |

The 3/4 per-arm task floor failed, so the preregistered stop rule disallows a
second capture. The latency figures are descriptive single-capture results;
they do not establish a stable performance difference. Candidate recall was
slower on mean, median, and maximum in this capture.

## What the read diagnostics establish

- The requested and returned valid-time axes match the registered instant on
  every read. A wrong `valid_at` does not explain these empty results.
- For each family, the first query fingerprint is the same across arms, but
  neither arm's first query exactly matches the registered question (0/4).
  Each arm's authorized retry changes the query while retaining its own
  non-query-argument fingerprint.
- First-read non-query-argument fingerprints match between arms in 2/4
  families and differ in 2/4. Because only the aggregate digest is retained,
  the capture cannot identify which argument differs or whether the query is
  semantically equivalent to the registered question.
- Independent probes succeed while the agent-issued reads remain empty. This
  localizes the observed gap to the agent-facing read path for this cohort, but
  does not prove whether query wording, another selector, or their interaction
  caused it. No retrieval or response-presentation change is supported yet.

### Invalid full-cutoff diagnostic

The original paired artifacts show `returned_cutoff_match: false` for all
reads. That result is not valid evidence: this cohort did not register a
transaction-revision checkpoint, while the scorer compared the returned
`valid_at + known_revision` pair against an artificial pair with
`known_revision = None`. The scorer now leaves full-cutoff matches unknown
unless both axes are registered. A public CLI regression first reproduced
the false mismatch and then passed after this correction. The frozen v6
artifacts remain unchanged; do not use their full-cutoff booleans. The valid-
time booleans and the C1 task evaluation are unaffected by this scoring bug.

## Reproduction and artifacts

The frozen inputs and one-time run command are indexed in the parent
[v6 registration](../README.md). The paired artifacts and deterministic
evaluation are in this directory. The run is intentionally not repeatable in
place; a repeat requires a newly frozen disjoint cohort.

The freeze manifest matched before capture. Afterward, the scorer and its
integration test changed to fix the unregistered-revision comparison, so the
current checkout now differs from the frozen hashes for those two source
files. Keep the manifest and capture unchanged; the registered command must
not be rerun from this checkout. The archived receipts and deterministic
evaluation remain the record of the original capture.
