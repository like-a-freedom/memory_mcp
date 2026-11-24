# Agent-managed maintenance v6 — bounded fact-evidence response

**Study verdict: NO-GO for agent-workflow promotion.** The candidate needed
8/8 end-to-end successes in both captures. It scored 8/8 and 7/8. The
[current product policy](#current-product-policy-clarification--2026-11-23)
separately defaults to bounded fact evidence under its response contract.

## Question and scope

This fixed-cohort feasibility study tested whether an agent can use the
bounded, opt-in fact-evidence response to maintain a correction with fewer
context tokens and tool calls. It used eight synthetic correction families
disjoint from v1–v5 and two repeated paired captures. The incumbent and
candidate product binaries are pinned in `plan.json`; both arms use the same
Codex alias and host protocol. The candidate changes the response capability,
not stored facts, extraction policy, or retrieval. Therefore this study can
measure workflow feasibility and regressions, but cannot establish an
improvement in persisted-memory quality.

The two captures repeat the same eight families. They are repeatability
evidence, not sixteen independent observations and not a powered population
study. The registered success rule is intentionally conjunctive: every
candidate task must be correct and within the registered budget on both
captures, with exact maintenance writes, evidence before invalidation, passing
independent probes, response-size compliance, and no hard violations.

## Frozen inputs and integrity

- Plan: `plan.json`; task/oracle inputs: `tasks.json`.
- Corpus SHA-256: `1d3c3c18e458684d215b163611815cd0fd9944544dd9680dd7b8b95b22d06da7`.
- Oracle SHA-256: `d6a6c781bfa41e3165962bab32bfe663e2ec74db6bd1c4cfa2fa237a90dcbc60`.
- Incumbent product binary SHA-256: `a06d3151ce1e854692e343cfcdbf829cfa26e46f283ca19fc37ae9ff2ff35273`.
- Candidate product binary SHA-256: `58695c4ab2f0da8f5897bb677e51ebb48247d993ccffd6207b42aad77a09c231`.
- Codex CLI: `0.171.0`, SHA-256 `fc8d77cf7585aded2b9bc487dad29c4ee2f066123e4e76dd3b17653a8322ce42`.
- Evaluator SHA-256: `48f4b70785e0ced4d4af6d358b658e3c2e16c360d2d017395ade5f35ddf315ff`.
- All frozen inputs and manifests are recorded in `freeze-manifest.sha256` and
  `fingerprints.json`; the freeze manifest was verified before execution.

The first run setup attempt failed before any model or agent call because its
temporary root did not exist. It recorded zero agent calls, zero memory calls,
and zero writes. The setup-only attempt is preserved under
`preflight-setup-failure-01/`; it is not counted as a trial. After creating
the required temporary root, the frozen run completed without changing its
inputs. This setup failure is distinct from the quality outcome.

## Results

| Capture | Incumbent task success | Candidate task success | Candidate formation within budget | Candidate evidence before invalidation | Independent probes, incumbent / candidate | Paired task delta (95% family-cluster interval) |
|---|---:|---:|---:|---:|---:|---:|
| run-01 | 7/8 | 8/8 | 8/8 | 8/8 | 8/8 / 8/8 | +0.125 [0.0, 0.375] |
| run-02 | 6/8 | 7/8 | 8/8 | 8/8 | 8/8 / 8/8 | +0.125 [-0.25, 0.5] |

The intervals are descriptive for these eight repeated families; they are not
population guarantees. In run-02, `bluepeak-robotics-history` was a candidate
end-to-end task failure (`direct_memory_success=false` and
`task_oracle_passed=false`) while its independent post-write probe passed. The
stored correction passed that fixed query; this does not erase the agent's
task failure. The receipt intentionally stores only payload hashes, not the
memory response or final answer, so the archived evidence cannot distinguish
an agent-query retrieval miss from answer formulation. It does not establish a
product retrieval regression or support assigning a more specific root cause.

Across both captures:

- Candidate maintenance formation was within the 65,536-token input limit in
  16/16 phases; the incumbent exceeded it in 16/16.
- Candidate formation used 24 tool calls per capture versus 32 for the
  incumbent, a 25% reduction under this protocol.
- Every arm/capture completed exactly one successful `extract` and one
  successful `invalidate` per family. All 32 independent post-write probes
  passed (16 per arm).
- Candidate evidence was requested and returned before invalidation in 16/16
  trials; no evidence was omitted. The largest serialized extract response
  was 3,415 bytes, below the 4,096-byte cap.
- There were zero unsupported/prohibited source findings and zero hard
  violations. No candidate formation phase exceeded the budget.
- Candidate mean formation input was 57,024.9 tokens in run-01 (maximum
  57,193) and 56,671.4 in run-02 (maximum 57,093). Incumbent means were
  72,994.8 and 70,071.5 respectively.

The candidate has a clear narrow feasibility signal: all its maintenance
phases fit the context budget while the incumbent phases do not, with fewer
tool calls and correct independent probes. However, run-02 missed the frozen
all-candidate-correct gate. Do not pool the repeated captures as n=16, infer a
memory-quality treatment effect, or claim broad agent benefit.

## Deterministic replay

`compare-agent-managed` was run twice per capture; each replay reproduced its
comparison byte-for-byte:

- run-01 comparison and replay SHA-256:
  `a7c0596b55d92bdd48edbba711147a25e871b2e1147b6d8e3d6a19bf38e81260`.
- run-02 comparison and replay SHA-256:
  `005aa1f024cce80e348005919041b9313586fa7c14a29bb4806560c4a3d10eb4`.

The raw paired results, receipts, normalized reports, comparison outputs and
replays are preserved in `run-01/` and `run-02/`. Reproduction commands and
the preregistered criteria are in `preregistration.md` and `plan.json`.

## Decision

The v6 promotion criterion is **not met** because candidate task correctness
was 7/8 in run-02 instead of the required 8/8. Keep
`extract.include_fact_evidence` opt-in. The bounded all-or-none 4 KiB response
contract remains useful and its product/evaluator tests pass, but a response
size cap alone is not evidence that silently exposing stored source text by
default is safe for all callers.

Any further default-on proposal needs a materially justified protocol or
workflow change, a new preregistered disjoint cohort, and explicit compatibility
coverage for callers that currently receive the compact response. Repeating
the same eight families or relaxing the failed correctness rule after seeing
the outcomes would not resolve this no-go. The accepted default for the
claim-reconciliation `evidence` rollout is a separate feature and is not
evidence for this response flag.

## Current product-policy clarification — 2026-11-23

The decision above records the v6 **agent-workflow promotion** result: the
frozen all-candidate-correct criterion failed, so v6 does not support an
agent-managed maintenance or agent-benefit claim. A later, separate
product-contract review made `include_fact_evidence` default-on with an
all-or-none serialized 4 KiB cap, explicit compact opt-out, schema/CLI/redaction
coverage, eight deterministic import/extract/probe cases, and a repeated fixed
retrieval no-regression check. That narrow default decision does not reverse
the v6 NO-GO for agent-workflow promotion or establish improved stored-memory
quality or broad agent usability. See the
[response-default report](../../independent-probes/2026-11-23-extract-evidence-default/README.md)
and [follow-up design](../../../docs/superpowers/specs/2026-11-24-agent-memory-follow-up-design.md).
