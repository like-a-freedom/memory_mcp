# Pre-capture audit — Ornith clarification-trace fix diagnostic

Completed before the first model-task request for this registration.

## Checks

- The preregistered family IDs (`stormpetal_index`, `ironwillow_registry`, `glassharbor_depot`, `bluecinder_lockbox`) were searched as exact strings under all `evals/` JSON and JSONL files. No overlap was found.
- `memory-eval hash-chat-completions-tasks` produced the endpoint, sampler, corpus, oracle, prompt, host-policy, retry-policy, protocol and simulator fingerprints recorded in `fingerprints.json`; the matching values are in `plan.json`. Task keys match the four planned cases and strata.
- Product binary SHA-256 is `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`; evaluator CLI SHA-256 is `c9299fdf3425f4b356784e027020407689cbdac2c0c32bf6d8a6248aee89f533`. Both hashes match the plan. The evaluator binary includes the clarification trace event fix.
- `lms ps --json` showed `memory-local-eval-ornith-v2` loaded/idle with 16,384 context and one parallel request; CLI commit `dcefd5a`; `/v1/models` returned HTTP 200. The provider-neutral local compatibility smoke passed with exactly one public `assemble_context` call in 1.95 seconds. No bearer token was used.
- The endpoint is numeric loopback. Only the four frozen synthetic facts are sent to the configured local service. No model response text is stored by the harness.
- The focused clarification-trace regression passed, full eval-harness passed (771 passed, 23 ignored), strict workspace Clippy, formatting and `git diff --check` passed before freeze.
- Prior cohorts, including the exposed invalid v5 registration, remain immutable.

## Limits

This audit establishes input disjointness, identity and tool-call compatibility. It does not establish task quality, memory improvement, or treatment effect. Preserve every receipt even if this single registered capture is invalid, inconclusive, or fails its gates.
