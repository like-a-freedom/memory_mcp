# Qwen source-authority qualification v6 — run 01

**Result: NO-GO for this Qwen/runner configuration.** Task-oracle and independent-probe gates did not pass. This is not evidence for or against a memory implementation change.

## Capture integrity

- Plan: `source-authority-local-chat-completions-2026-11-23-v6-runner-fix-qualification`
- Run ID: `45913-1795458086604710000-0`
- Capture: `2026-11-23T18:21:26.604712Z`–`2026-11-23T18:38:19.695017Z` (16m 53.090s)
- Four preregistered synthetic families; one receipt per family per arm. Both arms used the same product binary SHA-256 `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586` and the same Qwen alias, product configuration, public tool schema and sampler.
- The evaluator binary was pinned at SHA-256 `e4f03c47dc314de29e53e76c10d12be4f829ce899c2d1bbe273c0d7b50ad84be`.
- Local endpoint was numeric loopback. The capture used synthetic facts, no bearer token, no API key, no Codex runner, and no network fetch. Model weights and the LM Studio server build are not immutably identified.
- Both arm artifacts and stage sidecars passed the offline `compare-agent-managed` scorer.

## Frozen outcomes

| Measure | Incumbent | Candidate | Gate |
|---|---:|---:|---:|
| Task-oracle success | 0/4 | 0/4 | at least 3/4 in each arm |
| Independent fixed-query probes | 2/4 | 3/4 | 4/4 in each arm |
| Candidate source-file task success | — | 0/2 | at least 1/2 |
| Hard violations | 0 | 0 | 0 |
| Budget-exceeded trials | 0 | 0 | 0 |

The probe-pair counts were both-success 1, incumbent-only 1, candidate-only 2, neither 0. Candidate operator setup completed both file imports and both extraction actions. Formation status counts were incumbent 3 `timed_out` and 1 `model_failure`; candidate 3 `completed` and 1 `model_failure`. All three continuation gates failed.

## Interpretation

The v6 cohort corrected the prior unsupported incumbent instruction: `ingest.content` was supplied literal factual text, never a file path. Candidate file bytes were imported by the registered operator workflow before read-only recall. A separate v3 task-manifest identity now pins the literal-input protocol; legacy v1/v2 fingerprints remain unchanged.

The screen still fails even with the adapter preserving optional `reasoning_content` across tool-call follow-ups. The 0/4 task success in both arms and sub-floor probes make this Qwen configuration unsuitable for a broader workflow pilot. Do not rerun it or reuse these exposed families. This is not a memory quality result: both arms used the same memory binary, the cohort contains only four synthetic families, and both model weights and server build are unavailable for immutable identification. The result cannot support a powered or general agent-benefit claim.

The screen took longer than the previous Qwen four-family screen, but those runs are not a controlled speed comparison: they used different task families and evaluator revisions. The LFM2.5 screen remains smoke-only after all task answers failed its oracle.

## Reproduction and artifacts

The frozen inputs verify from the repository root with:

```sh
shasum -a 256 -c evals/paired/source-authority-local-chat-completions-v6-runner-fix-qualification/freeze-manifest.sha256
```

Offline scoring was performed with the registered `compare-agent-managed` command and the plan, arm artifacts, and stage sidecars in this directory. Run files and this report are covered by [`SHA256SUMS`](SHA256SUMS); verify from this run directory with `shasum -a 256 -c SHA256SUMS`. Do not call the model again or overwrite this run.
