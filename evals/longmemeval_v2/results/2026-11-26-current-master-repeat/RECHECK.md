# LongMemEval-V2 post-B5 current-master recheck

**Run date:** 2026-11-26 · **Host:** native Apple Silicon (`arm64`) ·
**Runtime:** standalone local MLX-LM loopback; no LM Studio application,
server, CLI, remote model, or provider credential.

The reader and judge weight files were checked against the pinned native
profile and every listed SHA-256 matched. The weights are stored under a cache
directory named `lm-studio`; that is only the file location, not the inference
runtime.

## Result and limits

The full frozen seven-case run built all seven prompts and completed all seven
memory reads. Every returned context was nonempty (1,473–3,598 tokens) and none
was truncated. The pinned upstream scorer then stopped on an empty judge
response for case `89a20f48`. Its failure-inclusive receipt records two native
scored rows, one judge error, four unscored cases, and **unavailable official
aggregate metrics**. The two partial rows are not an accuracy estimate.

To measure the saved memory results without rerunning slow reads, a paired
reader/judge replay used the saved B5 and current-master contexts on the same
seven IDs. Both arms used the same local model aliases, temperature 0, concise
output suffix and pinned upstream scoring predicates. No memory re-query was
made. The paired result is diagnostic rather than an official full
LongMemEval-V2 run:

| Measure | B5 context | Current master | Observed difference |
| --- | ---: | ---: | ---: |
| Pinned exact-match predicate | 1/7 | 3/7 | +2 cases |
| Strict phrase-set equality, list cases | 0/4 | 2/4 | +2 cases |
| Complete reviewed source span in context | 1/7 | 2/7 | +1 case |
| Mean memory-query latency | 630.4 s | 588.0 s | −6.7% |
| Median memory-query latency | 437.7 s | 423.7 s | −3.2% |
| Maximum memory-query latency | 1,011.5 s | 991.3 s | −2.0% |

## Case-level interpretation

The deterministic strict phrase-set metric confirms two correct list answers:
`6d975a08` (Forums, Tags, Alphabetical) and `d6da3938` (Reset Password).
The complete reviewed source span is present for `6d975a08`; for `d6da3938`,
the exact answer text is present but the whole reviewed span is not, so the
span metric is conservative. The pinned predicate also passes `0c6b0c60`, but
the answer adds “Past hour,” which is not in the memory context; the strict
phrase-set metric correctly rejects this unsupported extra. The three
false-premise cases remain wrong: `89a20f48` answers “Past year,” `922a0c56`
answers “Send Email,” and `f43926d9` answers “Forums.” The last case does
contain its complete reviewed span, showing that delivery of evidence alone
does not ensure the agent rejects a false premise. `f61a096f` answers “two”
instead of “nine.”

This is a favorable signal on one small fixed cohort, not a guarantee of
general accuracy or speedup. The pinned exact-match score includes local
model-based judging for abstention cases. Strict phrase-set equality and
reviewed-span presence are deterministic, but the latter is a conservative
full-span check rather than a complete semantic evidence metric. The mean
query-time difference is descriptive for seven reads in one capture.

## Harness defect found and fixed

The empty judge response aborted upstream scoring after reader answers had
already been generated, but the old observer had not saved those answers. This
made the unscored suffix impossible to judge offline. A focused regression
first failed when reader output was absent, then passed after
`scored_coverage.py` began atomically persisting `reader_outputs.jsonl` after
each response. It stores the answer, usage, status, and prompt/context hashes;
it does not duplicate prompts or memory content. Reader failures are also
persisted by status without exception text. The full focused LongMemEval test
suite passes.

## Artifacts

- [Execution manifest](execution-manifest.json)
- [Original full-run coverage](run-01/score-coverage.json)
- [Per-case memory evidence](run-01/reviewed-span-coverage.json)
- [Paired B5 comparison](run-01/scoring-replay-concise-v1/paired-b5-comparison.json)
- [Current paired replay](run-01/scoring-replay-concise-v1/replay.json)
- [Deterministic strict phrase-set result](run-01/scoring-replay-concise-v1/strict-phrase-set-accuracy.json)
- [Reader answers from the paired replay](run-01/scoring-replay-concise-v1/reader_outputs.jsonl)
