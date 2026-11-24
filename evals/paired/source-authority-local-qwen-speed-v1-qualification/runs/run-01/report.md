# Run 01 report — Qwen speed qualification NO-GO

**Plan:** `source-authority-local-qwen-speed-2026-11-23-v1-qualification`  
**Capture:** `69156-1795467146086499000-0`  
**Capture window:** 2026-11-23 20:52:26–20:59:37 UTC (7m 11.721s)  
**Product:** same binary in both arms, SHA-256 `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`

## Outcome

The offline `compare-agent-managed` scorer accepted all four paired families,
and a second offline comparison produced a byte-identical result. Task-oracle
success was **0/4 in each arm**, below the preregistered 3/4-per-arm floor.
Independent fixed-query probes passed **1/4 for incumbent and 4/4 for
candidate**. Candidate source-file task success was **0/2**, below the required
1/2. Both candidate operator imports and extractions succeeded (2/2), with zero
hard violations and zero budget-exceeded trials.

All four candidate final answers were present and schema-valid; all cited the
required evidence and matched the clarification rule. None matched the
accepted-answer oracle (0/4). One incumbent trial completed with an accepted
schema-valid response that missed the oracle; three incumbent trials ended in
model failures. Candidate trial receipts completed in all four families, while
the separately reported formation-stage counters include two model failures.
These answer diagnostics localize a final-value mismatch but cannot identify
its cause because raw responses are intentionally not retained.

Candidate direct-memory success was 4/4 versus incumbent 1/4 (descriptive
paired difference +0.75; family-cluster 95% interval [0.25, 1.00]). This does
not override the task-oracle stop gate, establish that an agent completed the
task, or estimate a memory effect: both arms used the same memory binary.

## Decision and limits

**NO-GO for this exact model/runner/sampler configuration.** Do not tune or
repeat the exposed cohort. The result does not qualify the source-authority
workflow for a larger powered study and does not justify a memory change.
Although this capture took about 7m12s, it is not a controlled speed comparison
with earlier cohorts: family sets, model configurations and/or evaluator
revisions differ. LM Studio and model weights were local and were not byte
identifiable in the run manifest.

## Reproducibility

The frozen inputs remain unchanged and verifiable with
[`freeze-manifest.sha256`](../../freeze-manifest.sha256). The original and
replayed comparator files are byte-identical (SHA-256
`fd5b01c41555ab4d744fef2d193e7e9f11afa0316b11c9d903e92e8f93ddcda3`). Their
hashes and the receipt hashes are recorded in `SHA256SUMS` in this directory.
The model calls used only synthetic tasks against numeric-loopback LM Studio;
no hosted API key was needed.
