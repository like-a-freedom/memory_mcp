# Claim rollout local performance characterization

Two single-run, 100-sample-per-scenario profiles on macOS/aarch64, local host,
debug binaries. The read scenario measures `assemble_context` with a persisted
claim relation at the selected rollout stage; the write scenario measures the
existing claim write path at scale 100. These are descriptive timings, not a
paired significance test, stable CI threshold, or proof of no performance
regression. Both full raw artifacts are preserved here.

| Stage | Scenario | p50 | p95 | p99 | Mean |
|---|---|---:|---:|---:|---:|
| Shadow | assemble, scale 2 | 10.141 ms | 11.443 ms | 12.195 ms | 10.286 ms |
| Evidence | assemble, scale 2 | 10.103 ms | 10.729 ms | 10.984 ms | 10.160 ms |
| Shadow | claim write, scale 100 | 396.600 ms | 516.765 ms | 536.144 ms | 397.761 ms |
| Evidence | claim write, scale 100 | 404.027 ms | 515.083 ms | 543.302 ms | 403.117 ms |

The stage-gated read showed no latency increase in this single capture. The
write-path p50 and p99 were about 1.9% and 1.3% higher under Evidence, while p95
was about 0.3% lower. With one run per stage, this small difference cannot be
separated from run-to-run noise. The older `<500 ms` proposal measured a
reconciliation candidate page, not this entire write operation; it must not be
treated as passed by these results. The stage comparison's functional
acceptance is in
[`the deterministic rollout archive`](../../../paired/claim-rollout-current-master-2026-11-21/README.md).

Artifacts: `shadow.json` (SHA-256
`19b2c4df27988cdc033ae311349c60c9c3cbb9aa141a716285b7196a85cca328`) and
`evidence.json` (SHA-256
`ab8f85a081908e2c2709fe0ca094f43211aa7ac206bbcb249e22e69249fb8a39`).
