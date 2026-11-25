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

## B5 deterministic retrieval regressions

Run the focused tests from the repository root on native Apple Silicon.

```sh
cargo test -p eval-harness --locked --test bounded_recall
cargo test -p memory_mcp --locked --lib fused_candidates_keep_lexical_rank_before_the_candidate_cap
```

The public tool fixture reproduced the oversized fallback with a fixed source,
query and one-item budget. The candidate returns exact UTF-8 excerpts of at
most 2,048 bytes for oversized context contents and quotes. Source identity
is retained. Fallback offsets address the original episode. Fact offsets
address each original field.
The fixtures cover a late witness, an answer adjacent to a matching window's
edge, a long extracted fact with a source quote, and two independent readers
against one store. Excerpt selection uses
lexical overlap, with interior overlap breaking ties to preserve space around
the match.

A fixed 32-candidate fusion fixture also reproduced unordered candidates
before admission applies the pool limit. The candidate folds duplicate facts
in ID order and sorts the fused candidate pool by the existing relevance
comparator with a total tie-break. This isolates one mechanism for unstable
reads. The results above for the official seven-case cohort still describe
the baseline.

A separate public CLI check used three independent restores of one synthetic
embedded store and the pinned upstream tokenizer and truncation function.
The C4 incumbent returned 31,398 bytes for the witness-bearing item.
The 4,096-token cap reduced its 8,567 tokens to zero. The two candidate
reads returned identical evidence. Each retained 564 tokens and the
witness survived the cap with its original source ID.

Two public MCP captures of the existing historical-boundaries corpus also
matched under `compare-decision-replays`. Both reproduced its recorded
2/3 decision success and 3/3 evidence success. The `absent-subject`
decision remains the failing case.

The native v9 rollout gate accepted the final executable in four captures. Both stages retained 8/8 evidence successes, with 5/8 decisions
in the default stage and 1/8 in the shadow stage. Matched repeats, zero
losses and zero hard failures reproduce the
[archived v9 control](../../evals/paired/native-rollout-v9-2026-11-24/report.md).

Two restored reads of frozen case `f43926d9` exposed a remaining path.
The fallback-only build returned identical content across both restores,
but four extracted items remained oversized. The upstream cap kept zero
tokens out of 259,374 both times. A new public ingest → extract → read fixture
reproduced this remaining failure with a 7,842-byte fact. It extends the
regression guard to both the extracted content and its verbatim source quote.

The final release build kept all five context items on both restores of
`f43926d9`. The same 4,096-token cap kept 3,598 tokens per restore.
The baseline retained zero. Both restores returned the same ordered context hash.
This was a context-only replay, so the official seven-case answer
accuracy remains measured only for the baseline.
[Context replay receipt](../../evals/longmemeval_v2/results/2026-11-24-small-web-text-baseline/b5-context-replay-f43926d9.json)

To reproduce this replay, use the existing adapter with the saved C4
`memory_state/store` and the absolute path to the candidate release
binary. Query the frozen text of `f43926d9` twice. Each
`MemoryMcpBackend.query` call opens a private clone of the store. Apply
the pinned upstream `evaluation.harness.truncate_memory_context` with
`max_tokens=4096` and the cached processor from the frozen profile.

## Post-B5 paired C3 recheck

The November 25 C3 recheck repeated the seven frozen cases on the same saved
snapshot. Its first scoring pass was incomplete because two judge responses
were empty. A later paired reader/judge replay used the saved B5 and C3
contexts with the same concise-answer instruction, local standalone MLX-LM
runtime, and pinned upstream scoring predicates. The prompt adaptation means
these are paired diagnostic scores, not an official full LongMemEval result.

| Measure | B5 context | Post-C3 context |
| --- | ---: | ---: |
| Pinned scoring predicate | 1/7 | 3/7 |
| Strict phrase-set exactness on list answers | 0/4 | 2/4 |
| Complete reviewed evidence span | 1/7 | 2/7 |
| Mean memory-query time | 630.4 s | 590.5 s |
| Median memory-query time | 437.7 s | 429.7 s |

The two strict list improvements are the forum browse tabs and the customer
password action. Three false-premise questions remain wrong in both arms: the
reader supplies “Past year,” “Send Email,” and “Forums” instead of rejecting
the premise. The official phrase-subset predicate also accepts an unsupported
extra “Past hour” in the answer to `0c6b0c60`; the supplemental exact-set
metric rejects it. The full seven-case results, per-case evidence and
limitations are in the [C3 recheck report](../../evals/longmemeval_v2/results/2026-11-25-c3-final-query-fix/RECHECK.md).

Query latency is slightly lower for C3 in this seven-case pair, but one
measurement per case does not establish a stable speedup. The C3 context
contains the complete reviewed span in 2/7 cases, versus 1/7 for B5. The
`f43926d9` case demonstrates that retrieving a complete reviewed span still
does not guarantee the reader will answer correctly.
