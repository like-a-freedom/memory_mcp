# Run 01 report — Qwen speed v2-prompt qualification NO-GO

**Plan:** `source-authority-local-qwen-speed-2026-11-24-v2-prompt-qualification`  
**Capture:** `78931-1795471053980137000-0`  
**Capture window:** 2026-11-23 21:57:33.980139–22:04:51.245020 UTC (7m 17.265s)  
**Product:** same binary in both arms, SHA-256 `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`

## Outcome

The offline `compare-agent-managed` scorer accepted all four paired families, and a second offline comparison produced a byte-identical result. Task-oracle success was **2/4 incumbent and 3/4 candidate**. The candidate meets the 3/4 arm floor, but the incumbent does not. Independent fixed-query probes passed **2/4 incumbent and 3/4 candidate**, below the required 4/4 in both arms. Candidate source-file tasks passed **2/2**; both operator imports and extractions succeeded. Hard violations and budget-exceeded trials were both zero.

The content-free receipts classified incumbent final outcomes as two exact answers, one answer without an accepted literal value, and one model failure; candidate outcomes were three exact answers and one model failure. No raw answer or retrieved memory text was retained. The paired task-success difference is +0.25 with a family-cluster 95% interval of [-0.50, 1.00], which is inconclusive at four families.

## Decision and limits

**NO-GO for this exact model/runner/sampler configuration.** The frozen per-arm task threshold and independent-probe threshold both fail. The v2 prompt was exercised on fresh families, but this cohort had no v1 prompt control; differences from earlier cohorts do not isolate the prompt change. The same product binary in both arms means this screen does not estimate a memory treatment effect. The 7m17s capture is not a controlled speed comparison with other models or cohorts.

The result shows a limited workflow signal: the candidate path passed both operator-import tasks, while the incumbent arm did not meet the task floor. It does not justify a memory change or prove general agent benefit. Keep the families immutable; do not tune or repeat this cohort. A powered study requires a separately registered design and fresh families after a model configuration clears the qualification gate.

## Reproducibility

The frozen inputs verified before capture with `freeze-manifest.sha256`. The original and replayed offline comparator artifacts are byte-identical (SHA-256 `a45159f3c09e502bb80f72216c0dbe751ba275656694358a8f561a7a49fab045`). `SHA256SUMS` records output hashes. Deterministic evaluation and offline comparison do not require LM Studio or an API key; this capture used a local numeric-loopback provider with synthetic facts.
