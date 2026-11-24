# Underdog Saluki local qualification v1

This is a fresh, four-family workflow qualification for the locally installed
Underdog Saluki 27B IQ2-mix model. It is not a memory treatment comparison.
Both arms use the same product binary. The model card's tool-use results are
publisher-reported and do not substitute for this cohort's frozen gates.

Before loading the model or capturing trials, verify the frozen-input manifest
from the repository root:

```sh
shasum -a 256 -c evals/paired/source-authority-local-underdog-saluki-v1-qualification/freeze-manifest.sha256
```

Load the registered local model alias in LM Studio:

```sh
lms load underdog-saluki-27b-1.0 --context-length 16384 --parallel 1 --identifier memory-local-eval-underdog-saluki --yes
```

Capture once. Do not retry or overwrite the registered output directory:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-underdog-saluki-v1-qualification/plan.json \
  --tasks evals/paired/source-authority-local-underdog-saluki-v1-qualification/tasks-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-underdog-saluki-v1-qualification/sampling-config.json \
  --response-format-config evals/paired/source-authority-local-underdog-saluki-v1-qualification/response-format.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-underdog-saluki-v1-qualification/runs/run-01"
```

Synthetic tasks, local numeric-loopback endpoint, no bearer token. The exact
gates and interpretation limits are in preregistration.md. Deterministic
profiles and offline scoring need no model, API key, LM Studio, or agent
harness.
