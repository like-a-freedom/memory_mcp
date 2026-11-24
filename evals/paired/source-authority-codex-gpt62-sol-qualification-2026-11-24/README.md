# Codex GPT-6.2 Sol qualification

A one-capture, four-family screen of this model/agent workflow using synthetic data. It is not a memory product comparison or powered agent-benefit result. See [preregistration](preregistration.md) for the frozen gates and claim limits.

Verify all frozen inputs before execution:

```sh
shasum -a 256 -c evals/paired/source-authority-codex-gpt62-sol-qualification-2026-11-24/freeze-manifest.sha256
```

The model provider is remote; the registered task text, synthetic facts, tool schemas and synthetic memory responses are the only evaluation payload. Do not add real user data. Existing Codex login is used with no separate API key.

```sh
env -u CODEX_API_KEY target/debug/memory-eval run-paired-codex \
  --plan evals/paired/source-authority-codex-gpt62-sol-qualification-2026-11-24/plan.json \
  --tasks evals/paired/source-authority-codex-gpt62-sol-qualification-2026-11-24/tasks-v3.json \
  --codex-binary /opt/homebrew/Caskroom/codex/0.171.0/bin/codex \
  --codex-home /Users/solovey/.codex \
  --memory-tool-profile agent_source_authority_literal_input \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --temp-root /private/tmp/memory-mcp-codex-gpt62-sol-qualification-2026-11-24 \
  --output-directory "$PWD/evals/paired/source-authority-codex-gpt62-sol-qualification-2026-11-24/runs/run-01"
```

The run directory must not exist before capture. Preserve it as immutable evidence.
