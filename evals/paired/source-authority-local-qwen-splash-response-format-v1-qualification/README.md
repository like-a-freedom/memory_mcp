# Qwen3.8 Splash JSON Schema qualification v1

This is a fresh four-family model/runner qualification screen for the optional
provider-neutral `response_format` setting. It is not a memory treatment-effect
comparison: both arms use the identical product binary. The strict response
format is fingerprinted in the paired plan and supplied on both tool and final
turns. Model weight and server-build hashes are unavailable.

## Frozen-input check

From the repository root, verify before loading the model or starting a run:

```sh
shasum -a 256 -c evals/paired/source-authority-local-qwen-splash-response-format-v1-qualification/freeze-manifest.sha256
```

## One-shot capture

Load the exact model as the registered alias in LM Studio:

```sh
lms load qwen3.8-27b-splash --context-length 16384 --parallel 1 --identifier memory-local-eval-qwen-splash-json
```

Then capture once. Do not retry, tune or overwrite the output directory:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-qwen-splash-response-format-v1-qualification/plan.json \
  --tasks evals/paired/source-authority-local-qwen-splash-response-format-v1-qualification/tasks-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-qwen-splash-response-format-v1-qualification/sampling-config.json \
  --response-format-config evals/paired/source-authority-local-qwen-splash-response-format-v1-qualification/response-format.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-qwen-splash-response-format-v1-qualification/runs/run-01"
```

The expected gate and limitations are recorded in `preregistration.md`. This
cohort is synthetic, local-loopback, no-token, and single-use. Deterministic
profiles and offline scoring remain model-independent.
