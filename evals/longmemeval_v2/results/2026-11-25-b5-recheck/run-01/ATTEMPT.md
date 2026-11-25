# Attempt 01 — invalid, not scored

This attempt did not produce an official LongMemEval score. Coverage reports
0/7 scored cases and official metrics unavailable. Do not use it as evidence
for or against B5 answer accuracy.

The pinned dataset, staged cohort, saved memory snapshot, product binary and
local reader/judge endpoints passed their preflight checks. The run then spent
1,644.69 seconds building memory prompts before stopping when the upstream
Qwen3.5-9B context processor was absent from the default Hugging Face cache.
`HF_HOME` had not been set to the already-staged offline cache. Reader and
judge inference did not start.

The first attempt's `score-coverage.json` shows every case as `unscored` with
no failure entry because the old observer only recorded reader and judge
failures. The runner now preflights the context processor before prompt
construction and records failures at that stage. The successful `run-02`
retry used the explicit pinned offline `HF_HOME` and scored all seven frozen
cases; see the parent [`RECHECK.md`](../RECHECK.md).
