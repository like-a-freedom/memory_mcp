# LongMemEval-V2 Small web text baseline

Run on 2026-11-24 on Darwin arm64 using the [frozen profile](../../evals/longmemeval_v2/small-web-native-profile.json) and the [pinned upstream scorer](../../evals/longmemeval_v2/README.md#local-scored-small-web-subset). The native release binary was built from product commit `3260f7bb`. Standalone local MLX-LM served the reader and judge over explicit loopback HTTP. Inference cost through external services was zero.

## Coverage

The pinned Small dataset has 451 questions. The [pre-score manifest](../../evals/longmemeval_v2/small-web-text-cohort.json) selects seven web questions grounded in three complete accessibility-tree observations. The selection receipt excludes 15 image questions, 218 web questions without modality adjudication, and 211 enterprise questions outside this slice. Every selected question retained the same original ordered 100-trajectory web haystack. The build restored from an isolated embedded snapshot and scored against separate copies for each read.

The selected cases cover static state and false premises about static state. They provide no estimate for the rest of Small or for other abilities. The surrogate validity date is fixed; the database writes use the build's wall clock.

## Initial run

| Measure | Value |
| --- | ---: |
| Native upstream `overall_full_set` | 0/7 |
| Static state, positive | 0/4 |
| Static state, false premise | 0/3 |
| Coverage, scored with successful reader and judge stages where applicable | 7/7 |
| Failure-inclusive accuracy on the frozen selection | 0/7 |
| Reader failures / judge failures | 0 / 0 |
| Shared build wall time | 72 min 04 s |
| Scoring wall time including preparation | 28 min 44 s |
| Reader requests, summed | 38.942 s |
| Judge scoring calls, summed | 25.319 s |
| Median / slowest public memory read | 441.544 s / 1,030.379 s |

The native aggregate was independently recomputed from all seven native result rows. All seven reader responses were `unknown`, which the upstream scorer marks as incorrect. [Native aggregate](../../evals/longmemeval_v2/results/2026-11-24-small-web-text-baseline/initial-aggregated-metrics.json) · [Coverage receipt](../../evals/longmemeval_v2/results/2026-11-24-small-web-text-baseline/initial-score-coverage.json) · [Selection receipt](../../evals/longmemeval_v2/results/2026-11-24-small-web-text-baseline/selection.json)

Case results and context sizes follow.

| Question ID | Class | Score | Tokens before cap | Tokens sent to reader from memory |
| --- | --- | ---: | ---: | ---: |
| `0c6b0c60` | Positive | 0 | 1,497,085 | 0 |
| `6d975a08` | Positive | 0 | 1,060,048 | 0 |
| `89a20f48` | False premise | 0 | 700,220 | 0 |
| `922a0c56` | False premise | 0 | 1,250,253 | 0 |
| `d6da3938` | Positive | 0 | 1,393,011 | 0 |
| `f43926d9` | False premise | 0 | 720,346 | 0 |
| `f61a096f` | Positive | 0 | 1,585,960 | 0 |

## Same-snapshot repeat

The repeat again scored all seven cases with no reader or judge failures. The native total was 0/7, including 0/4 positive cases and 0/3 false-premise cases. The scoring wall time was 32 min 03 s. Reader requests took 31.688 s and judge scoring took 25.170 s. The median public memory read took 458.582 s and the slowest took 1,214.020 s.

Case order, final prompt hashes, exact reader answers and case scores matched in both runs. The saved store also matched the original build store by file digest. The number of tokens returned by memory before the cap changed for six of seven queries. Both runs sent zero memory tokens to the reader on every case. The observed score is reproducible for this profile; the raw memory output is not stable across these two runs. The artifacts do not isolate the cause of that variability. [Native repeat aggregate](../../evals/longmemeval_v2/results/2026-11-24-small-web-text-baseline/repeat-aggregated-metrics.json) · [Repeat coverage](../../evals/longmemeval_v2/results/2026-11-24-small-web-text-baseline/repeat-score-coverage.json) · [Case comparison](../../evals/longmemeval_v2/results/2026-11-24-small-web-text-baseline/replay-comparison.json)

## Observed memory failure

The public memory read returned four or five items per question. Each first item alone exceeded the upstream context cap of 4,096 tokens. Upstream keeps the longest fitting prefix of complete items, which was empty for every case. Since the reader got no evidence, this run cannot measure whether the relevant evidence was ranked low or omitted before truncation.

A candidate memory change should first have distinct deterministic regressions through the public ingest, extract and read path. Exercise oversized source material, verify that a bounded read preserves the decisive evidence and its attribution, and require stable surviving evidence across restored reads. The same benchmark profile can then measure changes in evaluated accuracy, surviving evidence and read latency.
