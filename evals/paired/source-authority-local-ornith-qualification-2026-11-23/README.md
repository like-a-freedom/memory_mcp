# Local Ornith source-authority model screen

This is a one-capture, four-family feasibility screen. It uses the same memory product in both arms and does not estimate a memory implementation effect. The four synthetic families are disjoint from prior cohorts.

The protocol and stop criteria are in [`preregistration.md`](preregistration.md). Verify the frozen inputs before running:

```sh
cd evals/paired/source-authority-local-ornith-qualification-2026-11-23
shasum -a 256 -c freeze-manifest.sha256
```

Provider-neutral adapter tests do not need LM Studio or credentials:

```sh
cargo test --locked -p eval-harness --test chat_completions_runner
```

Run the one registered local capture only after verifying the plan and loaded alias:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-ornith-qualification-2026-11-23/plan.json \
  --tasks evals/paired/source-authority-local-ornith-qualification-2026-11-23/tasks-ornith-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-ornith-qualification-2026-11-23/runs/run-01"
```

No API key is used. Keep raw model text out of artifacts. A passing screen permits only a separately preregistered broader pilot, not a quality or benefit claim.

The registered run completed but failed its workflow screen: task-oracle success
was 0/4 in both arms and independent probes passed 1/4 incumbent versus 3/4
candidate. The candidate's two file-import probes passed, but that does not
rescue the task gate. See the [run-01 report](runs/run-01/report.md); do not
reuse its exposed families or scale this adapter configuration.
