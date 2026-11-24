# Execution addendum — preflight and capture path

The original run-01 invocation exited before any trial or Codex child process because its temporary root had not been created. Its empty receipt journals and preflight report are preserved as invalid technical evidence. The registered model/workflow screen is still one valid capture only.

Create the temporary root before running, preserve `runs/run-01`, and direct the valid capture to `runs/run-02`:

```sh
mkdir -p /private/tmp/memory-mcp-codex-gpt62-sol-qualification-2026-11-24
env -u CODEX_API_KEY target/debug/memory-eval run-paired-codex \
  --plan evals/paired/source-authority-codex-gpt62-sol-qualification-2026-11-24/plan.json \
  --tasks evals/paired/source-authority-codex-gpt62-sol-qualification-2026-11-24/tasks-v3.json \
  --codex-binary /opt/homebrew/Caskroom/codex/0.171.0/bin/codex \
  --codex-home /Users/solovey/.codex \
  --memory-tool-profile agent_source_authority_literal_input \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --temp-root /private/tmp/memory-mcp-codex-gpt62-sol-qualification-2026-11-24 \
  --output-directory "$PWD/evals/paired/source-authority-codex-gpt62-sol-qualification-2026-11-24/runs/run-02"
```

This administrative path correction does not change the frozen design or quality gates.

Run-02 completed and passed the frozen model/workflow screen. Its metrics,
claim limits, artifact hashes, and deterministic replay result are recorded in
[`runs/run-02/report.md`](runs/run-02/report.md) with
[`runs/run-02/SHA256SUMS`](runs/run-02/SHA256SUMS). The registration remains
unchanged and its frozen inputs still verify.
