# Run 02 report — corrected Codex source-authority qualification

**Plan:** `source-authority-codex-literal-input-2026-11-23-qualification`  
**Plan SHA-256:** `e7ebe78c075282562de41874618c1c17ce5d3fb316e0f6a1144c21281cf964c2`  
**Valid capture:** `7465-1795449143929572000-0`  
**Run interval:** 2026-11-23 15:52:23.929576Z–15:57:33.144277Z  
**Runner/model:** Codex CLI 0.171.0 / requested alias `gpt-6.1-sol`  
**Product:** same `memory_mcp` binary in both arms, SHA-256
`3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`.

## Results

| Measure | Incumbent | Candidate | Frozen gate |
|---|---:|---:|---|
| Task-oracle success | 1/4 | 1/4 | At least 3/4 in each arm — **fail** |
| Agent-observed direct-memory success | 4/4 | 4/4 | Diagnostic; no independent promotion threshold |
| Independent fixed-query probes | 4/4 | 4/4 | 4/4 in each arm — pass |
| Candidate operator imports and extraction | — | 4/4 | At least 3/4 — pass |
| Hard violations | 0 | 0 | 0 — pass |
| Budget-exceeded trials | 0 | 0 | 0 — pass |

All eight agent trials completed in one turn and each made one memory-read tool
call. The sole task-oracle success was the same family in both arms. The
operator-import setup completed all four candidate fixtures (eight setup
actions, 1,735 ms total), outside the candidate agent session.

The preregistered qualification is **NO-GO** because both arms scored 1/4
against a 3/4 task-oracle floor. Do not proceed to a broader agent workflow
study with this configuration. The 4/4 direct-memory results mean the agent
session returned admissible mapped memory evidence for each task; the 4/4
independent probes separately confirm the fixed-query memory path. The task
failures therefore do not establish a memory-retrieval defect. Since raw
answer text is intentionally not retained, the exact reason the final answer
missed the oracle in three cases is unknown. No memory-quality or agent-benefit
improvement is established: both arms used the same product binary and had the
same task success count.

## Integrity and replay

The literal source bytes were supplied directly to the incumbent formation
prompt; the candidate used evaluator-side `import file` on the same frozen
bytes and a fresh read-only recall session. The run used four new synthetic
families. The prior run-01 startup failure is documented separately and did
not produce a model turn.

Offline `compare-agent-managed` accepted all four paired families. Re-running
the comparator from the frozen plan and preserved artifacts produced a
byte-identical `comparison.json`. All ten preregistered input checksums pass;
the registration files and `freeze-manifest.sha256` were not changed.

## Artifact SHA-256

```text
99f1dacd3549a78dcf1fe7c8d4e1eb043aff5fc2c248dfce908131a1e5be6a8f  agent-managed-candidate.json
b2b2857fcd43b3ec6c8c29a0a0c0d1812c44d69f56b83899c0e504c9500fd937  agent-managed-incumbent.json
d3b5eca71304c2400dae3c48ce6da61e21ae6b2e487285e0fe9d0dce844d13c5  candidate.json
bf5ab217dfa771a8391b91ee56d7c4c68c90adee40e5d24cce12c9a7c7ac2243  comparison.json
5cb7048c8af676d0d101edc54b5d9b9c9a41f6c8e17df5ad4d4c4cc77479a863  incumbent.json
808562578f102bf9e1fac155875e4c243682e2b8e616910f5f21ecf1a78860ed  candidate-receipts.jsonl
c6367dbd6e989c4e33dc8f99b1cf9f8be2d80232deffa812fa200b4bce059ec0  incumbent-receipts.jsonl
```

**Decision:** This closes the corrected-benchmark qualification attempt as a
NO-GO for the tested Codex workflow. Source-authority usability remains
unaccepted. A future attempt needs a model/configuration that passes a new
disjoint qualification and a separately powered registration; do not reuse or
tune on these exposed families.
