# Underdog Saluki v2 source-authority qualification

## Decision

**NO-GO for the exact model, server, runner, prompt, and budget configuration.**
The screen did not qualify this agent workflow for a larger powered comparison.
Do not repeat or tune this exposed cohort. The independent probe result is a
small exploratory signal only; this run is not an estimate of a memory
implementation treatment effect.

## Registered capture

- Four fresh synthetic families, eight arm trials; the same product binary was
  used in both arms (`3f4506f9…f091586`). The comparison used the hash-pinned
  plan and evaluator binary.
- Model: local `memory-local-eval-underdog-saluki`, a loaded
  `ConwayResearch/Underdog-Saluki-27B-1.0` IQ2-mix alias. The model card
  recommends thinking off and temperature 0 for direct tool calls. The run
  pinned `temperature=0`, `top_p=0.95`, `top_k=20`, and
  `chat_template_kwargs.enable_thinking=false`; no response-format schema was
  active.
- Capture interval: 2026-11-23 20:13:13–20:26:41 UTC (13m28s).
- Inputs were frozen before model loading; `freeze-manifest.sha256` verified
  all 17 files. Both arms used the same local numeric-loopback endpoint, model
  alias, and memory binary.
- The exact model-weight and LM Studio server-build hashes were unavailable.
  Identity is therefore within-run alias identity, not byte-level model
  provenance.

## Frozen gates and observed results

| Gate | Result | Status |
|---|---:|---|
| Task-oracle success, incumbent | 0/4 | Fail (required ≥3/4) |
| Task-oracle success, candidate | 0/4 | Fail (required ≥3/4) |
| Independent probes, incumbent | 2/4 | Fail (required 4/4) |
| Independent probes, candidate | 4/4 | Pass |
| Candidate source-file task success | 0/2 | Fail (required ≥1/2) |
| Candidate operator imports/extractions | 2/2 imports; 3/3 extracts succeeded | Pass |
| Hard violations | 0 | Pass |
| Budget-exceeded trials | 4 | Fail (required 0) |

The offline `compare-agent-managed` scorer accepted the artifacts. The paired
task-success estimate is 0.0. Its deterministic interval is not evidence of
model-quality uncertainty because every task failed and there are only four
families. Candidate independent probes passed 4/4 versus 2/4 incumbent (two
candidate-only pairs, no incumbent-only pairs); the paired direct-memory
success difference is +0.5 with a 95% family-cluster interval of [0, 1]. That
signal is compatible with the operator-import path supplying retrievable
facts, but cannot qualify agent usability while all task answers fail and four
trials exceed their budgets.

Every arm trial used 6 or 8 `assemble_context` calls, then ended without a
final answer accepted by the task oracle. Receipts retain no raw model or
retrieved text. The run identifies an agent-tool-loop/final-answer failure
class, not a memory retrieval root cause. No unsupported-source use or hard
violation was observed.

## Reproduction and artifacts

The registered invocation and post-run offline comparator are in
[`README.md`](../../README.md); the exact frozen gates and task identities are
in the cohort's `preregistration.md`, `plan.json`, and `tasks-v2.json`.
Content-free arm receipts, stage supplements, and the comparator report are
stored beside this file. SHA-256:

- Incumbent artifact: `3a6dadac322a0b756af384d1a79ad874152fe89a77dc9d5f03702ad2221c1a52`
- Candidate artifact: `54e295022e5422ce57841e8155bedca58c90d0eb4f14c2ba826584906a5ea83a`
- Incumbent stage supplement: `b71bc5a84427b2e43d03d71a8315f6cd1b4a7c6937952e6d9e92c603e21a9d72`
- Candidate stage supplement: `006278ff429449e2bdfb9b3cbd74f247477e34ef2d2af97bd78cdc1e4adcdfe8`
- Offline comparison: `758a91a65d303dd56b6d0ba13944d192c962e8840a8fbb4e60546b37c33914f2`

No memory behavior was changed based on this failed agent workflow screen.
Continue using deterministic memory-decision and retrieval gates for memory
acceptance; powered agent-benefit evidence remains open.
