# Qwen speed source-authority qualification v1

This is a fresh four-family, provider-neutral Chat Completions workflow screen using the locally installed Qwen3.8 speed-optimized model. Both arms use the same memory binary; the run can qualify this exact agent/model configuration for a larger study but cannot show a memory treatment effect.

## Frozen inputs

The plan, tasks, fingerprints, sampling config, provider inventory, product and protocol controls, prompts, and preregistration are covered by `freeze-manifest.sha256`. Verify it from the repository root before capture:

```sh
shasum -a 256 -c evals/paired/source-authority-local-qwen-speed-v1-qualification/freeze-manifest.sha256
```

The earlier one-call synthetic local smoke passed; it is not a qualification metric. This cohort must be run once only. Do not edit inputs, retry failed trials, or overwrite the run directory.

## Capture

Run only with the exact model alias loaded and matching the inventory. Verify the effective alias with `lms ps` and `/v1/models` first.

```sh
env -u MEMORY_EVAL_BEARER_TOKEN rtk target/debug/memory-eval run-paired-chat-completions \
  --plan evals/paired/source-authority-local-qwen-speed-v1-qualification/plan.json \
  --tasks evals/paired/source-authority-local-qwen-speed-v1-qualification/tasks-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-qwen-speed-v1-qualification/sampling-config.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-qwen-speed-v1-qualification/runs/run-01"
```

The model weights and LM Studio build are not byte-pinned. The deterministic fake-provider tests and offline comparator do not require LM Studio, this model, or an API key. See `preregistration.md` for stop rules and limits.
