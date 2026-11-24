# Underdog Saluki local qualification v2

Verify the frozen manifest before loading the model or capturing trials:

```sh
shasum -a 256 -c evals/paired/source-authority-local-underdog-saluki-v2-qualification/freeze-manifest.sha256
```

Load the registered alias in LM Studio:

```sh
lms load underdog-saluki-27b-1.0 --context-length 16384 --parallel 1 --identifier memory-local-eval-underdog-saluki --yes
```

Capture once. Never retry or overwrite the registered output directory:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-underdog-saluki-v2-qualification/plan.json \
  --tasks evals/paired/source-authority-local-underdog-saluki-v2-qualification/tasks-v2.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-underdog-saluki-v2-qualification/sampling-config.json \
  --request-parameters-config evals/paired/source-authority-local-underdog-saluki-v2-qualification/request-parameters.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-underdog-saluki-v2-qualification/runs/run-01"
```

Tasks use synthetic facts and a local numeric-loopback endpoint. Deterministic scoring and contract tests require no LM Studio or API key.
