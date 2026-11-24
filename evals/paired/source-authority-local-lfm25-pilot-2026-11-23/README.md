# Local LFM2.5 source-authority feasibility pilot

This is a one-capture, four-family exploratory workflow pilot for the local
`liquid/lfm2.5-1.2b` model. It uses the same Memory MCP product in both arms and
does not estimate a memory implementation effect or establish general agent
quality. The four task families are synthetic and were created separately from
the earlier Qwen cohort.

LM Studio is an optional provider. The adapter uses the provider-neutral
OpenAI-compatible Chat Completions API over numeric loopback. Deterministic
contract tests use fake HTTP and MCP endpoints and remain independent of LM
Studio, Codex, model accounts, and API keys.

## Verify the frozen registration

```sh
shasum -a 256 -c evals/paired/source-authority-local-lfm25-pilot-2026-11-23/freeze-manifest.sha256
```

Run the provider-independent contract tests on Windows, Linux, or macOS:

```sh
cargo test --locked -p eval-harness --test chat_completions_runner
```

## Run the single registered capture

The local model must already be loaded and served at the pinned loopback
endpoint. Do not retry a failed arm or reuse the run directory.

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-lfm25-pilot-2026-11-23/plan.json \
  --tasks evals/paired/source-authority-local-lfm25-pilot-2026-11-23/tasks.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-lfm25-pilot-2026-11-23/runs/run-01"
```

The runner uses the exact alias in `provider-inventory.json`; the model weights
and server build do not expose immutable hashes, so the registration records
that limitation. Preserve all receipts, including failures. A single capture
is descriptive feasibility evidence only and cannot pass a promotion gate.
