# Qwen source-authority qualification v5 — sampler correction

This is a fresh four-family, model/workflow qualification screen. It tests the
same source-authority workflow as v4 with Qwen's published thinking-mode
sampling recommendations explicitly sent and pinned. It is not a memory
change comparison or a powered agent-benefit evaluation. See
[preregistration.md](preregistration.md) for the decision gate and limits.

## Frozen inputs

- [Task manifest](tasks-v5.json)
- [Paired plan](plan.json)
- [Hash output](fingerprints.json)
- [Sampling configuration](sampling-config.json)
- [Provider inventory](provider-inventory.json)
- [Freeze manifest](freeze-manifest.sha256)
- Product, protocol, retry, host and simulator configuration copied into this
  directory; prompts are in `prompts/`.

Before capture, verify every listed input from the repository root:

```sh
shasum -a 256 -c evals/paired/source-authority-local-chat-completions-v5-sampler-qualification/freeze-manifest.sha256
```

PowerShell can use `Get-FileHash -Algorithm SHA256` for each listed path.

## Capture

Capture once, only after confirming `memory-local-eval-qwen38` is the loaded
model alias. Do not retry into the same cohort or output directory.

```sh
cargo build --locked -p memory_mcp --bin memory_mcp
env -u MEMORY_EVAL_BEARER_TOKEN cargo run --locked -p eval-harness --bin memory-eval -- \
  run-paired-chat-completions \
  --plan evals/paired/source-authority-local-chat-completions-v5-sampler-qualification/plan.json \
  --tasks evals/paired/source-authority-local-chat-completions-v5-sampler-qualification/tasks-v5.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-chat-completions-v5-sampler-qualification/sampling-config.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-chat-completions-v5-sampler-qualification/runs/run-01"
```

On Windows use the same Cargo command with PowerShell environment syntax and
`target/debug/memory_mcp.exe`. The deterministic tests, sampler hashing and
offline scorer need no LM Studio or model service; LM Studio is only the local
provider used for this one screen.

After capture, preserve every artifact, produce a report and run SHA-256
checksums over the outputs. Do not edit the frozen inputs or repeat the model
call. The model server build and weight file are not immutably identified, so
the result remains exploratory even if it meets the screen thresholds.

