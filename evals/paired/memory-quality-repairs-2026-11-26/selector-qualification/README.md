# Autonomous temporal recall: first execution

**One complete capture passed; the required independent repeat is incomplete.**
This execution does not establish two-capture eligibility. Capture 02 exhausted
the stricter whole-workflow deadline during preparation, before any completed
agent trial. This is an infrastructure limitation, not an observed model or
temporal-answer failure. A separately registered execution may address that
overhead without changing the quality criteria; these results remain unchanged.

## Source truth and execution

The four source-first synthetic families cover current recall, valid time,
an exact public revision checkpoint, and knowledge before any ingestion.
See [preregistration.md](preregistration.md), [source-first-truth.json](source-first-truth.json),
[tasks.json](tasks.json), [probes.json](probes.json), [criteria.json](criteria.json),
and [plan.json](plan.json). All eleven original freeze checksums passed before
model calls; [freeze-manifest.sha256](freeze-manifest.sha256) remains unchanged.
The `*.preflight.json` and `plan.ready.json` files are superseded preparation
scaffolding, not execution registrations.

Source-only public preflight returned precisely the expected original sources
and the empty pre-ingestion result. It resolved public formation revisions to
6 per event and checkpoint 6; technical probe cutoffs are 6/12/6/0. These
preflight receipts used the earlier binary named in their receipts, not the
final immutable product. The final immutable product's tool schema was captured
separately in [immutable-identity/identity.json](immutable-identity/identity.json).

The actual model chooses the public `assemble_context` query and selectors;
the Rust runner forwards that live request and scores its answer/citations.
Exact actual requests and responses are retained in `public-traces-01/`.
The independent fixed query is a separate control, not the model's query.
The Dahlia control uses revision zero, whereas the actual model must request
the registered `known_at`; forbidden seeded sources and the empty returned
result protect this source-based absence oracle. No separate returned-known_at
metadata attestation is claimed.

Both arms use the same immutable native Apple product and the adapter's explicit
regex/no-embeddings/evidence profile. This is a lexical development screen,
not a default-Anno or semantic-retrieval qualification. Runtime is direct local
MLX/Qwen on numeric loopback, with no remote service, download or LM Studio
runtime. Parent [server configuration](../server-config.json) and
[model asset inventory](../model-assets-before.json) document the shared local
runtime; file inventory is not an attestation of server-loaded bytes.

## Observed results

| Capture | Arm | Exact tasks | Agent evidence | Independent probes | Actual selectors | Violations / agent resource exits |
|---|---|---:|---:|---:|---:|---:|
| 01 | incumbent | 4/4 | 4/4 | 4/4 | 4/4 | 0 / 0 |
| 01 | candidate | 4/4 | 4/4 | 4/4 | 4/4 | 0 / 0 |
| 02 | both | 0 completed / 8 scheduled | unobserved | 0 saved / 8 scheduled | unobserved | global preparation deadline |

All ten effective selector comparisons passed for every completed read.
Registered requested/returned valid times and the Cobalt requested/returned
revision cutoff also passed. Both-arm audit additionally requires a unique
observed successful read, nonblank actual query, raw-call hashes, and linkage
of query/non-query argument hashes to the scored read.

Capture 01 took 281.997 seconds. Summed observed agent phases were 97.814
seconds (54.212 incumbent; 43.602 candidate); the remainder includes public
formation, fresh sessions, independent probes and runner overhead. No causal
timing partition or performance comparison is inferred. The 300-second total
workflow deadline left 17.9 seconds for capture 02 preparation; its process group
was terminated with exit -15. Whole execution including final audit overhead was
300.127 seconds. See [execution-receipts.json](execution-receipts.json).

[decision.json](decision.json) preserves the 16 scheduled arm-trial denominator,
eight completed first-capture trials and eight unobserved second-capture trials.
The two-capture Rust qualifier was not run with fabricated repeat artifacts.
The first pass alone does not qualify C2, reopen stopped C1, establish general
agent stability, or estimate a memory treatment effect.

## Grader correction and tests

The original frozen `audit_capture.py` and test are retained byte-for-byte as
historical v1. Review found that Python optimization removed its `assert`
validation, and an empty manifest could qualify zero of zero. Public CLI
regressions first reproduced both failures. The active reusable grader is
[temporal_read_audit.py](../../temporal_read_audit.py); it uses explicit validation
and rejects empty/duplicate task identities. Quality criteria did not change,
and no model call was repeated for this correction.

[auditor-v2-correction.json](auditor-v2-correction.json) records exact grader/test
hashes. Regrading the identical raw captures gives
[8/8 first-capture pass](audit-01-v2.json) and
[incomplete second-capture rejection](audit-02-v2.json). The frozen v1 script is
not the final acceptance authority.

`python3 -m unittest discover -s evals/paired -p test_temporal_read_audit.py -v`
passed six CLI tests, including corrupt incumbent selectors, blank actual
queries, absent observations, empty tasks and `python -O`. The same six tests
also passed with `PYTHONOPTIMIZE=1`.

The Rust qualifier regression first failed when all four answers/probes passed
without the registered temporal read. After the registered-read gate,
`rtk cargo test -p eval-harness --lib paired_agent::independent::tests`
passed 14 tests, one opt-in test ignored. Matching captured knowledge selectors
qualify; incorrect captured selectors reject otherwise identical correct
answers/probes. Ordinary v3 tasks without read expectations keep their semantics.

All client processes were closed. Owned preflight databases and the sole scratch
directory from the terminated second capture were removed; raw MCP, trace,
source, freeze, execution and scoring receipts remain.
