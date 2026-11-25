# LongMemEval-V2 B5 post-fix recheck

**Completed:** 2026-11-25 03:19 UTC  
**Host:** native Apple Silicon (`aarch64-apple-darwin`)  
**Inference:** standalone local MLX-LM servers on loopback; no LM Studio runtime, remote provider, or API credential.

## Decision

The full frozen seven-case run completed and emitted official metrics for all
7/7 cases, with no reader, judge, or scorer failures. B5's context-preservation
fix is still present: the final contexts are 2,828–3,598 tokens, none was
truncated, and the new run's seven context payloads match the preceding B5
run-02 exactly. This confirms repeatability of the measured retrieval output
for this cohort.

This run does **not** show a demonstrated improvement in evidence-grounded
answer quality. The official exact-match result is 1/7 (14.3%), unchanged in
all three valid B5 captures and up from 0/7 in the two pre-B5 baseline runs.
The only scored answer is unsupported by its reviewed source span. Conversely,
the one case whose full reviewed source span reached the reader still received
an incorrect final answer. The small official-score increase therefore cannot
be treated as a memory-quality gain.

## Case results

“Reviewed span present” means the complete frozen accessibility-tree line
range, after verifying its SHA-256 against
[`small-web-text-cohort.json`](../../small-web-text-cohort.json), occurs in the
reader's memory context. It is a strict source-presence test, not a semantic
relevance judgment.

| Case | Official exact match | Complete reviewed span present | Query latency | Context tokens |
|---|---:|---:|---:|---:|
| `0c6b0c60` | Pass* | No | 404.1 s | 2,833 |
| `6d975a08` | Fail | No | 414.9 s | 3,508 |
| `89a20f48` | Fail | No | 437.7 s | 3,424 |
| `922a0c56` | Fail | No | 947.9 s | 3,532 |
| `d6da3938` | Fail | No | 1,011.5 s | 2,828 |
| `f43926d9` | Fail | Yes | 406.9 s | 3,598 |
| `f61a096f` | Fail | No | 789.9 s | 2,939 |

\* Case `0c6b0c60` received an official exact-match pass, but its response
explicitly inferred the missing dropdown options from standard Postmill
behavior. The complete frozen options list was absent from the context. Count
it as an official scorer pass, not a supported memory answer.

The evidence and answer failures point to two separate boundaries. In 6/7
cases, retrieval did not place the complete reviewed source span in the
context. In `f43926d9`, retrieval did place it there, but the reader failed to
produce the accepted answer. Improving retrieval alone will not address that
reader-use failure.

## Accuracy, repeatability, and latency

| Measure | Pre-B5 initial | Pre-B5 repeat | B5 run 01 | B5 run 02 | B5 recheck |
|---|---:|---:|---:|---:|---:|
| Official exact match | 0/7 | 0/7 | 1/7 | 1/7 | 1/7 |
| Cases retaining nonempty context after cap | 0/7 | 0/7 | 7/7 | 7/7 | 7/7 |
| Cases containing full reviewed source span | n/a | n/a | 1/7 | 1/7 | 1/7 |
| Mean memory-query latency | 637.0 s | 720.1 s | 733.3 s | 670.8 s | 630.4 s |
| Median memory-query latency | 441.5 s | 458.6 s | 522.7 s | 464.5 s | 437.7 s |
| p95 / max memory-query latency | 1,030.4 s | 1,214.0 s | 1,161.7 s | 1,066.9 s | 1,011.5 s |

The two baseline run means average 678.5 seconds; the three valid B5 means
average 678.2 seconds. This seven-case sample shows no material average
latency change. The latest run took 1,838.3 seconds wall time; its memory-query
durations sum to 4,412.8 seconds because three prompt workers read in parallel.
Latency varies substantially across cases, and the p95 is based on only seven
observations.

The B5 context payloads and official per-case scores match the preceding
run-02 for all seven cases. Parsed answer text matches in six of seven cases
between those two captures; earlier B5 captures also showed wording variation.
The retrieval result is repeatable here, while generation text is not fully
deterministic.

## Input and execution controls

- The seven question IDs and their ordered 100-trajectory haystack are the
  frozen Small-Web text cohort. The staged question, haystack, trajectory, and
  selection file hashes match the earlier B5 staged cohort exactly.
- The C4 shared memory snapshot was reused; `build-coverage.json` records the
  same seven selected questions and 100 ordered trajectories. No ingestion or
  extraction change is mixed into this retrieval-only recheck.
- The release binary SHA-256 is
  `9c9b24b757a948eae3299313dd1a0a14c6cd4ddd62f033580bc27a7edf06213d`, the
  same binary recorded by the accepted B5 context replay. There are no
  `crates/memory-mcp/src` changes after that B5 replay point in this checkout.
- The pinned upstream commit, dataset revision, model profile, and model file
  hashes are recorded in
  [`small-web-native-profile.json`](../../small-web-native-profile.json).
  The staged processor and reader/judge model files were verified against
  that profile before scoring.
- Run arguments, per-question outputs, prompt rows, score coverage, and
  aggregates are retained in [`run-02/`](run-02/). Run 01 is retained as an
  invalid, unscored attempt: it used the wrong default Hugging Face cache and
  did not reach reader or judge inference. Its cause was fixed before run 02
  by adding processor preflight and an explicit offline `HF_HOME`.

## Follow-up

Keep the B5 retrieval fix: it resolves the measured loss of every context at
the upstream token cap, and this result repeats that behavior. Do not claim
LongMemEval answer-quality or latency improvement from this cohort. The next
diagnostic should distinguish retrieval relevance from agent use of retrieved
evidence. The positive ordinary-agent control and fresh C1 read-completeness
check are tracked separately in the implementation plan; no confirmatory C2
study is eligible until a workflow passes its frozen task gates.
