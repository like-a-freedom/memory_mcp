# Local Qwen source-authority qualification v4

This is a single, small, preregistered qualification screen for the current local Qwen model and the post-diagnostic evaluator. It is not a paired memory-quality comparison or confirmation cohort. The complete decision rules and limits are in [preregistration.md](preregistration.md).

The four task families are new and synthetic. They cover two direct conversation memories and two source-file imports. Both arms use the same product binary; only the memory preparation workflow differs. The candidate operator imports are evaluator-owned fixtures and the model has no shell or network-fetch tool.

## Frozen inputs

- Task manifest: [tasks-v4.json](tasks-v4.json)
- Plan: [plan.json](plan.json)
- Task/oracle/config fingerprints: [fingerprints.json](fingerprints.json)
- Local provider inventory: [provider-inventory.json](provider-inventory.json)
- Product and protocol configuration, host policy, simulator, retry policy and prompts are preserved in this directory.

Before any capture, verify the manifest:

```sh
shasum -a 256 -c evals/paired/source-authority-local-chat-completions-v4-qualification/freeze-manifest.sha256
```

## Capture

The optional model-backed capture requires the LM Studio local server already running with model alias `memory-local-eval-qwen38`. It uses no Codex account or API-key environment variable:

```sh
env -u MEMORY_EVAL_BEARER_TOKEN cargo run --locked -p eval-harness --bin memory-eval -- \
  run-paired-chat-completions \
  --plan evals/paired/source-authority-local-chat-completions-v4-qualification/plan.json \
  --tasks evals/paired/source-authority-local-chat-completions-v4-qualification/tasks-v4.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-chat-completions-v4-qualification/runs/run-02"
```

Use the corresponding `memory_mcp.exe` path and PowerShell environment syntax on Windows. This model screen itself is local to this laptop; deterministic contract tests and offline artifact scoring remain provider-independent and require no LM Studio installation.

The output directory must be new. Preserve failures and never rerun into it. `run-01` is reserved for the recorded setup failure: the initial command used a relative output path, and the CLI rejected it before creating a model process or issuing a request. The valid first capture uses `run-02`. Do not edit, replay or re-score this cohort after seeing results. The single capture is diagnostic and has no repeatability or population inference.

