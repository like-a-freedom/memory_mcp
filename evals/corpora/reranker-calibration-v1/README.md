# Independent development relevance calibration

Source: LongMemEval cleaned small dataset, immutable revision `98d7416c24c778c2fee6e6f3006e7a073259d48f`, CC-BY-4.0. All 500 raw cases were downloaded; source SHA-256 `d6f21ea9d60a0d56f34a05b609c79c88a451d2ae03597821ea3d5a9678c3a442`, 277,383,467 bytes. This is a development subset, not an official benchmark score.

Selection precedes model inference: first 12 published cases; one complete first answer user turn and the top three lexical non-answer user turns. A separate reviewer adjudicated all 48 proposals without seeing code or model scores. Three incomplete positive turns were excluded with source-only reasons; 45 binary pairs remain (9 positive, 36 negative). Every exact raw turn was subsequently matched to the pinned upstream data (`raw-lineage-verification.json`). No gold answer text is ingested into documents.

`source-review.json` records exact labels/support spans and limitations. Documents use the same source/reference-date prefix as production relevance inference. `token-visibility.json` covers the original XLM-R input; `qwen-token-visibility.json` verifies actual Qwen token IDs, all nine positive spans, and a maximum of 932/2048 tokens. Source dates are recording dates and do not imply event dates.

Run actual calibration with the E5/Qwen directories and other native environment variables configured as in the retrieval subset README:

```sh
cargo run -p eval-harness --bin memory-eval --locked -- calibrate-reranking --pairs evals/corpora/reranker-calibration-v1/pairs.json --output work/evals/calibration.json
```

The command fits the maximum development F1 operating point, then precision, recall and the higher floor for ties. It reports exact input/model identities and all raw scores. For this frozen source-reviewed sample, mMARCO yielded F1 0.7778; Qwen yielded TP9/FP0/FN0/TN36 and floor `-2.9250669479370117`. The floor was fixed before Qwen retrieval acceptance predictions. It is a selected raw-logit operating point, not a relevance probability or a universal guarantee.

Negatives often concern obviously different topics; repeated windows within a case are correlated. Do not split these source cases into development and held-out test, call this a hard-negative benchmark, or tune the operating point against the 35 retrieval acceptance results.
