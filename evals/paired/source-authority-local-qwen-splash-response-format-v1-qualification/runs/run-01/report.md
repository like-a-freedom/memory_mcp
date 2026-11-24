# Qwen3.8 Splash JSON Schema qualification v1 — run 01

**Result: NO-GO for this Qwen Splash / Chat Completions / JSON Schema configuration.** Neither arm met the frozen task-success floor. This run is a workflow and adapter qualification screen, not evidence for or against a memory implementation change.

## Capture integrity

- Plan: `source-authority-local-qwen-splash-json-schema-2026-11-23-v1`
- Run ID: `57879-1795462142295371000-0`
- Capture: `2026-11-23T19:29:02.295382Z`–`2026-11-23T19:31:15.781014Z` (about 2m 13s).
- Four newly frozen synthetic families: two conversational formation tasks and two source-file tasks. Inputs passed the frozen manifest check before loading the model and again before capture.
- Both arms used the same memory product binary SHA-256 `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`, evaluator binary SHA-256 `58d38b8b5e155f51cdf1f5031060fbffc0e6bead3818230d1d265677faae2a8b`, model alias `memory-local-eval-qwen-splash-json`, response-format file and frozen synthetic tasks.
- The local endpoint was numeric loopback. No bearer token, API key, Codex runner or external retrieval was used. The locally indexed model was `incoai/Qwen3.8-27B-Splash`, 4-bit, with a 16,384-token context and parallelism 1. The model-weight hash and LM Studio server-build hash were unavailable.
- The output was accepted by `compare-agent-managed`. Re-running that offline comparator produced a byte-identical comparison artifact. Raw model messages were not persisted.

## Frozen outcomes

| Measure | Incumbent | Candidate | Gate |
|---|---:|---:|---:|
| Task-oracle success | 0/4 | 0/4 | at least 3/4 in each arm |
| Agent-observed direct-memory success | 1/4 | 3/4 | descriptive; not the independent probe |
| Independent fixed-query probes | 1/4 | 3/4 | 4/4 in each arm |
| Candidate source-file task success | — | 0/2 | at least 1/2 |
| Candidate operator imports and extractions | — | 2/2 each | both must complete |
| Hard violations | 0 | 0 | 0 |
| Budget-exceeded trials | 0 | 0 | 0 |

All eight final answers were present and schema-valid, but accepted-answer match was 0/4 in each arm. Required evidence citation passed 1/4 incumbent and 3/4 candidate; clarification behavior matched 4/4 in both arms. Each arm's formation phase completed one task and ended in `model_failure` for three. The probe pairs were both-success 1, incumbent-only 0, candidate-only 2 and neither 1.

## Interpretation

The preregistered qualification required task-oracle success of at least 3/4 and independent probes of 4/4 in each arm, plus at least one of two candidate source-file tasks. These workflow gates failed. The successful schema validation shows that this model accepted the registered response shape on this run; it did not produce correct task answers. Stop qualification for this exact model/runner/sampler/response-format configuration. Do not repeat or tune this exposed cohort.

The higher candidate direct-memory and probe counts are descriptive signals from four synthetic families, not a treatment estimate. Both arms used the same memory binary. The receipts intentionally omit answer and retrieved-content text, so they cannot localize the misses to agent recall, answer formulation, or another model behavior. No memory-quality gain, causal memory effect, broad usability, or model-wide conclusion follows.

## Reproduction and artifacts

The frozen inputs verify from the repository root with:

```sh
shasum -a 256 -c evals/paired/source-authority-local-qwen-splash-response-format-v1-qualification/freeze-manifest.sha256
```

Offline scoring uses the `compare-agent-managed` command with the registered plan, arm artifacts and stage sidecars. Run outputs and this report are covered by [`SHA256SUMS`](SHA256SUMS); verify from this directory with `shasum -a 256 -c SHA256SUMS`. Deterministic tests and scoring require no model, LM Studio, account, API key or agent harness.
