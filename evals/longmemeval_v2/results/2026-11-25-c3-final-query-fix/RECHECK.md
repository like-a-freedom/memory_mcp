# LongMemEval-V2 post-C3 recheck

**Date:** 2026-11-25
**Host:** Apple Silicon (`aarch64-apple-darwin`)
**Reader/judge runtime:** standalone MLX-LM 0.32.2 / MLX 0.32.4, loopback only. The LM Studio application, server and CLI were not used.

## Paired seven-case result

The same seven frozen questions, saved memory snapshot, reader/judge model
files, decoding settings and concise output instruction were applied to B5 and
post-C3 contexts. Reader and judge outputs completed for 7/7 cases in both
arms. Exact scoring used the pinned LongMemEval-V2 scoring predicates and
abstention judge prompt; the HTTP adapter sent requests only to local MLX-LM.
Because this paired diagnostic adds a concise-answer instruction to the system
prompt, its scores are **not an official full LongMemEval-V2 result** and must
not be compared directly with leaderboard scores.

| Measure | B5 | Post-C3 repeat |
|---|---:|---:|
| Pinned scorer, 7 frozen cases | 1/7 | 3/7 |
| Strict phrase-set accuracy, list-valued cases only | 0/4 | 2/4 |
| Complete reviewed source span in context | 1/7 | 2/7 |
| Mean memory-query latency | 630.4 s | 590.5 s |
| Median memory-query latency | 437.7 s | 429.7 s |
| Maximum memory-query latency | 1,011.5 s | 989.6 s |
| Mean reader generation time in concise protocol | 13.2 s | 9.8 s |

The query means are 6.3% lower for post-C3, while the median is 1.8% lower.
These are descriptive differences from seven questions and one memory read
per arm; they do not establish a stable speedup. The post-C3 repeated context
payloads match the first C3 run for all 7/7 questions. B5 and post-C3 context
payloads match for 3/7 and differ for 4/7.

The exact-match gains are `6d975a08` and `d6da3938`. The deterministic strict
list metric confirms those two answers. The upstream phrase-subset scorer also
counts `0c6b0c60` as correct, but its response adds the unsupported option
“Past hour”; the strict metric rejects it. This is why the report preserves
both scores instead of treating subset inclusion as exact list accuracy.

None of the three false-premise cases passed the abstention judge in either
arm. The reader gave concrete answers for `89a20f48` (“Past year”),
`922a0c56` (“Send Email”), and `f43926d9` (“Forums”). This is a persistent
memory-to-answer failure slice; retrieval changes alone did not resolve it.
For `d6da3938`, C3 returned “Reset Password” and passed exact phrase scoring;
its context contains the adjacent toolbar controls but not the complete
reviewed span. For `6d975a08`, the complete span and exact list are both
present. The `f43926d9` context also contains its complete reviewed span, but
the reader still gave a wrong answer. Retrieval evidence is therefore
necessary for these cases but does not guarantee correct use.

## Deterministic list metric

The pinned `norm_phrase_set_match` predicate verifies that every gold phrase
appears in the prediction; it permits additional phrases. The local
`exact_answer_quality` metric supplements it by requiring equality of the
normalized phrase sets and counting incomplete selected rows as failures.
Five unit tests cover order/punctuation normalization, extra and missing
phrases, empty values, inapplicable judge rows, and incomplete responses.

```sh
python3 -m unittest evals.longmemeval_v2.test_exact_answer_quality
python3 -m evals.longmemeval_v2.exact_answer_quality \
  --per-question evals/longmemeval_v2/results/2026-11-25-c3-final-query-fix/run-02/scoring-replay-concise-v1/per_question.jsonl \
  --output evals/longmemeval_v2/results/2026-11-25-c3-final-query-fix/run-02/scoring-replay-concise-v1/strict-phrase-set-accuracy.json
```

## Provenance and run validity

The seven IDs, ordered 100-trajectory haystack, input hashes and persisted C4
snapshot are unchanged. C3 changed retrieval/query construction only; the
snapshot was not rebuilt. The current C3 memory-query run produced all seven
contexts and reader outputs, but its original scoring attempt had two empty
judge responses. That attempt remains in `run-02/scoring-replay/` as a failed
diagnostic and is not used for the paired result above.

The repeat imported `evaluation.harness` from
`target/c4-native/upstream`, a staged source copy without its own `.git`
directory. A `git rev-parse` in that directory misleadingly resolved to the
memory-mcp checkout. Provenance was therefore rechecked by comparing all 53
staged source files against the clean checkout at pinned commit
`ef67f10aacd9080c75aeb2dd527a0af25dc26f1b`; every compared file matched.
The extra 29 files in the pinned checkout are generated `build/lib` and
packaging metadata and were not imported by this runner.

Reader and judge traffic used only `127.0.0.1:18767` and `127.0.0.1:18766`.
Hugging Face was offline. Model files were locally cached and hash-checked
against the pinned profile before the memory-query run. No Codex or remote
inference service was involved.

Artifacts:

- [Paired comparison JSON](concise-b5-vs-c3-comparison.json)
- [C3 repeated execution manifest](execution-manifest.json)
- [B5 concise replay](../2026-11-25-b5-recheck/run-02/scoring-replay-concise-v1/replay.json)
- [C3 concise replay](run-02/scoring-replay-concise-v1/replay.json)
- [C3 strict list score](run-02/scoring-replay-concise-v1/strict-phrase-set-accuracy.json)
- [C3 reviewed-span coverage](run-02/reviewed-span-coverage.json)

The cohort remains a seven-case audited slice, not the full LongMemEval-V2
benchmark. The source-span metric requires a complete reviewed span and can
miss smaller sufficient evidence; a deterministic answer-bearing control
oracle remains future evaluation work.
