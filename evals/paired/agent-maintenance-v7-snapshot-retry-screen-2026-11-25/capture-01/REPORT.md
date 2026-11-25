# A11 capture 01 report

## Decision

**Retry behavior passed; agent-facing retrieval and task outcome failed.** Every case exposed an eligible empty partial read, and each registered retry followed the snapshot-bound contract. Both arms nevertheless returned no evidence from their two agent-facing reads and passed 0/4 exact answer-plus-citation tasks. This is a four-family synthetic feasibility result with the same memory product in both arms; it is not a product treatment comparison or a general usability estimate. No second capture is registered.

## Results

| Measure | Incumbent | Candidate |
| --- | ---: | ---: |
| Completed recall tasks | 4/4 | 4/4 |
| Exact accepted answer plus required evidence | 0/4 | 0/4 |
| Agent-visible source evidence | 0/4 | 0/4 |
| Actual reads | 8 | 8 |
| Empty reads | 8/8 | 8/8 |
| Eligible registered retry exposures | 4/4 | 4/4 |
| Compliant snapshot-bound retries | 4/4 | 4/4 |
| Independent fixed-query probes | 4/4 | 4/4 |
| Correction extraction | 4/4 | 4/4 |
| Exact superseded-fact invalidation | 4/4 | 4/4 |
| Evidence returned before invalidation | 4/4 | 4/4 |
| Unsupported / prohibited source use | 0 / 0 | 0 / 0 |
| Hard violations | 0 | 0 |
| Recall latency, mean / median | 18.128 / 18.140 s | 19.199 / 18.861 s |
| Recall latency, range | 16.828–19.405 s | 17.543–21.530 s |
| Formation input / output tokens | 203,133 / 1,365 | 203,316 / 1,386 |
| Formation wall time, total | 101.615 s | 105.273 s |

Each task made exactly two reads. The scorer marks every first read as eligible, then verifies the agent made one retry with a changed query, copied the returned snapshot cutoff, omitted legacy `as_of` and `known_at`, and preserved all six other selectors. All 4/4 task-level retry diagnostics per arm were compliant; none required a second retry.

The agent reads returned no items, so the final response did not mention the registered value or cite its source. The independent fixed-query probe found the corrected event in all four stores, while the correction extraction, evidence-before-invalidation audit, and exact invalidation checks also passed in all four. This separates a successful write/state transition from an empty agent-facing read, but does not identify the exact query or selector choice: request text and selector values are intentionally absent from these receipts.

Recall latency is descriptive only. Both arms used the same memory binary and configuration, and this one capture does not establish a speed or accuracy difference between arms.

## Measurement limitation

The frozen preregistration also required elapsed time for each actual memory
read. The captured observations and receipts contain no per-read duration, so
that requirement was not met and cannot be reconstructed from this capture.
The table above reports total recall-phase latency only; it must not be read as
memory-read latency. Add and validate per-read timing at the observation
boundary before registering another comparable screen. The frozen
preregistration, model responses and scored run artifacts remain unchanged;
this report now discloses the missing measurement.

## Execution integrity

The run used native Apple Silicon, local `memory_mcp`, and the pinned Codex CLI/model alias. Only the four newly registered synthetic histories were sent for remote inference; LM Studio was not used. The first setup attempt stopped before inference because its tool-schema fingerprint came from an older preflight capture; its journals are empty and it is excluded. A fresh local synthetic preflight captured the current tool schema before this valid capture.

The offline `compare-agent-managed` replay reproduced `agent-managed-comparison.json` byte-for-byte. The SHA-256 manifest in this directory covers the capture outputs.
