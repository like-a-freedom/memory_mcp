# Flint source-authority qualification v1 — run 01

**Result: NO-GO for this Flint/Chat Completions configuration.** The task and independent-probe floors both failed. This is not evidence for or against a memory implementation change.

## Capture integrity

- Plan: `source-authority-local-flint-2026-11-23-v1-qualification`
- Run ID: `52038-1795459853482108000-0`
- Capture: `2026-11-23T18:50:53.482110Z`–`2026-11-23T18:53:17.673693Z` (2m 24.192s)
- Four preregistered synthetic families; one receipt per family per arm. Both arms used the same product binary SHA-256 `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`, product configuration, public tool schema, task manifest and sampler.
- The evaluator binary was pinned at SHA-256 `e4f03c47dc314de29e53e76c10d12be4f829ce899c2d1bbe273c0d7b50ad84be`.
- The local endpoint was numeric loopback. The capture used synthetic facts, no bearer token, no API key, no Codex runner and no network fetch. The frozen model alias resolved to `recursivesi/flint` Q4_K_M, loaded with context 16,384 and parallelism 1. The model-weight hash, device identity and LM Studio server build were unavailable.
- The frozen inputs passed `freeze-manifest.sha256` before capture. The offline `compare-agent-managed` scorer accepted both arm artifacts and stage sidecars.

## Frozen outcomes

| Measure | Incumbent | Candidate | Gate |
|---|---:|---:|---:|
| Task-oracle success | 0/4 | 0/4 | at least 3/4 in each arm |
| Independent fixed-query probes | 1/4 | 3/4 | 4/4 in each arm |
| Candidate source-file task success | — | 0/2 | at least 1/2 |
| Operator file imports and extraction | — | 2/2 each | both must complete |
| Hard violations | 0 | 0 | 0 |
| Budget-exceeded trials | 0 | 0 | 0 |

The probe pairs were both-success 1, incumbent-only 0, candidate-only 2 and neither 1. The normalized task receipts contained seven `final_answer_not_json` model failures; the remaining candidate receipt completed with a schema-valid but oracle-incorrect answer. Each arm's formation stage completed one task and ended in `model_failure` for three tasks. The candidate's two operator imports and two extraction actions succeeded, but neither source-file recall task passed.

## Interpretation

The registered continuation rule required task-oracle success of at least 3/4 and independent probes of 4/4 in each arm, plus at least one of two candidate source-file tasks. All three gates failed. Stop qualification for this exact model/runner/sampler configuration; do not replay this cohort or reuse its exposed families.

The candidate's higher independent-probe count is descriptive only: four synthetic families are too few to establish a treatment effect, and both arms used the same product binary. The wrong or malformed final answers do not show that memory behavior improved or regressed. Model weights and server build were not immutably identified, so this also cannot support a model-wide claim.

## Reproduction and artifacts

The frozen inputs verify from the repository root with:

```sh
shasum -a 256 -c evals/paired/source-authority-local-flint-v1-qualification/freeze-manifest.sha256
```

Offline scoring used the registered `compare-agent-managed` command with the plan, arm artifacts and formation/probe sidecars. Run outputs and this report are covered by [`SHA256SUMS`](SHA256SUMS); verify from this directory with `shasum -a 256 -c SHA256SUMS`. Deterministic tests and scoring require no model, LM Studio, account, API key or agent harness.
