# Flint source-authority qualification v1

Fresh four-family screen for the provider-neutral Chat Completions adapter. The candidate was selected because the official [RecursiveSI/Flint model card](https://huggingface.co/RecursiveSI/flint) describes multi-turn tool use and publishes temperature 1.0, top-p 0.95, top-k 20; the serving guidance requires forwarding `reasoning_content` in assistant history, which this runner now supports. These model-card results do not establish performance on memory tasks.

This is a model/workflow qualification screen, not a memory change comparison or powered agent-benefit evaluation. Both arms use the same product binary.

## Frozen inputs

`plan.json`, `tasks-v1.json`, `fingerprints.json`, `sampling-config.json`, `provider-inventory.json`, protocol/simulator/product/host/retry configuration, prompts, `preregistration.md`, and `freeze-manifest.sha256`. Verify before model load or capture:

```sh
shasum -a 256 -c evals/paired/source-authority-local-flint-v1-qualification/freeze-manifest.sha256
```

## Model and capture

After verifying the frozen inputs, load the registered local model alias `memory-local-eval-flint` at 16,384 context with one parallel prediction in LM Studio: `lms load recursivesi/flint --context-length 16384 --parallel 1 --identifier memory-local-eval-flint`. Run once with no bearer token; do not retry this cohort or overwrite its output. The capture command is pinned in the preregistration.

Deterministic eval-harness tests and offline scoring do not require LM Studio, a model, account, API key, or agent harness.
