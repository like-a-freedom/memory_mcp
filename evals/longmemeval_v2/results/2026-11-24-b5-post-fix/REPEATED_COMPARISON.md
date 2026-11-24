# LongMemEval-V2: repeated seven-case B5 run

- **Run date:** 2026-11-24 UTC (completed 2026-11-24T21:41:38Z)
- **Host:** native Apple Silicon (`aarch64-apple-darwin`)
- **Provider path:** direct local MLX servers on loopback; no LM Studio process, remote provider, or API credential
**Question:** did B5's retrieval-only change consistently preserve context after the upstream token cap, and did that produce better supported answers without an unacceptable latency cost?

## Frozen inputs and comparability

Both candidate runs use the same seven frozen case IDs, pinned upstream/dataset, 100-item ordered haystack, exact persisted C4 snapshot, post-B5 retrieval binary, reader and judge model files, local server configuration, and score command. The baseline pair was run against that same persisted snapshot and cohort before B5. The retrieval binary SHA-256 is recorded in each `run_args.json`; it is identical across the candidate runs and matches the accepted B5 replay binary. No ingestion or extraction change is mixed into this comparison.

The official LongMemEval scorer emitted metrics for all seven selected cases in both candidate runs. `score-coverage.json` reports 7/7 native-scored and covered cases, recomputed official aggregate equality, and no scorer-recorded failures in each run.

## Results

| Measure | Baseline initial | Baseline repeat | B5 run 01 | B5 run 02 |
|---|---:|---:|---:|---:|
| Official full-set score | 0/7 (0%) | 0/7 (0%) | 1/7 (14.3%) | 1/7 (14.3%) |
| Non-abstention score | 0/4 | 0/4 | 1/4 (25%) | 1/4 (25%) |
| Abstention score | 0/3 | 0/3 | 0/3 | 0/3 |
| Mean final memory context | 0 tokens | 0 tokens | 3,237 tokens | 3,237 tokens |
| Contexts empty after cap | 7/7 | 7/7 | 0/7 | 0/7 |
| Query latency mean | 637.0 s | 720.1 s | 733.3 s | 670.8 s |
| Query latency p50 | 441.5 s | 458.6 s | 522.7 s | 464.5 s |
| Query latency p95 / max | 1,030.4 s | 1,214.0 s | 1,161.7 s | 1,066.9 s |
| Sum of query latencies | 4,458.9 s | 5,040.7 s | 5,132.8 s | 4,695.9 s |
| Reader / judge failures recorded by scorer | 0 / 0 | 0 / 0 | 0 / 0 | 0 / 0 |

The mean query latency changed by **+15.1%** in B5 run 01 versus the baseline initial run, and by **−6.8%** in run 02 versus the baseline repeat. These small, oppositely directed samples do not establish a latency improvement or regression. The candidate's two-run mean over 14 case reads was 702.0 seconds; the two baseline runs' combined mean over their 14 reads was 678.5 seconds (+3.5%). With seven selected cases and substantial run-to-run spread, treat that difference as descriptive only.

### Repeated evidence and answer stability

| Case | Context tokens (both runs) | Full reviewed source span present | Score (both runs) | Parsed answer identical |
|---|---:|---|---:|---|
| `0c6b0c60` | 2,833 | No | 1 | No |
| `6d975a08` | 3,508 | No | 0 | No |
| `89a20f48` | 3,424 | No | 0 | Yes |
| `922a0c56` | 3,532 | No | 0 | Yes |
| `d6da3938` | 2,828 | No | 0 | Yes |
| `f43926d9` | 3,598 | Yes | 0 | Yes |
| `f61a096f` | 2,939 | No | 0 | Yes |

The source-span check is deterministic: for each case, it takes the frozen line range and SHA-256-verified accessibility-tree span from `small-web-text-cohort.json` and checks whether the complete span is present in the returned context after decoding the stored text escapes. This is a strict, narrow evidence-presence measure, not a semantic relevance score. It found the complete reviewed span in 1/7 cases. In case `0c6b0c60`, the answer passed the official exact-match scorer, but the reader's answer text itself says the full dropdown options are not explicit in the provided context and invokes standard Postmill behavior. That pass therefore is not evidence-grounded success.

Context hashes and context token counts matched exactly across the two B5 runs for all seven case IDs. Official per-case scores also matched 7/7. Parsed answer text matched for 5/7; the two differing answers did not change their case scores. Thus retrieval is reproducible here, but generated response wording is not fully deterministic even with temperature set to zero.

## Interpretation and decision

1. **B5 fixes the measured context-loss defect:** every case now retains 2,828–3,598 context tokens; the baseline left zero tokens after the upstream cap in all seven cases. This result repeats exactly.
2. **The sample shows a small official-score increase, not proven answer-quality improvement:** 1/7 in both B5 runs versus 0/7 in both baseline runs. The only scored success is not supported by the complete reviewed source span. The result is too small and too weakly grounded to claim improved evidence-based answering.
3. **Retrieval relevance is now the next measured gap:** 6/7 cases did not include their complete frozen evidence span. Preserve these as explicit failures for the agent-control/C1 work; do not change memory behavior based on the raw 1/7 score alone.
4. **Latency is unresolved:** candidate runs move in opposite directions relative to the corresponding baseline runs. No latency benefit or material regression is established by this seven-case sample.

## Reproduction artifacts

- `run-01/` and `run-02/` contain `run_args.json`, `prompt_build_summary.json`, `prompt_rows.jsonl`, `per_question.jsonl`, `aggregated_metrics.json`, and `score-coverage.json`.
- Baseline aggregates and replay checks are in `../2026-11-24-small-web-text-baseline/`.
- The pinned text evidence cases and verified source spans are defined in `../../small-web-text-cohort.json` and staged from the pinned dataset by `../../text_cohort.py`.
- The temporary model/data environment and model weights were kept outside the repository; only benchmark receipts are checked in.
