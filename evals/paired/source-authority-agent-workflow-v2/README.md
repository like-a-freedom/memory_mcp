# Source-authority paired-agent evaluation v2

Status: frozen exploratory plan; no model run was executed.

This registration retries v1 after two technical failures, with no task, oracle,
product or model outcome available from v1. The 16 task inputs, counterbalanced
order, product binaries, model alias, prompts, budgets, schemas and operator
probes remain byte-identical to v1. The evaluator binary identity is updated to
include the source-root routing regression fix. All frozen inputs pass
`freeze-manifest.sha256`.

The runner keeps the Codex workspace empty in both arms. The incumbent stdio
MCP child alone receives the evaluator-owned fixture root as its cwd so the
clean-HEAD product can resolve its registered relative file path. Candidate
operator import runs from that same root before the candidate agent starts; the
candidate agent and MCP server retain the empty task workspace. The public
runner regression verifies both routes and an independent source probe.

Verification completed:

- Before the attempted invocation, `source_authority_codex_runner`,
  `independent_probe`, `agent_managed_trial`, and `codex_agent_runner` passed:
  48 passed, 1 ignored; plan and evaluator/product/task hashes matched.
- After the approval review stopped the invocation, full `eval-harness` passed:
  693 passed, 22 ignored; workspace strict Clippy and
  `cargo fmt --all --check` passed.

## Attempt status

The planned `run-paired-codex` invocation was rejected by the automatic
approval review before process creation. The review cited the possibility that
the paired task prompts, memory responses and source-fixture text would be sent
to the external Codex service, and said the prior laptop authorization did not
explicitly approve that exact payload and destination. No model call, external
transmission, receipt or run directory was created. The attempted command
removed `CODEX_API_KEY` from its environment and named the existing local
Codex home; no missing key caused this stop.

Until the user explicitly authorizes this data-transfer scope, do not run the
live paired study. The result remains unmeasured: deterministic product
preflight and fake-Codex runner tests establish wiring and isolation, not real
agent usability, non-inferiority, or memory-quality improvement.
