# Codex source-authority v5 diagnostic — run 01

**Decision: NO-GO for the tested Codex workflow.** This one-run screen is a model/workflow qualification, not a memory implementation comparison or powered benefit study.

## Frozen capture

- Plan: `source-authority-codex-literal-input-diagnostic-escalated-2026-11-23`; SHA-256 `d6a5c698809d6d30ec09d00e64ace51f4da31712455981a66092fcc228376e21`.
- Run ID: `18890-1795451869486274000-0`; interval `2026-11-23T16:37:49.486287Z`–`2026-11-23T16:42:41.194812Z`.
- Codex CLI `0.171.0`, requested model alias `gpt-6.1-sol`, existing local login; served model revision unavailable.
- Four new synthetic source families; same product binary in both arms, SHA-256 `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`.
- Agent-trial wall time totals: 50.904 s incumbent and 55.067 s candidate; end-to-end capture about 4 min 52 s.

## Outcomes

| Measure | Incumbent | Candidate | Frozen gate |
|---|---:|---:|---|
| Completed trials | 4/4 | 4/4 | 4/4 |
| Task-oracle success | 1/4 | 1/4 | At least 3/4 per arm — **fail** |
| Agent-observed direct-memory success | 4/4 | 4/4 | Diagnostic |
| Independent fixed-query probes | 4/4 | 4/4 | 4/4 per arm — pass |
| Candidate operator import and extraction | — | 4/4 | 4/4 — pass |
| Hard violations / budget exceedances | 0 / 0 | 0 / 0 | 0 / 0 — pass |

The v5 content-free diagnostics localize each arm's three task failures: final answers were present and schema-valid in 4/4; accepted-answer match was 1/4; required evidence citation was 4/4; clarification behavior matched in 4/4. Thus the observed miss is the answer value failing the task oracle despite valid structure and source citation. Raw answer text and retrieved passages were not retained, so this cannot distinguish model transcription from the precise content it read. The same one family passed in both arms.

Both arms use the same memory binary and have identical task-oracle and direct-memory counts. This does not show a memory treatment effect, a memory-quality regression, or agent benefit. The registered per-arm floor failed, so do not run a repeat or scale this cohort. Keep it separate from the LFM feasibility result; no tested model/configuration is yet qualified for source-authority workflow acceptance.

## Deterministic replay

The offline `compare-agent-managed` replay was byte-identical: SHA-256 `e9749b38620d8f7ee7ab1c2d617802a9588bfce4b39de7c0bdab0a16ac76dc87`. New v5 receipts contain component diagnostics but no raw answer, citation strings, source content or hidden oracle values.
