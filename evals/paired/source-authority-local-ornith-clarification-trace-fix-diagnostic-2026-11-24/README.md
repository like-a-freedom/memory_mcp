# Local Ornith clarification-trace fix diagnostic

This one-capture, four-family development diagnostic checks that the public Chat Completions runner can complete and validate a fresh paired workflow after a deterministic clarification-counter/trace mismatch was fixed. It also collects sidecar v5's content-free transport failure stage. Both arms use the same product binary, so this is not a memory treatment comparison.

The previous v5 cohort is exposed and invalid; this registration uses four disjoint family IDs. The exact configuration and one-shot rule are in [`preregistration.md`](preregistration.md).

Before the one model capture, verify every frozen input from the repository root:

```sh
shasum -a 256 -c evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/freeze-manifest.sha256
```

Then follow the exact capture and offline comparison commands in `preregistration.md`. Do not replay the cohort or reuse its family IDs after the first model-task request. Deterministic tests and offline scoring require no model or API key; LM Studio is only the optional local provider used for this diagnostic.
