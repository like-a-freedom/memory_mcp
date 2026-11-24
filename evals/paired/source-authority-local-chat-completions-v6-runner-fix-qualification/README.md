# Qwen source-authority qualification v6 — reasoning-history runner fix

Fresh four-family qualification after the provider-neutral adapter began preserving optional `reasoning_content` in tool-call follow-ups. It also corrects the prior task's unsupported assumption that `ingest.content` reads a path: incumbent tasks now provide the factual sentence as literal text, while the candidate operator-imports identical bytes from a staged file.

This is a model/workflow qualification screen. Both arms use the same memory binary; it is not a memory change comparison or a powered agent-benefit evaluation.

## Frozen inputs

`tasks-v6.json`, `plan.json`, `fingerprints.json`, `sampling-config.json`, `provider-inventory.json`, protocol/simulator/product/host/retry configuration, and `prompts/`. Verify before capture from the repository root:

```sh
shasum -a 256 -c evals/paired/source-authority-local-chat-completions-v6-runner-fix-qualification/freeze-manifest.sha256
```

## Capture

Run once only when `memory-local-eval-qwen38` is loaded. Do not retry this cohort or overwrite output.

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-chat-completions-v6-runner-fix-qualification/plan.json \
  --tasks evals/paired/source-authority-local-chat-completions-v6-runner-fix-qualification/tasks-v6.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-chat-completions-v6-runner-fix-qualification/sampling-config.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-chat-completions-v6-runner-fix-qualification/runs/run-01"
```

The deterministic tests and offline scorer need no provider or LM Studio. The live screen uses synthetic input, numeric loopback and no bearer token. Model weights and the LM Studio server build are not immutably identified; a pass only qualifies a later independent pilot.
