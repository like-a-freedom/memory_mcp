# Escalated Codex source-authority diagnostic screen

This one-run four-family screen uses the current Codex CLI alias and v5 content-free task-oracle diagnostics. It is disjoint from all earlier cohorts. Run only with the user's authorized laptop-local execution outside the restricted shell sandbox; the model receives synthetic data only.

Verify the frozen inputs before running:

```sh
shasum -a 256 -c evals/paired/source-authority-codex-literal-input-diagnostic-escalated-2026-11-23/freeze-manifest.sha256
```

The evaluation and product binaries are `target/debug/memory-eval` and `target/debug/memory_mcp`; their hashes are pinned in `plan.json`. Confirm existing local Codex login and use a new temporary root and output directory.

```sh
CODEX_HOME=/Users/solovey/.codex /opt/homebrew/Caskroom/codex/0.171.0/bin/codex login status
mkdir -p /private/tmp/memory-mcp-source-authority-codex-diagnostic-escalated-2026-11-23
env -u CODEX_API_KEY target/debug/memory-eval run-paired-codex \
  --plan evals/paired/source-authority-codex-literal-input-diagnostic-escalated-2026-11-23/plan.json \
  --tasks evals/paired/source-authority-codex-literal-input-diagnostic-escalated-2026-11-23/tasks-v1.json \
  --codex-binary /opt/homebrew/Caskroom/codex/0.171.0/bin/codex \
  --codex-home /Users/solovey/.codex \
  --memory-tool-profile agent_source_authority_literal_input \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --temp-root /private/tmp/memory-mcp-source-authority-codex-diagnostic-escalated-2026-11-23 \
  --output-directory "$PWD/evals/paired/source-authority-codex-literal-input-diagnostic-escalated-2026-11-23/runs/run-01"
```

Do not repeat this cohort. Artifacts contain content-free outcomes and hashes, not raw model answers.
