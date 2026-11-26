# Autonomous temporal recall: session-reuse development qualification

The exact registered development workflow is eligible: two fresh captures each
passed all four tasks in both same-product arms, **16/16 scheduled trials** in
total. Answers/citations, direct evidence outcomes, independent probes, actual
selectors and resource acceptance each passed 16/16. There were zero hard
violations and zero resource-status exits. Both the existing Rust two-capture
qualifier and the supplementary both-arm v3 auditor accepted the captures.
See `decision.json`, `qualification.json`, `audit-01.json`, and `audit-02.json`.

| Capture | Arm | Answer/source | Direct evidence | Independent probe | Actual selectors | Resource acceptance | Maximum phase |
|---|---|---|---|---|---|---|---|
| 01 | incumbent | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 12.301 s |
| 01 | candidate | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 12.242 s |
| 02 | incumbent | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 16.637 s |
| 02 | candidate | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 19.295 s |

## What was measured

The four source-first tasks cover current state, historical valid time, a public
knowledge revision checkpoint before a correction, and known_at before ledger
ingestion. Prompts supply the literal temporal axes and timestamps. The model
chooses the actual public recall query and tool selectors; this does not measure
uncued inference of temporal intent. Raw calls and responses are retained under
`public-traces-01/` and `public-traces-02/`, alongside independently scored runner
receipts under each capture directory. The ten selector comparisons and requested/
returned valid time are checked independently from the final answer. Checkpoint
reads also require the registered requested and returned revision cutoff.

The pre-ingestion unknown case forbids every seeded event source and requires
an empty read and an unknown answer. Its independent fixed probe deliberately
uses revision zero as a differing absence control. The actual agent requests
the registered known_at. This provides a causal absence check without claiming
a separate full returned-known_at attestation.

Both arms use the same immutable Apple product with explicit regex NER,
embeddings disabled and evidence stage. This qualifies the recorded lexical
development workflow, not the default Anno/semantic profile. The two executions
are independent fresh process/store executions. These sources were exposed in
the original attempt: this is not independent source confirmation, a held-out
study, a population reliability estimate, a performance comparison, or a memory
treatment effect. Stopped C1 remains stopped and C2 remains ineligible.

## Preserved original attempt and execution erratum

The original frozen attempt at `../selector-qualification/` remains intact. Its
first capture passed 8/8 trials; its second stopped during preparation under the
300-second whole-workflow bound, with no completed agent trial. It remains an
incomplete-repeat infrastructure outcome. No original result was rewritten.

Before these new model calls, `preregistration.md` and
`execution-erratum.json` registered the session-reuse repair and corrected
resource allowance. Tasks, sources, probes and quality criteria are byte-identical
to the prior attempt, as recorded by hashes. The runner retains the seeded fresh
public MCP session for that same trial's read, reducing scheduled sessions from
33 to 17 per capture without sharing trial stores, histories, responses or
independent probes. The runner repair was independently tested by its owning
agent (52 plus 17 tests); session-count reduction is a lifecycle property, not
a measured speed claim.

The new whole-workflow hard bound is 600 seconds. Observed total execution was
432.967 seconds; capture CLI wall times were 204.430 and 228.249 seconds. Observed
agent phases summed to 203.702 seconds. Each phase must be at most 25 seconds,
three turns and 384 output tokens to be accepted; observed maxima were 19.295
seconds, two turns and 145 tokens. The at-most-400-second sum for sixteen accepted
phases is arithmetic, not a hard in-flight cancellation guarantee. A blocking MCP
call has its own transport ceiling and can finish after the 25-second agent
budget; late Completed receipts are explicitly rejected. Transport cancellation
remains technical debt. No 300-second aggregate model-work guarantee is claimed.

## Frozen identities and grading enforcement

All 21 registered inputs passed SHA-256 verification before and after execution.
The freeze manifest SHA-256 is
`3115cdbb1091133f88020a95c1b6f5fded213e99c96d2beb2f66dc25aed00e87`.
The immutable binaries are under
`target/evals/memory-quality-repairs-2026-11-26/optimized-bin/`:

- Product: `c91d2d2eb7682e448292ffaa33ef0b52553cb1e36eb011500c4e494ed628acb7`.
- Evaluator: `fa9b32f77b05320f89cda57b8fab5678828c2aac9eabaa32bcd863805152509d`.
- Active auditor: `evals/paired/temporal_read_audit.py`, schema
  `temporal-both-arm-audit/v3`, SHA-256
  `a65011c57ecc01e2caaf168548625f8bc73baaeec1a1ed94d07067a9371d908e`.

The Rust qualifier retains its candidate-scoped API. Eligibility here additionally
requires the reusable auditor's complete coverage and raw-read checks for BOTH
arms. Its explicit checks survive optimized Python. It rejects missing/duplicate
reads, absent/mismatched selector evidence, incorrect trace linkage, empty task
sets, incomplete scheduled arms and late or oversized Completed receipts.
Historical frozen v1 and corrected v2 graders and their regrading receipts remain
in the original attempt; v3's additional budget enforcement was registered before
these calls. `auditor-v3-resource-enforcement.json` records the observed RED and
GREEN checks. Relevant public test commands were:

```sh
python3 -m unittest discover -s evals/paired -p test_temporal_read_audit.py -v
PYTHONOPTIMIZE=1 python3 -m unittest discover -s evals/paired -p test_temporal_read_audit.py -v
rtk cargo test -p eval-harness --lib paired_agent::independent::tests
```

Results: 10/10 auditor tests in each interpreter mode; 14 Rust tests passed,
one ignored. Negative regressions cover correct answers with missing or corrupt
actual temporal reads and resource violations, including optimized Python.

`execution-receipts.json` preserves exact CLI commands, process IDs, exits and
elapsed times. Both CLI invocations and audits exited zero. The owned `scratch/`
directory was empty after successful trial cleanup and was removed after raw
receipts were finalized. No raw receipts or traces were deleted. The coordinating
agent stopped the owned direct local MLX server after both captures finished.
Shared `../server-config.json` and `../model-assets-before.json` document runtime
and local model inventory; these inventories do not attest every server-loaded
byte. The coordinator rehashed all 13 model files unchanged after capture. This
used direct loopback MLX with the local Qwen3.8-27B MLX model, without downloads,
remote inference or an LM Studio runtime.
