# Local Ornith corrected-sampler source-authority screen

This is a new four-family, provider-neutral Chat Completions workflow screen. It tests the model card's recommended sampling parameters after the previous Ornith configuration failed with temperature 0 and implicit `top_p`. It uses the same memory product in both arms and cannot estimate a memory implementation effect.

The official model-card and local runtime evidence are summarized in [`model-runtime-evidence.md`](model-runtime-evidence.md). The local preflight passed a tool-call round trip but is not a task-quality result.

Verify the frozen inputs from the repository root before starting the one capture:

```sh
shasum -a 256 -c evals/paired/source-authority-local-ornith-corrected-sampler-qualification-2026-11-24/freeze-manifest.sha256
```

The scorer and fake-provider tests do not need LM Studio or a model. The live screen uses only a numeric-loopback provider and synthetic facts. Do not rerun or replace any trial after model-task capture begins.

```sh
cargo build --locked -p memory_mcp
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-ornith-corrected-sampler-qualification-2026-11-24/plan.json \
  --tasks evals/paired/source-authority-local-ornith-corrected-sampler-qualification-2026-11-24/tasks-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-ornith-corrected-sampler-qualification-2026-11-24/sampling-config.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-ornith-corrected-sampler-qualification-2026-11-24/runs/run-01"
```

Apply only the preregistered gates in [`preregistration.md`](preregistration.md). Deterministic scoring and offline comparison need no API key.
