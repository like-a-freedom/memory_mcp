# Codex literal-source qualification

This is a four-family, one-run model/workflow qualification screen using the corrected literal-input protocol. It is not an evaluation of a memory implementation change. See [preregistration](preregistration.md) for the stop rule and claim limits.

Verify the frozen inputs before running:

```sh
shasum -a 256 -c evals/paired/source-authority-codex-literal-input-qualification-2026-11-23/freeze-manifest.sha256
```

Run only after the preregistration and fingerprints are finalized and the output directory does not exist. The Codex adapter uses an already-authenticated local Codex home; no API key is needed.

```sh
mkdir -p /private/tmp/memory-mcp-source-authority-literal-input-2026-11-23
env -u CODEX_API_KEY target/debug/memory-eval run-paired-codex \
  --plan evals/paired/source-authority-codex-literal-input-qualification-2026-11-23/plan.json \
  --tasks evals/paired/source-authority-codex-literal-input-qualification-2026-11-23/tasks-v2.json \
  --codex-binary /opt/homebrew/Caskroom/codex/0.171.0/bin/codex \
  --codex-home /Users/solovey/.codex \
  --memory-tool-profile agent_source_authority_literal_input \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --temp-root /private/tmp/memory-mcp-source-authority-literal-input-2026-11-23 \
  --output-directory "$PWD/evals/paired/source-authority-codex-literal-input-qualification-2026-11-23/runs/run-01"
```
