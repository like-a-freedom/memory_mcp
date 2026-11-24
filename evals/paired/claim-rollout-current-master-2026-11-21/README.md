# Claim-stage default: current-checkout deterministic replication

This archive compares the product-default `evidence` stage with explicit
`shadow` on the public MCP path. Each arm has two fresh captures, and each
case runs against its own embedded store. The comparison is same-binary and
same-corpus; no model runner, Codex executable, account, or API key is involved.

## Frozen identities

`preregistration.json` and `acceptance-plan.json` were written before these
captures. The run is pinned to:

| Input | SHA-256 |
|---|---|
| Product executable | `320707567b63d8d816326d42fc4e07321fa93ae46efca4a33cad2673dc05702e` |
| Evaluator executable | `a2875e6ec96bddbbca193502bf4f2abe00d3a0ac9d00b484abed3ac04dcca004` |
| v6 corpus | `fbe0c34e1e0ed6cea86f6ecd3470b5ef514be10040c54cb31c5f2da04e478735` |
| Acceptance plan (canonical serde JSON) | `742a78c11755647a6df2318e5a369766fd67348ea2a739c2f64f4eb0a6280b07` |

The product identity is `current-checkout-worktree`: the workspace had local
changes at capture time. The binary hash pins the evaluated executable, but
this is not described as a clean-commit build. The eight scenario templates
were already evaluated in an earlier v6 cohort. Treat this as a deterministic
replication on a fresh local build, not an independent task-family sample or
population estimate.

## Frozen decision rule

The plan compares explicit `shadow` as incumbent with product-default
`evidence` as candidate. Acceptance requires two distinct captures per arm
with repeat-identical normalized semantics, exact eight-case coverage, the
same product binary/revision, corpus and public input schema, at least 25
percentage points of equal-family decision gain, no per-case evidence or
ordinary-decision regressions, no new candidate safety findings, and no more
than five returned items. Shared safety findings are reported separately;
acceptance does not mean zero absolute safety findings.

## Captures and report

Raw run reports, indexes and immutable receipts are stored in `shadow-first/`,
`shadow-second/`, `default-first/`, and `default-second/`. The machine-readable
acceptance result is `acceptance.json`; the normalized repeat comparison and
per-case outcome summary are `comparison.json`. The separate persisted-claim
regression report is `claim-reconciliation-regression.json` (41/41 cases,
precision/recall/F1 all 1.0; SHA-256
`7204e22d77e17971d420646f4edd92353296a063b62a82a23667b13bbac40a8d`).

The memory-decision run artifacts can show `QUALITY FAILED` because those
profiles intentionally have no per-case promotion gates and four or more cases
are not fully observable at either stage. All captures were valid and covered
8/8; the separately frozen paired acceptance CLI is the stage-transition
decision. `QUALITY FAILED` here is a diagnostic quality outcome, not an invalid
run or harness execution failure.

| Metric (8 cases) | Shadow | Evidence default | Change |
|---|---:|---:|---:|
| Primary evidence success | 6/8 | 6/8 | 0 |
| Primary complete-decision success | 1/8 | 4/8 | +3 cases |
| Primary probe success | 1/8 | 4/8 | +3 cases |
| Temporal diagnostic evidence success | 7/8 | 7/8 | 0 |
| Temporal diagnostic complete-decision success | 0/8 | 6/8 | +6 cases |

The frozen primary acceptance passed: three decision gains, no decision
losses, no primary evidence regressions, and no *new* candidate safety
findings. Two shared baseline source-safety findings remain
(`similar-name:forbidden_source`, `valid-boundary:forbidden_source`); this
rollout does not fix those cases. Repeat comparisons matched for both stages.
The deterministic acceptance report's descriptive resampling interval is
0.125–0.75 on eight scenario families; because this is a finite replication of
previously seen templates, it is not a population confidence claim.

The scores are public-response evidence and decision measures for these fixed
scenarios. They do not measure general agent usability, productivity, or
performance under a population workload. Separate downstream agent evaluation
is required for those claims.

## Current-checkout quality-gate replay

After the archive above was captured, the ignored deterministic quality-gate
test was run against the current checkout's product binary. Its fresh artifacts
are under `ci-gate/`. This run also re-executes the frozen v4 legacy acceptance
profile; that check passed. For v6, the same current product binary was used for
the explicit Shadow control and the default Evidence candidate, with two fresh
captures per arm. The v6 acceptance passed with three decision-family gains,
zero losses, no primary evidence regressions, and repeat-identical semantics.
The primary scores were Shadow 1/8 to Evidence 4/8 for complete decisions and
6/8 to 6/8 for evidence success. Two shared baseline safety findings remain
(`similar-name:forbidden_source`, `valid-boundary:forbidden_source`).

The machine-readable v6 report is `ci-gate/claim-rollout-default-acceptance.json`;
the v4 compatibility report is `ci-gate/acceptance.json`. The successful local
gate command was:

```sh
MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp" \
MEMORY_QUALITY_GATE_OUTPUT="$PWD/evals/paired/claim-rollout-current-master-2026-11-21/ci-gate" \
cargo test -p eval-harness --test deterministic_quality_gate --locked -- --ignored
```

This is a deterministic acceptance check on the fixed corpus, not proof of
population-level improvement or general agent usability.

## Reproduction

From the repository root, build the pinned binaries and ensure their hashes
match `preregistration.json`. Use fresh artifact roots for all four captures.
The capture profile commands are the ones named in the two profile files in
`evals/profiles/`: `memory_decisions_v6_shadow_control.json` and
`memory_decisions_v6_default_evidence.json`. For each run set
`MEMORY_DECISIONS_SERVER_BIN`, `MEMORY_DECISIONS_PRODUCT_REVISION`, and a fresh
absolute `MEMORY_DECISIONS_ARTIFACT_ROOT`, then invoke `memory-eval run`.
Compare each arm's two indexes with `memory-eval compare-decision-replays`,
then run `memory-eval accept-memory-candidate` with the frozen plan and the
four indexes. Do not reuse receipt directories.
