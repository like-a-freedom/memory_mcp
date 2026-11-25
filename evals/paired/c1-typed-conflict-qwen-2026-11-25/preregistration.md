# C1 typed-conflict local qualification — preregistration

**Status:** frozen before C1 task captures on 2026-11-25.

The initial runner preflight exposed a stale MCP tool-schema fingerprint copied
from the preceding ordinary cohort. Four fresh public probe captures against
the exact current binary all returned schema SHA-256
`cc5339eb0b61c4a1ab93caf7f48b6e9d9c0b1c33a8fe5deb72658fe1abfdb752`, while
the copied plan had `9475b795bde1360eda3cf672b8c75d99ff1fa6cd0e91b219907a7fc81c71880f`.
The paired runner rejected this mismatch before invoking the model. The first
freeze manifest is retained as `preflight-initial-freeze-manifest.sha256`; the
corrected plan and inputs are re-frozen below before any agent task request.

## Question and scope

Can the exact local Qwen + direct MLX + provider-neutral Chat Completions runner complete a small fixed set of synthetic typed-conflict recall tasks and preserve the registered one-read and independent evidence gates? This is workflow qualification only. Both paired arms use the same current product binary, configuration, evidence rollout stage, and prompts. The screen does not estimate a memory treatment effect or population success rate.

## Frozen inputs

`plan.json`, `tasks.json`, `probes.json`, `criteria.json`, `simulator.json`, and `model-manifest.json` define four fresh synthetic families, the exact model weight manifest, and the evaluation contract. The model was selected using separate synthetic protocol smoke requests before this cohort was frozen; those smoke requests are excluded from every C1 denominator. No C1 family is reused from the earlier ordinary, typed-conflict, source-import, or maintenance cohorts.

## Execution

- Native Apple Silicon (`aarch64-apple-darwin`) with direct `mlx-lm==0.32.2` server on `127.0.0.1:18768`; no LM Studio application, CLI, or server is used. The path name of the downloaded weights contains `.cache/lm-studio`, but the runtime is the independent MLX server.
- `HF_HUB_OFFLINE=1`, `TRANSFORMERS_OFFLINE=1`, `enable_thinking=false`, temperature 0. All prompts, histories, identifiers and answers are synthetic.
- One tool call, three turns, 384 output tokens, 4,096 context tokens, 25 seconds per trial. Both arms use the same current `memory_mcp` binary and `evidence` claim stage.
- Each capture contains the four same frozen families. These are repeatability captures, not eight independent families.

## Deterministic prerequisites and stop rule

Before any agent capture, replay the four independent public probes against the exact product binary. Require 4/4 source coverage, required `conflicting` annotations, and correct snapshot metadata. If any deterministic probe fails, stop the agent screen and preserve the failure. After capture 01, run capture 02 only if both arms have at least 3/4 exact task successes, at least 3/4 actual memory reads, 4/4 successful independent probes, zero hard violations, and zero budget or timeout exits. The same rule applies to capture 02. The existing offline `qualify-agent-workflow` report is retained; because its candidate gate alone is not enough for this same-binary configuration, both arms must also meet these preregistered floors.

No result from this screen can justify a memory product change or a powered benefit claim. A miss is attributed using per-read diagnostics and the independent probe, not post-hoc prompt or oracle changes.

## Local command

```sh
HF_HUB_OFFLINE=1 TRANSFORMERS_OFFLINE=1 UV_CACHE_DIR=/private/tmp/memory-mcp-uv-cache \
  uv run --no-project --with mlx-lm==0.32.2 python -m mlx_lm.server \
  --model /Users/solovey/.cache/lm-studio/models/lmstudio-community/Qwen3.8-27B-MLX-4bit \
  --host 127.0.0.1 --port 18768 --max-tokens 512 --temp 0 \
  --chat-template-args '{"enable_thinking":false}' --decode-concurrency 1 --prompt-concurrency 1

target/debug/memory-eval run-paired-chat-completions \
  --plan evals/paired/c1-typed-conflict-qwen-2026-11-25/plan.json \
  --tasks evals/paired/c1-typed-conflict-qwen-2026-11-25/tasks.json \
  --model-endpoint http://127.0.0.1:18768/v1 \
  --recall-read-mode live \
  --independent-probes evals/paired/c1-typed-conflict-qwen-2026-11-25/probes.json \
  --incumbent-memory-server target/debug/memory_mcp \
  --candidate-memory-server target/debug/memory_mcp \
  --output-directory evals/paired/c1-typed-conflict-qwen-2026-11-25/capture-01
```
