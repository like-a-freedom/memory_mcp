# Post-capture source formatting note

The v5 freeze manifest pins
`crates/eval-harness/src/paired_agent/codex.rs` at SHA-256
`06fe4c8a7145a0a15622e7b452fef288d3b6d373d1498b79983ddb5fbe33a893`.
After capture-01, `cargo fmt --all` changed only the line wrapping of the
`supports_agent_managed_trial()` condition in the current worktree. No behavior
changed. To keep the frozen source auditable without rewriting its manifest,
the exact pre-format file was reconstructed by reversing that one-line diff and
stored at
[`source-snapshot/crates/eval-harness/src/paired_agent/codex.rs`](source-snapshot/crates/eval-harness/src/paired_agent/codex.rs).
Its SHA-256 matches the registered digest. The capture's product and evaluator
binary hashes are independently recorded in
[`agent-managed-comparison.json`](agent-managed-comparison.json).

The source change happened after the capture and after the registered stop
decision. No further capture was run. The frozen plan, prompts, tasks, profile,
budgets, model and criteria remain unchanged.
