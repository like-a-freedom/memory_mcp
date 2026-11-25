# LongMemEval-V2 post-C3 query-fix recheck

**Completed:** 2026-11-25 06:14 UTC  
**Host:** native Apple Silicon (`aarch64-apple-darwin`)  
**Reader and judge:** standalone local MLX-LM 0.32.2 / MLX 0.32.4 over loopback; no LM Studio runtime, remote inference, or API credential.

## Result

The run scored all 7/7 frozen text cases. Reader, judge and scorer failures were
zero, aggregate metrics recomputed from per-question scores, and none of the
seven memory contexts was truncated. Official exact match was 3/7 (42.9%),
versus 1/7 in the immediately preceding B5 run on the same cohort and saved
snapshot. Non-abstention accuracy was 3/4; abstention accuracy was 0/3 because
the reader answered rather than returning unknown on the three closed-list
false-premise questions.

This is one post-C3 model capture on seven cases. It is a positive result for
this sample, not a confirmed population-level or repeatable quality gain. The
per-case context and score are model-run outcomes; only the reviewed-source
span check is deterministic. The full selected-cohort denominator and failure
coverage are retained in [`score-coverage.json`](run-01/score-coverage.json).

## Evidence, answers and cost

The exact reviewed spans were regenerated from the frozen line ranges and
verified against their SHA-256 values. The provider-free
[`reviewed-span-coverage.json`](run-01/reviewed-span-coverage.json) scorer
found 2/7 complete spans in the reader contexts. This is a strict whole-span
check, not a complete measure of semantic sufficiency.

| Case | Exact match | Full reviewed span | Context tokens | Query time |
|---|---:|---:|---:|---:|
| `0c6b0c60` | Pass, unsupported | No | 3,463 | 292.3 s |
| `6d975a08` | Pass | Yes | 3,564 | 434.0 s |
| `89a20f48` | Fail | No | 3,543 | 300.8 s |
| `922a0c56` | Fail | No | 3,532 | 950.0 s |
| `d6da3938` | Pass | No | 1,473 | 1,008.2 s |
| `f43926d9` | Fail | Yes | 3,598 | 408.9 s |
| `f61a096f` | Fail | No | 2,939 | 797.8 s |

The `0c6b0c60` answer lists options inferred from typical Postmill behavior;
the options are absent from the returned context, so this is an official
scorer pass but not a grounded memory success. In `d6da3938`, the context
contains consecutive toolbar buttons `Create Order` and `Reset Password`,
which support the exact answer, but the complete 20-line reviewed span is not
present. That case exposes a limitation of the full-span metric: whole-span
absence does not necessarily mean the answer-bearing evidence is absent.
Conversely, `f43926d9` contains the full reviewed span but the reader still
answers incorrectly, so retrieval alone does not guarantee correct use.

| Metric | B5 run 02 | Post-C3 run 01 |
|---|---:|---:|
| Official exact match | 1/7 | 3/7 |
| Complete reviewed span | 1/7 | 2/7 |
| Mean query latency | 630.4 s | 598.8 s |
| Median query latency | 437.7 s | 434.0 s |
| p95 / max query latency | 1,011.5 s | 1,008.2 s |
| Wall time | 1,838.3 s | 1,854.0 s |

The two new exact-match cases are `6d975a08` and `d6da3938`; the pre-existing
`0c6b0c60` scorer pass remains unsupported. Four contexts changed and three
were byte-identical to B5 run 02. The mean and median query times moved down
slightly, while wall time moved up. With seven observations and one post-C3
capture, do not claim a meaningful or stable latency improvement.

## Reproduction and provenance

The [execution manifest](execution-manifest.json) records the exact seven
question IDs, ordered 100-trajectory cohort, hashes, saved memory snapshot,
binary and local model profile. The same B5 snapshot and input hashes were
reused. C3 changed query construction only; it did not re-ingest the dataset.
The C3 release product binary is native arm64. The reader and judge used their
previously hash-verified pinned model files and the same profile/settings as
B5; the model files were reused at the same paths and were not re-hashed during
this run. Processor, reader and judge versions and model-file pins are listed
in [`small-web-native-profile.json`](../../small-web-native-profile.json).

`run-01/run_args.json` records the invocation arguments and local loopback
endpoints. `run-01/prompt_rows.jsonl`, `per_question.jsonl`,
`aggregated_metrics.json`, `score-coverage.json` and
`reviewed-span-coverage.json` retain the run evidence. The temporary cohort
inputs are not copied into the repository; their hashes and staging path are
in the execution manifest. This is the reviewed seven-case subset, not a full
LongMemEval-V2 benchmark run.

The model-free source-span scorer can be replayed with the command documented
in [`evals/longmemeval_v2/README.md`](../../README.md). The complete adapter
unit suite passed 32 tests with five opt-in skips:
`python3 -m unittest discover -s evals/longmemeval_v2`.

## Follow-up

Keep the C3 retrieval fix for the cases it repairs, while treating this run as
confirmatory evidence still pending a repeat. The next evaluation improvement
is a source-sufficiency oracle that measures whether answer-bearing controls
and values are present without requiring every unrelated line in a reviewed
span. Preserve abstention questions as a separate failure slice. No speedup,
full-benchmark result, or general agent-utility claim is established here.
