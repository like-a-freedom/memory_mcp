# Run 01 report — corrected-sampler source-authority NO-GO

**Plan:** `source-authority-local-ornith-35b-a3b-2026-11-24-corrected-sampler-qualification`  
**Capture:** `86015-1795474107563424000-0`  
**Model alias:** `memory-local-eval-ornith-v2`  
**Capture window:** `2026-11-23T22:48:27.563435Z`–`2026-11-23T22:50:21.589294Z` (114.026 s)  
**Offline comparator:** accepted; an independent comparator replay was byte-identical.

## Frozen outcomes

| Measure | Incumbent | Candidate | Frozen requirement | Result |
|---|---:|---:|---:|---|
| Task-oracle success | 0/4 | 2/4 | at least 3/4 per arm | Fail |
| Independent fixed-query probes | 1/4 | 3/4 | 4/4 per arm | Fail |
| Source-file task-oracle success | 0/2 | 2/2 | candidate at least 1/2 | Pass |
| Candidate operator imports | — | 2/2 | 2/2 | Pass |
| Candidate extractions | — | 2/2 | 2/2 | Pass |
| Hard violations | 0 | 0 | zero | Pass |
| Budget-exceeded trials | 0 | 0 | zero | Pass |

The paired task-success difference is +0.50 with a descriptive family-cluster
95% interval of [0.00, 1.00] over only four synthetic families. Direct-memory
success was 1/4 incumbent and 3/4 candidate. These estimates are descriptive
and are not promotion evidence. Both arms used the same product binary and
product configuration, so this run does not estimate a memory implementation
effect.

## Failure localization and limits

The agent-managed formation sidecar reports `completed` for one family and
`model_failure` for three families in each arm. The new content-free terminal
class is `transport` for all six failures. In the Chat Completions adapter,
`transport` covers client send errors, non-success HTTP responses, and response
body read errors; the status code and response body are intentionally not
retained. Therefore the exact failure cause is unknown. These failures count
against the registered usability gate, but they do not establish a reasoning
or memory defect.

The candidate's two source-file tasks passed and each operator import and
extraction succeeded. Both conversation-control tasks failed their task
oracle in both arms. Cross-model runtime comparisons are not valid here: the
LFM pilot and this screen used different cohorts and this screen includes
product/database setup time. The capture is not evidence that Ornith is faster
or slower than LFM.

## Decision

**NO-GO for this exact Ornith alias, LM Studio runner, sampler and task
protocol.** The task and probe thresholds failed. Preserve this exposed cohort
and do not rerun or tune it. Keep source-authority agent usability open; no
memory change or powered agent-benefit claim follows from this screen.

## Reproduction and artifacts

The frozen inputs verify with `freeze-manifest.sha256` in the cohort directory.
The capture command is in the cohort [`README.md`](../../README.md). The
accepted offline comparison is `agent-managed-comparison.json`; the two arm
artifacts and content-free formation/probe sidecars are in this directory.
`SHA256SUMS` covers all run outputs, including this report.
