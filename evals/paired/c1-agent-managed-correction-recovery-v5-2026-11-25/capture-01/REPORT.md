# C1 agent-managed correction recovery — capture 01

**Outcome:** NO-GO; stop rule applied, no capture 02.  
**Run:** 2026-11-25 12:46:18–12:53:48 UTC, native Apple Silicon.  
**Agent:** Codex CLI 0.171.0-alpha.13, `gpt-6.1-sol`.  
**Inference boundary:** synthetic registered histories only; no LM Studio.

## Results

| Measure | Incumbent | Candidate |
|---|---:|---:|
| Exact cited task answers | 0/4 | 0/4 |
| Independent memory probes | 4/4 | 4/4 |
| Exact correction extractions | 4/4 | 4/4 |
| Exact stale-fact invalidations | 4/4 | 4/4 |
| Evidence verified before invalidation | 4/4 | 4/4 |
| Memory reads | 8, all empty | 8, all empty |
| First reads matching registered question exactly | 0/4 | 0/4 |
| Authorized query-only retries | 4/4 trials | 4/4 trials |
| Hard violations / timeouts / prohibited probe sources | 0 / 0 / 0 | 0 / 0 / 0 |
| Recall latency, mean / median / max | 20.753 / 21.059 / 21.642 s | 21.531 / 21.668 / 22.003 s |

The candidate-minus-incumbent task-success and direct-memory-success differences
are both zero across four paired families. The registered bootstrap estimate
for candidate latency gain is −778 ms (95% interval −1,991 to +37 ms); this
small pilot does not establish a latency difference. The two arms used the
same product binary and configuration, so these results are a workflow
qualification screen, not a memory treatment comparison.

## Interpretation

The correction path itself worked: each arm extracted the correction, verified
the superseded fact from a successful read before invalidating it, and passed
the independent probe on all four histories. The agent-facing recall path did
not: all sixteen observed reads across both arms returned empty, and no final
answer met the exact citation oracle. The run therefore fails the registered
task-success floor of 3/4 per arm. Per the frozen stop rule, capture 02 was not
run. The exposed families were not recycled, and the prompts, profile, budgets,
model and criteria were not changed. After capture, `cargo fmt` made a
single whitespace-only change to one registered Rust source file. The
pre-format source was reconstructed and its SHA-256 matches the preregistered
digest; the detail is recorded in [`SOURCE-DRIFT.md`](SOURCE-DRIFT.md). The
captured product and evaluator binary hashes remain those recorded in the
paired run.

The first-read exact-query diagnostic was 0/4 in each arm. Receipts deliberately
retain query hashes rather than query text, so they establish that the first
queries differed from the registered question but do not reveal the query
wording. This makes poor query formulation a plausible explanation, not a
proven root cause. The current evidence supports investigating the
agent-to-memory query contract before changing memory ranking or storage.

The preceding ordinary workflow control passed on its registered cohort. That
shows the runner can complete its ordinary read-and-answer flow under the
qualified configuration; it does not explain this correction-recovery failure
or establish benefit from any memory change. See the [ordinary workflow
screen](../../agent-workflow-qualification-withdraw-tools-v3-2026-11-25/README.md).

## Reproduction and integrity

The frozen plan and source identities are in the parent directory. The
post-capture artifacts are checksummed in [`SHA256SUMS`](SHA256SUMS). The
registered evaluator returned `eligible_for_capture_02: false`; its frozen
output is [`evaluation.json`](evaluation.json), and the paired run is
[`agent-managed-comparison.json`](agent-managed-comparison.json). The parent
`README.md` and preregistration remain unchanged as frozen pre-run records.
[`SOURCE-DRIFT.md`](SOURCE-DRIFT.md) documents the one post-capture formatting-
only source change and its hash-matched snapshot.

The run used the local Codex CLI with a temporary home and temporary copy of
the authentication file; it did not use the user's normal Codex state or
configuration. Only the registered synthetic tasks were sent for inference.
The temporary credentials/state were removed after the run. No LM Studio
application, CLI, server, or endpoint was used.
