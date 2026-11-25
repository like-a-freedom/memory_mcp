# C1 agent-managed correction read diagnostics — v3

**Decision: NO-GO for this exact workflow configuration.** The frozen
candidate task floor was 3/4; candidate scored 2/4, so the preregistered stop
rule prohibits capture 02. The incumbent scored 3/4. Both arms used the same
memory binary, product configuration, tool schema, and model alias. This is a
workflow screen, not a product treatment comparison.

## Frozen scope and execution

The four disjoint synthetic families and all task, prompt, model, product,
schema, simulator, and retry-policy identities were frozen in
[`freeze-manifest.sha256`](freeze-manifest.sha256) before capture. All 13
manifest entries passed `shasum -a 256 -c` before execution. The run used
native Apple Silicon (`arm64`), the local release `memory_mcp` binary, and
Codex CLI 0.171.0 with model alias `gpt-6.1-sol`. Remote model requests
contained only the registered synthetic tasks, under the user's prior
authorization. No LM Studio service or CLI was involved.

Capture 01 ran from 2026-11-25 03:45:44Z to 03:52:37Z. The memory-server
SHA-256 was
`9c9b24b757a948eae3299313dd1a0a14c6cd4ddd62f033580bc27a7edf06213d`;
the `memory-eval` SHA-256 was
`f858a13f6a0a45798daf920448db34100fc3dd831687dc344a9d029a31cc000f`.

## Results

| Arm | Exact task answers | Direct agent memory success | Independent probes | Extract / invalidate | Evidence before invalidation | Hard violations / budget exits | Mean / p50 / max recall latency |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Incumbent | 3/4 | 3/4 | 4/4 | 4/4 / 4/4 | 4/4 | 0 / 0 | 16.84 / 17.86 / 20.11 s |
| Candidate | 2/4 | 2/4 | 4/4 | 4/4 / 4/4 | 4/4 | 0 / 0 | 15.05 / 15.28 / 17.48 s |

The offline comparison estimates candidate-minus-incumbent task success at
−0.25, with a deterministic 95% interval of [−0.75, 0.00] over four families.
The descriptive wall-time gain is 1.79 s, with interval [−2.30, 5.89] s.
These small same-product arms do not establish a quality or latency effect.

Every failed task made one empty read and stopped. Every successful task first
made an empty read, then made a second read that returned the cited source.
This is a useful diagnostic association in this eight-trial screen, not proof
that retrying alone caused success. It motivates a separately frozen agent
behavior test for recovery after an empty or partial read.

## What the actual read receipts establish

Across 13 actual `assemble_context` responses, the adapter recognized the
structured MCP result and normalized `outcome` and `item_count` for every
read: eight empty reads had count 0, and five nonempty reads had count 1.
Source coverage was false for the empty reads and true for all five nonempty
reads. The report does not persist raw episode IDs; it records source coverage
and cited-source counts so that user memory identifiers do not leak into
receipts. The in-memory normalizer retains the IDs for scoring.

Four reads reported `partial` completeness and nine reported `unknown`.
`unknown` alone cannot distinguish absent metadata from an unrecognized value:
the raw tool result and the model's exact read arguments are intentionally not
persisted. The frozen preregistration required known completeness for every
successful structured response and asked for source IDs in a content-free
receipt. Those two expectations exceed the current contracts. A future test
must request a revision-bounded read and require completeness only when that
registered request's response contains temporal metadata; it should report
source coverage/counts without persisting raw IDs. Do not rewrite this frozen
registration or its artifacts to fit that corrected design.

Focused parser tests cover both JSON-RPC `result.structuredContent` and the
adapter `payload.structured_content` envelope. They confirm that structured
source IDs, item counts, and recognized completeness values are read from
those shapes, while a content-only response remains unrecognized. The live
capture confirms the JSON-RPC wrapper path produced source coverage and counts;
because the raw response was not retained, it cannot independently resolve
the nine `unknown` completeness values.

## Artifacts

- `plan.json`, `tasks.json`, `preregistration.md`, and `freeze-manifest.sha256`:
  frozen scope and identities.
- `capture-01/`: paired receipts, stage receipts, and offline comparison.
- `SHA256SUMS`: checksums for the capture and report artifacts.

No second capture was run. This result does not qualify C1 for C2 and does not
justify changing memory retrieval behavior. The separate ordinary agent
control remains eligible only for its exact configuration; see the
[ordinary-workflow qualification](../agent-workflow-qualification-withdraw-tools-v3-2026-11-25/README.md).
