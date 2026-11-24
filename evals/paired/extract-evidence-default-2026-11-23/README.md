# Current product deterministic replay

**Purpose:** rerun the registered v9 default-versus-shadow claim-reconciliation
comparison on the current product binary after the bounded
`extract.include_fact_evidence` default change. This is a memory-quality
regression check for the current product and claim stage; it does not estimate
an effect of the response-default change.

## Frozen inputs and product identity

- Product binary SHA-256: `0c465a019c824602e1adc02c37b97f93ff3a6a5b98226d3007b7c3d93c990f25`
- Public tool schema SHA-256: `9475b795bde1360eda3cf672b8c75d99ff1fa6cd0e91b219907a7fc81c71880f`
- Corpus SHA-256: `40fdef00b8201de03df680d2ad9185038ead358372d5b5b3fedac66e55044ee3`
- Acceptance plan SHA-256: `a5f3746ac3e8be388f0258ce66fe9bb96bce1d34439862dd0a7075ae7d9ff561`
- Acceptance report SHA-256: `8ec4e2c221cf504a056cbfbd6fe32e5787f2434556530e63d8fb948a02973079`

The four captures use the same product binary and differ only in the explicit
claim-reconciliation stage. Each profile contains eight fixed histories and
was run twice.

## Results

| Profile | Decisions | Evidence |
| --- | ---: | ---: |
| `default` | 5/8 | 8/8 |
| `shadow` | 1/8 | 8/8 |

The acceptance report recorded four per-case decision gains, zero losses,
zero evidence regressions, zero hard failures, and identical scored repeats.
The frozen gate accepted. The family-cluster descriptive gain estimate was
0.5 with interval `[0.125, 0.875]` across eight families; this is a finite
synthetic cohort, not population or agent-utility evidence.

The individual `memory-eval run` commands report `QUALITY FAILED` because
their standalone quality floor is higher than these finite-cohort scores. The
registered paired acceptance test then applies its separate frozen comparison
rule and passes. No cases were invalid.

## Reproduction

From the repository root, with `target/debug/memory_mcp` built from the same
checkout:

```sh
MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp" \
MEMORY_QUALITY_GATE_OUTPUT="$PWD/target/fresh-claim-rollout-output" \
cargo test -p eval-harness --test deterministic_quality_gate --locked -- --ignored
```

The four capture directories and `claim-rollout-default-acceptance.json` are
retained alongside this report. The response-default-specific checks are
recorded separately at
[`extract-evidence-default`](../../independent-probes/2026-11-23-extract-evidence-default/README.md).

## Fresh current-checkout replay — 2026-11-23

After the archived gate, two new `memory_decisions_v9_default` captures ran
with the registered `ci-current` identity and the same product binary SHA.
Both matched both archived default replays with `compare-decision-replays`:
5/8 primary decisions, 8/8 evidence, 7/8 temporal probes, and zero invalid
cases. The second ran with `PATH=/usr/bin:/bin` and `CODEX_API_KEY` unset; it
completed all eight public-MCP cases without a Codex executable or agent
runner. Each standalone run returned quality exit 1 because its primary score
is below the diagnostic profile floor; this does not change the registered
default-versus-shadow acceptance. The reports and receipts are in
[`current-audit-repeat/`](current-audit-repeat/).

- Registered-identity `index.json` SHA-256: `fb8dd3e09ebce4f194a08b42a6af0350b58e3e10dbf1a3cada1b8142ec2a1e1f`
- Registered-identity `run.json` SHA-256: `78b01cde798dc34c65d9ac965511b14a1a5e671c424c01cd52d86c1af541602e`
- No-Codex `index.json` SHA-256: `f7485eb70794ab36a296a0064f4febcdc7cc9d86ceab73c5af9a92d488ad386b`
- No-Codex `run.json` SHA-256: `92dbb5bea5eeef3b6ed13103938302936b70b489b40c5875d44a158edff94d28`
