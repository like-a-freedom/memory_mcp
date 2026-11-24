# Current Codex source-authority qualification

This registered screen uses the provider-neutral paired observation/evaluation contract with the optional Codex CLI capture adapter. It does not estimate a memory implementation effect.

Verify inputs from this directory before any invocation:

```sh
shasum -a 256 -c freeze-manifest.sha256
```

Run `run-01` only after confirming the current login and that its output directory is absent. A second capture is allowed only if the first passes every frozen criterion in `preregistration.md`.

```sh
CODEX_HOME="$HOME/.codex" codex login status
mkdir -p /private/tmp/memory-mcp-source-authority-codex-2026-11-23
env -u CODEX_API_KEY cargo run --locked -p eval-harness --bin memory-eval -- run-paired-codex \
  --plan evals/paired/source-authority-current-codex-qualification-2026-11-23/plan.json \
  --tasks evals/paired/source-authority-current-codex-qualification-2026-11-23/tasks-v1.json \
  --codex-binary /opt/homebrew/Caskroom/codex/0.171.0/bin/codex \
  --codex-home "$HOME/.codex" \
  --memory-tool-profile agent_source_authority \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --temp-root /private/tmp/memory-mcp-source-authority-codex-2026-11-23 \
  --output-directory "$PWD/evals/paired/source-authority-current-codex-qualification-2026-11-23/runs/run-01"
```

Artifacts contain normalized outcomes, metadata and hashes, not raw model answer text. Do not use the model card or this screen as evidence of memory quality or agent benefit.
