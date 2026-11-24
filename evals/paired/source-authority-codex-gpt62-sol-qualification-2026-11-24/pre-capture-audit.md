# Pre-capture audit

- New family IDs and answer values were checked against all existing JSON, Markdown, and text artifacts under `evals/paired/`; none matched.
- `memory-eval hash-agent-managed-tasks --memory-tool-profile agent_source_authority_literal_input` accepted the v3 manifest and reported 4 tasks / 4 independent families.
- Codex CLI `0.171.0` executable SHA-256 and `memory-eval` SHA-256 are pinned in `fingerprints.json` and `plan.json`. Product binary/config/schema identities are identical across arms.
- Official Codex model documentation lists `gpt-6.2-sol` for Codex CLI and recommends it for everyday tasks. The installed CLI smoke used only the synthetic request “reply exactly READY”; it exited successfully and the expected synthetic response was observed. No smoke output was persisted.
- `codex login status` returned success using `/Users/solovey/.codex`; the check recorded only status, not account details. `CODEX_API_KEY` was absent.
- The model child is ephemeral, read-only, tool-limited and has no shell or web search. Only registered synthetic material is passed to the remote model.
- Family order is balanced by `incumbent_first`; no task content or answer values occur in the run output directory before capture.

This audit records preconditions only. Qualification outcomes are reported separately after capture and offline comparison.
