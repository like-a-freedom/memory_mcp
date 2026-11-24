# Local source-authority paired pilot v2

This preregistered exploratory pilot compares two ways to prepare the same
memory product for a fixed recall task. Both arms use the same product binary;
the pilot can measure workflow feasibility and interaction cost, but cannot
estimate a memory implementation effect or prove agent benefit.

## Frozen registration

- `tasks-v2.json` registers 16 synthetic families: eight conversation controls
  and eight source-file imports. Their family names and facts are disjoint from
  the earlier paired task manifests in this repository.
- `plan.json` pins the exact arm order, budgets, current product binary,
  evaluator binary, model alias, endpoint configuration, and protocol hashes.
- `fingerprints.json` records the CLI-computed corpus, oracle, prompt, policy,
  evaluator, product, and endpoint identities.
- The prompt assets, product configuration, protocol, host and retry policies,
  simulator, and provider inventory document the frozen registration.
- `freeze-manifest.sha256` binds the registration inputs other than itself and
  the later `runs/` outputs. A first no-model preflight caught copied v1
  corpus/oracle hashes in the v2 plan; those two fields were corrected to the
  v2 CLI fingerprints before any provider request. The rejected preflight is
  preserved in `runs/attempt-history.json`.

Verify the registration from this directory:

```sh
shasum -a 256 -c freeze-manifest.sha256
```

The deterministic contract tests do not use this model or endpoint. They run
with an in-process fake HTTP server and fake MCP session:

```sh
cargo test --locked -p eval-harness --test chat_completions_runner
```

The optional captures below use only synthetic task text. The model service
reports a local loaded model and is contacted through numeric loopback. That
does not prove the provider process makes no unrelated network connections;
no private or project memory is in the registered task data. No Codex API key
or bearer token is required for these commands.

## Captures

Use the exact frozen plan and tasks twice, in separate output directories. Do
not change prompts, tasks, binaries, model alias, endpoint, or budgets between
captures. Preserve every failed or partial output; do not retry a failed run
into the same directory.

```sh
env -u CODEX_API_KEY -u MEMORY_EVAL_BEARER_TOKEN \
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-chat-completions-v2/plan.json \
  --tasks evals/paired/source-authority-local-chat-completions-v2/tasks-v2.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-chat-completions-v2/runs/run-01"
```

Repeat with `runs/run-02`. The evaluator requires absolute paths for the product
binaries and output directory. On Windows, use the built `memory_mcp.exe` path
and an absolute `Join-Path` output path; the deterministic contract command
above remains the same in PowerShell because it does not need an endpoint.

## Interpretation

The incumbent agent reads a synthetic relative-path source through `ingest`
and `extract`. In the candidate arm, the operator imports and extracts the
same source before a read-only agent session. Conversation controls use the
same product behavior in both arms. A separate fixed public-MCP probe checks
the stored fact after each workflow.

Report task success, direct-memory observation, independent-probe success,
unsupported or prohibited evidence, hard violations, tool calls, model usage,
wall time, and operator setup cost separately. A failed or missing receipt is
not a pass. If both captures complete, repeat observations remain nested
within the same 16 families; they are not 32 independent samples. This pilot
has no power analysis, promotion criteria, or memory-treatment contrast. It
cannot establish agent benefit, general workflow usability, or equivalence.
