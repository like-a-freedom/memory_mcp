# Agent-managed memory maintenance feasibility v1

This archive records a preregistered feasibility and cost pilot for one
bounded memory correction. It is not a product comparison, promotion gate,
population estimate, agent-benefit study, or source-authority usability test.

## Frozen protocol

The protocol was registered before the first live agent trial in
[`preregistration.md`](preregistration.md). It uses eight synthetic histories,
each with one existing fact, one user-supplied correction, and one superseded
fact to invalidate. Two repeats run both arms with fresh isolated stores.
Agent maintenance has only the existing `assemble_context`, `ingest`,
`extract`, and `invalidate` MCP tools; shell, filesystem, web, plugins, apps,
and Codex memories are disabled. A separate fixed public MCP probe runs after
the agent sessions against the same store.

The product binary, evaluator binary, Codex CLI, tool schema, product config,
prompts, host policy, protocol, simulator, task corpus, and oracle are pinned
in [`plan.json`](plan.json) and [`fingerprints.json`](fingerprints.json).
Both arms use the same product binary
(`a06d3151ce1e854692e343cfcdbf829cfa26e46f283ca19fc37ae9ff2ff35273`), the
same `evidence` rollout stage, and the same existing local Codex login. No
`CODEX_API_KEY` was needed. No product-memory code changed for this pilot.
The four exact maintenance and fresh-recall prompt files are archived under
[`prompts/`](prompts/), with their raw and composite hashes in
[`prompt-fingerprints.json`](prompt-fingerprints.json). The composite hashes
match the prompt identities frozen in the plan.

## Results

| Repeat | Arm | Task and direct-memory success | Independent probes | Maintenance phase status | Mean input tokens / phase | Mean time / phase |
|---|---|---:|---:|---|---:|---:|
| 01 | incumbent | 6/8 | 8/8 | 8/8 budget exceeded | 101,383 | 38.7 s |
| 01 | candidate | 6/8 | 8/8 | 8/8 budget exceeded | 102,083 | 42.8 s |
| 02 | incumbent | 3/8 | 8/8 | 8/8 budget exceeded | 101,557 | 38.2 s |
| 02 | candidate | 8/8 | 8/8 | 8/8 budget exceeded | 102,108 | 39.4 s |

Across the 32 arm/trial maintenance phases:

- **32/32** exceeded the registered 65,536-token per-turn context budget.
  Each phase used one turn; the highest phase input was 103,504 tokens. Mean
  aggregate input was 101,783 tokens per phase. The maximum phase wall time was
  64.2 seconds, below the separate 120-second limit.
- All **32/32** independent probes supported the registered correction and
  returned no unsupported or prohibited sources. There were **zero** hard
  violations.
- Each trial made exactly one successful `ingest`, one `extract`, and one
  `invalidate`: **32/32 successes per operation**, with no failed or unresolved
  writes.
- Agent task success totaled **23/32**. Repeat 1 was tied at 6/8 per arm; repeat
  2 varied from 3/8 incumbent to 8/8 candidate. As the arms ran the identical
  product, this is unexplained agent/run variation, not a memory treatment
  effect. The archived receipts contain hashed traces rather than raw answer
  text, so they do not support a detailed answer-error taxonomy.

The preregistered feasibility gate **fails** because every maintenance phase
exceeded its context budget and not every task answer passed. The probes and
write accounting show that the memory state ended with the expected correction
in these cases; they do not make the over-budget workflow feasible or establish
that agents can reliably maintain memory in ordinary use.

## Report-field scope

`paired_agent_comparison.budget_exceeded_trial_count` belongs to the generic
paired recall comparison and counts budget-exceeded recall receipts. It is
zero here because those recall receipts were completed. Maintenance formation
phases are a separate stage; their statuses are recorded in each arm's
`formation_status_counts` and are `budget_exceeded: 8` in every run/arm. The
frozen preregistration is unchanged; this is a post-capture schema
interpretation note, not a protocol amendment.

## Replaying the deterministic comparisons

The live Codex invocations are not deterministic. The offline comparison of
each frozen pair of artifacts is deterministic and can be replayed with the
pinned evaluator build. For example:

```sh
target/debug/memory-eval compare-agent-managed \
  --plan evals/paired/agent-managed-maintenance-v1/plan.json \
  --incumbent evals/paired/agent-managed-maintenance-v1/run-01/incumbent.json \
  --candidate evals/paired/agent-managed-maintenance-v1/run-01/candidate.json \
  --incumbent-stages evals/paired/agent-managed-maintenance-v1/run-01/agent-managed-incumbent.json \
  --candidate-stages evals/paired/agent-managed-maintenance-v1/run-01/agent-managed-candidate.json \
  --output /tmp/agent-maintenance-run-01-replay.json
```

Change `run-01` to `run-02` and the output path to replay the second report.
Both archived per-run comparisons were replayed and matched byte-for-byte.
The evaluator SHA used for capture is in
[`evaluator-binary.sha256`](evaluator-binary.sha256); the Codex CLI and product
SHA pins are in the plan and adjacent checksum files. Verify archived files
with:

```sh
shasum -a 256 -c evals/paired/agent-managed-maintenance-v1/SHA256SUMS.txt
```

## Interpretation and next gate

The pilot establishes a reproducible failure mode for this frozen
agent-maintenance protocol, not for every possible maintenance workflow. Do
not infer a treatment effect from different incumbent/candidate task scores:
both arms used the same product. Do not tune on these eight observed histories
and call them confirmation. Any smaller-token workflow needs a new
preregistration, new disjoint families, fixed budgets and prompts, and the same
source-support, invalidation, independent-probe, and hard-safety criteria.
Even a passing feasibility protocol would not establish agent benefit; that
requires an appropriately powered comparison against a defined workflow
baseline.

The separate source-authority usability study also remains open. The existing
Codex adapter has no safely scoped `import file`/`import url` capability; this
pilot intentionally adds no MCP tool and does not exercise operator import.
