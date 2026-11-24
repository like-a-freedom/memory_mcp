# Local source-authority paired pilot v3

This exploratory local run uses one identical Memory MCP product binary in both arms. It evaluates the feasibility and interaction cost of two preparation workflows; it does not estimate a memory implementation effect or prove agent benefit. The cohort is synthetic, fixed, and disjoint from v1/v2.

LM Studio is an optional local model provider. The harness only requires an OpenAI-compatible Chat Completions endpoint at numeric loopback. The deterministic contract target uses a fake HTTP/MCP endpoint and does not require LM Studio or any model.

## Frozen registration

All preregistration inputs and checksums are in this directory. Verify with:

```sh
shasum -a 256 -c freeze-manifest.sha256
```

Run the independent provider-neutral contract suite on Windows, Linux, or macOS with:

```sh
cargo test --locked -p eval-harness --test chat_completions_runner
```

This target uses only fake in-process endpoints. It does not read model settings or access a provider.

## Optional local captures

The first capture uses only this frozen plan/task set and the installed local provider. Preserve failures; do not rerun into an existing directory. Repeat as `run-02` with exactly the same hashes and settings.

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions --plan evals/paired/source-authority-local-chat-completions-v3/plan.json --tasks evals/paired/source-authority-local-chat-completions-v3/tasks-v3.json --model-endpoint http://127.0.0.1:1234/v1 --incumbent-memory-server "$PWD/target/debug/memory_mcp" --candidate-memory-server "$PWD/target/debug/memory_mcp" --output-directory "$PWD/evals/paired/source-authority-local-chat-completions-v3/runs/run-01"
```

For PowerShell, use the same Cargo command with `memory_mcp.exe` and an absolute `Join-Path` output directory. The adapter does not invoke `lms`; the provider process must already be running. No Codex API key or bearer token is used.

The frozen stage is `development_pilot`; the correct two-repeat report command is `summarize-pilot` with the two run directories. Do not use a diagnostic promotion gate. Preserve per-arm raw receipts and sidecar validation outcomes; incomplete or rejected captures are not usability passes.

## Interpretation

Treat repeats as nested within the same 16 synthetic families. A valid capture can confirm corrected artifact compatibility and provide descriptive feasibility evidence for this one model and workflow only. It cannot establish powered agent benefit, broad workflow usability, memory-quality improvement, equivalence, or generalization to other providers.
