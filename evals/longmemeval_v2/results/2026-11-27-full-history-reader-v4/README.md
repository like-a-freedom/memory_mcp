# Reader comparison on the V4 full-history capture — 2026-11-27

**Full acceptance: FAILED for both arms.** The seven-case denominator includes the upstream retrieval failure. This directory evaluates reader protocols on fixed V4 delivered contexts; it does not establish improved retrieval, general memory utility, or a qualified default reader.

[Registration](registration.json), [acceptance criteria](acceptance-registration.json), [archived runner](runner.source.py), [outputs](outputs.json) and [report](report.json) retain one paired capture: **7 questions × 2 arms = 14 calls**. The same local native Qwen3.8-27B MLX model used temperature 0, 256 completion tokens, 60 seconds per call and zero retries, in case-major order. Capture time was 241.802 seconds; [execution identity](execution-identity.json) pins the runtime/model. No outputs were retried or selected for success.

| Metric, denominator 7 scheduled questions per arm | Structured baseline | Ordered-relations candidate |
| --- | ---: | ---: |
| Correct value, including truncated answers | 4/7 | 5/7 |
| Completed correct value | 3/7 | 4/7 |
| Valid answer format | 7/7 | 7/7 |
| Literal source quote | 6/7 | 4/7 |
| Completed correct value **and** literal quote | 3/7 | 2/7 |
| Correct false-premise answer | 0/3 | 1/3 |
| Reader errors / unobserved calls | 0 / 0 | 0 / 0 |
| Truncated completions | 1/7 | 1/7 |

The report's `completed_correct` excludes truncation but does **not** require an exact quote. The conjunction above preserves that distinction. For `922a0c56`, quote applicability is null because no context was delivered; a null check does not contribute a quote success. On the six delivered contexts, completed-value counts are 3/6 and 4/6 and literal-quote counts are 6/6 and 4/6. These conditional diagnostics never replace the seven-case end-to-end denominator. Literal quotation is also narrower than source attribution and evidential sufficiency.

Failures explain why the value increase is insufficient:

- `922a0c56`: V4 retrieval delivered no context after the external process limit. Both readers answer `UNKNOWN`; this is conservative reader behavior under missing information, but remains an end-to-end failure. The missing context cannot be classified as a reader reasoning failure or filled from another case.
- `f61a096f`: both arms emit the correct value `nine` and a literal quote, but hit the 256-token limit. Both completed-value checks fail.
- `89a20f48`: both values are wrong; a literal quote does not rescue the answer.
- `d6da3938` and `f43926d9`: the candidate emits correct values but quotes derived arrow/`EMPTY` relations, so exact quotation fails. Only `f43926d9` changes a false-premise value to correct `None`.

The independent [V4 source-support review](../2026-11-27-full-history-v4/source-support-review.json) records strict registered-span support on 4/7 and independently sufficient delivered support on 6/7, distinguishing alternate support from registered gold. No gold was promoted or borrowed across questions. Full acceptance requires all seven sufficiently supported deliveries plus correct completed values, formats and quotes, with no retrieval/reader faults. Neither arm satisfies it; the candidate is not qualified by this result. Historical failures, raw outputs, attempts and events remain unchanged.
