# Ordered-relations reader regression comparison — 2026-11-27

**Candidate gate: FAILED.** This is one exposed development comparison on ten frozen synthetic cases, not an unseen qualification or a reason to change the default reader. Both arms choose `North` for the conflicting duplicate-anchor case whose answer is `UNKNOWN`.

[Registration](registration.json), [acceptance criteria](acceptance-registration.json), [archived runner](runner.source.py), [outputs](outputs.json) and [report](report.json) are retained. The comparison used the same local native Qwen3.8-27B MLX model, temperature 0, 256 completion tokens, 60 seconds per call, zero retries and one case-major capture: **10 cases × 2 arms = 20 calls**. The recorded capture took 100.744 seconds. Model/runtime identity is pinned in [execution identity](execution-identity.json); a model-cache directory name does not require LM Studio as the serving application.

| Metric | Structured baseline | Ordered-relations candidate |
| --- | ---: | ---: |
| Correct value | 7/10 | 9/10 |
| Completed correct value | 7/10 | 9/10 |
| Valid answer format | 10/10 | 10/10 |
| Literal source quote | 10/10 | 7/10 |
| Completed correct value **and** literal quote | 7/10 | 7/10 |
| Correct false-premise answer | 0/2 | 2/2 |
| Reader errors / unobserved calls | 0 / 0 | 0 / 0 |

`completed_correct` in the frozen report means correct value with valid, nontruncated completion; it does **not** include the separate quote check. The conjunction above is counted from the existing per-case fields and is not a replacement score. Literal presence alone does not establish correct source-ID binding or sufficient supporting evidence.

The candidate correctly answers the adjacent and terminal negative cases with `None`, but quotes derived `->`, `EMPTY` or `NONE` navigation rather than literal source text. Its duplicate-anchor answer also quotes a derived relation. These three quote failures remain failures. The baseline instead answers `UNKNOWN` on both source-supported negative cases and fails the duplicate-anchor case too.

The preregistered candidate gate requires 10/10 correct values, 10/10 valid formats, 2/2 false-premise answers, no errors/unobserved calls, no loss on baseline-correct cases, and preservation of unknown controls. The 9/10 value result violates it even though quotes were registered as a separate diagnostic. No retry, answer repair, gold change or best-output selection was used. All raw attempts and inference events remain alongside the report.
