# Fresh Codex source-authority diagnostic screen

This one-run four-family screen uses the current Codex CLI model alias and v5 content-free task-oracle diagnostics. It is disjoint from prior exposed source-authority cohorts; it is not a memory implementation comparison. See `preregistration.md` for the frozen gate.

Verify the inputs before running:

```sh
shasum -a 256 -c evals/paired/source-authority-codex-literal-input-diagnostic-2026-11-23/freeze-manifest.sha256
```

The required evaluation and product binaries are `target/debug/memory-eval` and `target/debug/memory_mcp`; their hashes are pinned in `plan.json`. Confirm local Codex login and ensure both temporary root and output directory are new.

```sh
CODEX_HOME=/Users/solovey/.codex /opt/homebrew/Caskroom/codex/0.171.0/bin/codex login status
mkdir -p /private/tmp/memory-mcp-source-authority-codex-diagnostic-2026-11-23
env -u CODEX_API_KEY target/debug/memory-eval run-paired-codex \
  --plan evals/paired/source-authority-codex-literal-input-diagnostic-2026-11-23/plan.json \
  --tasks evals/paired/source-authority-codex-literal-input-diagnostic-2026-11-23/tasks-v1.json \
  --codex-binary /opt/homebrew/Caskroom/codex/0.171.0/bin/codex \
  --codex-home /Users/solovey/.codex \
  --memory-tool-profile agent_source_authority_literal_input \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --temp-root /private/tmp/memory-mcp-source-authority-codex-diagnostic-2026-11-23 \
  --output-directory "$PWD/evals/paired/source-authority-codex-literal-input-diagnostic-2026-11-23/runs/run-01"
```

Do not repeat this cohort. Artifacts contain content-free booleans and hashes, never raw model answers.
