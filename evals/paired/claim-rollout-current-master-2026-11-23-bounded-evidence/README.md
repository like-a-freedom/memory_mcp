# Current-master v9 claim-rollout replay

The deterministic quality gate was rerun on the current product binary after
adding the bounded opt-in fact-evidence response. The product binary SHA-256 is
`58695c4ab2f0da8f5897bb677e51ebb48247d993ccffd6207b42aad77a09c231`.

The frozen v9 corpus was captured twice for each configuration. Every replay
matched exactly. The acceptance report is
[`claim-rollout-default-acceptance.json`](claim-rollout-default-acceptance.json).

| Claim-reconciliation stage | Decision success | Evidence success |
|---|---:|---:|
| Default `evidence` | 5/8 | 8/8 |
| Rollback control `shadow` | 1/8 | 8/8 |

Acceptance passed with four decision gains, no losses, no evidence regressions,
and no candidate safety findings. The test completed successfully in 653.85 s.

This validates the already-default **claim-reconciliation rollout stage** on
the frozen v9 cases. It does not evaluate whether
`extract.include_fact_evidence` should default on; those are separate features
with separate inputs, outputs, and acceptance criteria. The associated agent
response study is documented in
[`agent-managed-maintenance-v5`](../agent-managed-maintenance-v5/README.md).
