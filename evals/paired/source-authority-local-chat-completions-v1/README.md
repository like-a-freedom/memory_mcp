# Local provider-neutral source-authority pilot v1

This is a development pilot of two workflows using the same memory product
binary. It is not a memory implementation comparison, deterministic quality
acceptance, powered study, or promotion gate. See `preregistration.md` for the
question, limits, cohort, budgets, and interpretation rules.

## Frozen inputs

- `plan.json` and `tasks-v1.json` register 16 synthetic task families: eight
  conversation controls and eight source-file imports.
- `fingerprints.json` records the CLI-generated task/protocol hashes plus the
  model endpoint, evaluator, and product identities.
- `provider-inventory.json` records this laptop's selected provider and what
  it exposes about the model revision and context window.
- `protocol-config.json`, `simulator.json`, host/retry policies, and `prompts/`
  preserve the protocol and prompt assets associated with the registration.
- `freeze-manifest.sha256` binds every registration input other than itself.

Verify the registered files from this directory with:

```sh
shasum -a 256 -c freeze-manifest.sha256
```

The no-model preflight test validates the registered task coverage, protocol
hashes, and endpoint pin without contacting the model. It substitutes the
test executable for the CLI/product binaries so it can exercise the public
preflight path in any development environment; the actual local binary hashes
for this capture are recorded separately in `fingerprints.json`:

```sh
cargo test --locked -p eval-harness --lib registered_local_pilot_inputs_pass_preflight_without_contacting_a_model
```

The provider-neutral runner contract is tested separately by
`cargo test --locked -p eval-harness --test chat_completions_runner`. It uses an
in-process fake HTTP server and fake MCP session. It does not require LM
Studio, another model service, a Codex key, or any provider CLI, and can be run
with Cargo on Windows, Linux, or macOS. This registration and its two model
captures are specific to the pinned artifacts and local endpoint on the
registering laptop.

## Optional local captures

The endpoint used for this pilot is numeric loopback, but that cannot prove the
provider process does not forward its requests. All registered task content is
synthetic. No Codex key or bearer token is needed for this run.

From the repository root, run each capture once into a new directory, without
changing plan, tasks, prompts, product binaries, or budgets between captures:

```sh
env -u CODEX_API_KEY -u MEMORY_EVAL_BEARER_TOKEN cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-chat-completions-v1/plan.json \
  --tasks evals/paired/source-authority-local-chat-completions-v1/tasks-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-chat-completions-v1/runs/run-01"
```

The runner requires absolute paths for both product binaries and the output
directory. Repeat the same command with `run-02`. Preserve failed or invalid captures;
they are not replaced by a tuned rerun. The endpoint and model fingerprint
checks happen before any trial. The report must keep the two runs as repeated
observations over the same 16 families, not as 32 independent samples.

To run a different provider or platform, use a new registration that hashes
that environment's `memory-eval` and product binaries, endpoint, model alias,
tasks, and budgets. The model-free contract tests remain provider-independent.
