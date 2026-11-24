# Qwen speed source-authority qualification v2

This is a fresh four-family, provider-neutral Chat Completions workflow screen using the locally installed Qwen3.8 speed-optimized model. It changes only the recall task-rules prompt from frozen v1 to v2, which asks for the exact value without explanation to match the deterministic oracle. Both arms use the same current memory binary, so this screen cannot estimate a memory treatment effect.

## Frozen inputs

The plan, tasks, fingerprints, sampling config, provider inventory, product and protocol controls, prompts, preregistration, and pre-capture audit are covered by `freeze-manifest.sha256`. Verify it from the repository root before capture:

```sh
shasum -a 256 -c evals/paired/source-authority-local-qwen-speed-v2-prompt-qualification/freeze-manifest.sha256
```

The four synthetic task families are disjoint from prior source-authority cohorts. The model identity is pinned to the loaded local alias within this invocation. Its weights and LM Studio server build cannot be byte-pinned; see `provider-inventory.json`.

## Capture

Confirm the alias in `lms ps --json` and `/v1/models` immediately before running. Do not change inputs, retry failed trials, or overwrite the output directory. The deterministic evaluator, fake-provider tests and offline comparison do not need LM Studio, this model, or an API key.

```sh
env -u MEMORY_EVAL_BEARER_TOKEN rtk target/debug/memory-eval run-paired-chat-completions \
  --plan evals/paired/source-authority-local-qwen-speed-v2-prompt-qualification/plan.json \
  --tasks evals/paired/source-authority-local-qwen-speed-v2-prompt-qualification/tasks-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-qwen-speed-v2-prompt-qualification/sampling-config.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-qwen-speed-v2-prompt-qualification/runs/run-01"
```

Compare captured artifacts offline with the `compare-agent-managed` subcommand. Apply only the gates in `preregistration.md`; a failed screen is a no-go for this exact model/configuration, not evidence of a memory defect or benefit.
