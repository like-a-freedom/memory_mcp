# Local Ornith transport-stage diagnostic

This one-capture, four-family diagnostic records whether formation transport failures occur while sending a request or reading a successful HTTP response body. It uses a fresh disjoint task manifest and sidecar v5, which retains only the closed stage value; provider response headers and bodies remain discarded. Both arms use the same product binary, so this is not a memory treatment comparison.

The frozen setup, limits and gates are in [`preregistration.md`](preregistration.md). Deterministic tests and offline scoring do not require LM Studio or an API key. The local provider is optional and this particular capture uses synthetic facts only.

Verify all frozen inputs from the repository root before the one capture:

```sh
shasum -a 256 -c evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/freeze-manifest.sha256
```

The registered capture uses `memory-eval run-paired-chat-completions`; its command and all configuration are fixed in `preregistration.md`. After capture, run `compare-agent-managed` offline with the files in `runs/run-01/`. Do not replay or alter the cohort after its first model-task request. Record the transport-stage counts and frozen quality gates in `runs/run-01/report.md`.
