# Qwen source-authority qualification v5 — run 01

**Result: NO-GO for further workflow evaluation with this model/configuration.** The preregistered task-oracle and independent-probe floors were not met. This is a model/workflow qualification result, not a memory-quality treatment estimate or an agent-benefit claim.

## Capture integrity

- The capture ran from `2026-11-23T17:35:07.941655Z` to `2026-11-23T17:45:26.385162Z` (10m 18.444s).
- All four frozen synthetic families have one receipt per arm. Both arms used the same local Qwen alias, product binary SHA-256 `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`, product configuration and public tool schema.
- The offline `compare-agent-managed` scorer accepted the paired artifacts and produced [agent-managed-comparison.json](agent-managed-comparison.json). It reports zero hard violations and zero budget-exceeded trials.
- Independent-probe outcomes were both-success 2, incumbent-only 1, candidate-only 1, neither 0; therefore each arm passed 3/4, below the frozen 4/4 floor.
- The preregistered sampler was sent on every turn. The local endpoint was numeric loopback; no bearer token or external API key was used. Raw provider responses and retrieved source text were not persisted.

## Frozen screen outcomes

| Measure | Incumbent | Candidate | Gate / interpretation |
|---|---:|---:|---|
| Task-oracle success | 1/4 | 0/4 | Fails the minimum 3/4 in each arm |
| Agent-observed direct-memory success | 3/4 | 3/4 | Paired estimate 0; not the independent probe |
| Independent fixed-query probe | 3/4 | 3/4 | Fails the required 4/4 per arm |
| Candidate source-file task success | — | 0/2 | Fails the required 1/2 |
| Hard violations | 0 | 0 | Safety check only |
| Budget-exceeded trials | 0 | 0 | Resource check only |

Candidate receipts contained two `model_failure` attempts classified as `final_answer_not_json` and two completed but oracle-incorrect answers. One completed answer omitted required evidence; the other cited evidence but still gave the wrong answer. Incumbent receipts contained one `invalid_tool_call` model failure, two completed incorrect answers and one completed correct answer. The candidate's source-file tasks both failed the task oracle. The 95% family-cluster interval for the overall task-success difference was `[-0.75, 0.0]` across four families; the cohort is too small for a comparative claim.

The recorded trial wall-time sums were 179.217s for candidate and 103.174s for incumbent; total capture wall time was 618.444s. These descriptive figures do not establish a controlled speed comparison and do not qualify the workflow.

## Decision and limits

All three preregistered continuation conditions failed: task-oracle success was below 3/4 in both arms, independent probes were below 4/4 in both arms, and candidate source-file task success was below 1/2. Stop model-backed source-authority evaluation for this Qwen configuration. Do not rerun or reuse these exposed families.

Both arms used the same memory binary, so this capture does not estimate a memory implementation effect. The four synthetic families, one run, mutable local model alias, unavailable model-weight hash and unreported LM Studio server build also prevent a powered or general agent-benefit claim. The result does not show that the memory changes helped or harmed agents.

## Reproduction and artifact integrity

The preregistration inputs are verified with `shasum -a 256 -c evals/paired/source-authority-local-chat-completions-v5-sampler-qualification/freeze-manifest.sha256`. Run artifacts and this report are covered by [SHA256SUMS](SHA256SUMS), verifiable from the repository root. Recreate only the offline comparison with this directory's plan and four JSON artifacts; do not call the model again or overwrite this run.
