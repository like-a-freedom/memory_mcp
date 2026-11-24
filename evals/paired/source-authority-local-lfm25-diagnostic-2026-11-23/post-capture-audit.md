# Post-capture audit — 2026-11-23

This note was added after the registered capture. It is deliberately outside
`freeze-manifest.sha256`; the frozen README and every registered input retain
their original hashes. Do not rerun this registration or reuse its four
families.

## Runner-command correction

The frozen README's example used a relative `--output-directory`. The runner
requires this path to be absolute. The first attempt was rejected before any
model request, created only an empty run directory, and that directory was
removed. The successful single capture used the same frozen tasks and plan with
an absolute output path:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-lfm25-diagnostic-2026-11-23/plan.json \
  --tasks evals/paired/source-authority-local-lfm25-diagnostic-2026-11-23/tasks.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-lfm25-diagnostic-2026-11-23/runs/run-01"
```

The model identity is pinned in `plan.json`; LM Studio was only the selected
local provider. This exact command is historical evidence, not a portable
replay recipe. A comparable run on any OS or provider needs a new, disjoint
registration. The provider-neutral deterministic tests and offline scorers do
not require a model service.

## Captured outcome and interpretation

- Task-oracle success: 0/4 in each arm; every terminal receipt retained the
  broad `invalid_final_answer` class available in the captured evaluator.
- Candidate independent source-import probes: 2/4 passed.
- The conversation-control traces show no memory calls and a final-answer
  failure. Given the captured formation validator, that is consistent with an
  empty completion, but the raw response was not retained. Source-import traces
  show one successful `assemble_context` call before final-answer failure. The
  old class cannot distinguish malformed JSON from a JSON-shape mismatch.
- The later v3-observation/v4-receipt diagnostics add those distinctions and
  have provider-neutral fake-endpoint coverage. The capture was not replayed or
  re-scored under the later schema; its historical artifacts and scores are
  unchanged.

This is a failure-localization and runner-feasibility result. It is not a
memory-quality treatment effect, a powered comparison, or evidence that LFM2.5
is a reliable workflow evaluator. Keep LFM opt-in for transport/tool smoke.
