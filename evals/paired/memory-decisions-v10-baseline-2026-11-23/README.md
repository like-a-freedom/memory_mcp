# Memory decision v10 baseline

**Purpose:** deterministically compare the product-default `evidence` rollout
with `shadow` for decision readiness, while keeping evidence retrieval and
ordinary unscoped reads as separate measurements.

This is a local public-MCP evaluation. It does not invoke an agent, model,
Codex, LM Studio, API key, or external service. Each case is ingested and
queried in a fresh embedded store. Four runs use the same product executable
and corpus; only the claim rollout stage differs.

## Why oracle v2 exists

The v9 oracle v1 expected `current` on three ordinary non-snapshot reads.
That contradicts [ADR-0083](../../../docs/adr/0083-revision-bounded-temporal-context.md),
which says temporal roles are derived from revision-visible relations and must
not be manufactured on an ordinary read. The old v9 cases and artifacts were
preserved unchanged. Oracle v2 rejects expected roles unless the probe has
separate valid-time and knowledge-revision cutoffs, so role scores correspond
to an observable product contract.

The new v10 cohort has eight synthetic families, disjoint from v1-v9:

- six probes exercise exact snapshot roles, temporal selection, conflict, or
  correction relations;
- one ordinary location read checks evidence without requiring a temporal
  role;
- one absent-target read checks ordinary abstention against an unrelated
  distractor.

## Results

| Rollout | Runs | Primary decisions | Temporal decisions | Evidence | Temporal evidence | Invalid |
| --- | --- | --- | --- | --- | --- | --- |
| `default` (`product_default` / `evidence`) | `run-1795438241`, `run-1795438643` | 8/8 each | 7/8 each | 8/8 each | 8/8 each | 0 |
| `shadow` | `run-1795438449`, `run-1795438807` | 2/8 each | 1/8 each | 8/8 each | 8/8 each | 0 |

The paired primary comparison gives `evidence` six wins, zero losses and two
ties. All six wins are role/relation cases. Evidence recall is unchanged at
8/8. The remaining temporal miss in both arms is the empty partial snapshot
for the absent-target query. Ordinary retrieval correctly abstains; a partial
historical snapshot correctly cannot establish global absence. The scorer
reports that temporal result as unobservable rather than inventing a negative
fact or role.

Each within-arm run pair passed `compare-decision-replays`. Default runs took
160,364 ms and 158,081 ms; shadow runs took 160,439 ms and 156,474 ms. The
profiles are deliberately gate-free: the `QUALITY FAILED` verdict for shadow
is an expected diagnostic difference, not invalid execution. This is a finite
synthetic baseline, not powered statistical evidence or proof of broad agent
utility. These families are now exposed and may serve as regression cases,
not independent confirmation.

## Reproduction

Build the product and run each profile twice with a new artifact root and
report path on every invocation. Example for the default arm:

```sh
MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp" \
MEMORY_DECISIONS_PRODUCT_REVISION=master-v10-baseline-2026-11-23 \
MEMORY_DECISIONS_ARTIFACT_ROOT=/tmp/memory-decisions-v10-default-a \
cargo run -p eval-harness --bin memory-eval --locked -- run \
  --profile evals/profiles/memory_decisions_v10_default.json \
  --artifact /tmp/memory-decisions-v10-default-a.json
```

Repeat with a fresh `-b` output for default, then use
`evals/profiles/memory_decisions_v10_shadow.json` for the two shadow runs.
Compare each arm's indexes with:

```sh
cargo run -p eval-harness --bin memory-eval --locked -- compare-decision-replays \
  --left /tmp/memory-decisions-v10-default-a/index.json \
  --right /tmp/memory-decisions-v10-default-b/index.json
```

Replace `default` with `shadow` for its pair. PowerShell users can set the
three environment variables with `$env:NAME = 'value'` before invoking the
same `cargo run` command. The harness and deterministic scorer are provider
independent.

## Captured identity

- Base `HEAD`: `1ae1da4a4e3120f2de94753fc1a949caee3aced7`; working tree was dirty.
- Product executable SHA-256: `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`.
- Combined corpus SHA-256: `19b4e9890df5f5706cf99b97638341c2db1ea54dc86f16e8f4e04696e9caaa0e`.
- Input-schema SHA-256: `9475b795bde1360eda3cf672b8c75d99ff1fa6cd0e91b219907a7fc81c71880f`.
- Default / shadow config SHA-256: `17ff6b862de83dce474a4e94ab75db5c32b7965b4b8e8265002c338df04de892` / `d52b1ccd6308b544317bb2b9ca8b10ca5fcfe9f6d3a39c59419db0a5ee42755a`.
- Default / shadow profile SHA-256: `3cb72b4cece52519aa0a7225660c47fcbe3ec2106b84d44026da526a6892a22e` / `997e872043c493827f4e1daf666a80bcf9b21137d01e7f0a705eee5b2be59602`.
- Runtime: `rustc 1.100.0`, macOS/aarch64, debug build, no optional features.

`SHA256SUMS` binds the frozen corpus, profiles, reports, receipt indexes and
per-case raw responses. The v10 corpus is also listed in
[`docs/evals/README.md`](../../../docs/evals/README.md), and its interpretation
is reconciled in
[`docs/evals/MEMORY_DECISION_BASELINE.md`](../../../docs/evals/MEMORY_DECISION_BASELINE.md).

## Model-run recommendation

LFM2.5-1.2B remains appropriate for a fast endpoint/tool-call smoke, not for
agent task acceptance: its four-family screen completed in 73.88 seconds but
produced 0/4 task-oracle successes per arm. The Qwen3.8-27B v3 workflow pilot
also produced 0/16 task-oracle successes per arm in both registered runs. Do
not spend another long cohort run on either current setup until a short,
disjoint model qualification trial demonstrates that the model can complete
the task protocol. Deterministic memory-quality runs need neither model.
