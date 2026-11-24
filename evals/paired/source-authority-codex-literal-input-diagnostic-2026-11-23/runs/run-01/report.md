# Sandboxed Codex diagnostic attempt — invalid preflight

**Disposition: INVALID / INCONCLUSIVE; not a model qualification.**

- Plan: `source-authority-codex-literal-input-diagnostic-2026-11-23`; SHA-256 `c43367709b32a731b0c1ee48d6ef06ca5dbc12154c21a5f59cfcd4a3f0911b6e`.
- Run ID: `17592-1795451531787371000-0`.
- All eight arm receipts ended in `model_failure` before a model turn: zero turns, tool calls and tokens. Recorded trial wall times were 267–337 ms.
- No task answer was produced. The v5 fields (`final_answer_present=false`, schema invalid, accepted-answer/evidence unset) only describe the missing response; they are not model-quality outcomes.
- The receipt format intentionally retained no process stderr, so the precise spawn failure is not established. The invocation used the restricted executor; a separate Codex CLI version check succeeded outside that sandbox. This attempt is preserved and excluded from model qualification. Do not reuse its frozen family set.

The local-login, synthetic-only run on a fresh, separate family set is recorded in the escalated diagnostic registration. No conclusion about model usability or memory quality is drawn from this preflight attempt.
