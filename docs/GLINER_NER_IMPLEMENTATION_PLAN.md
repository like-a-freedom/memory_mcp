# GLiNER NER Implementation Plan

**Status:** 🟡 Implemented with verified follow-up fixes pending  
**Date:** March 30, 2026  
**Predecessor:** `docs/GLINER_NER_SPEC.md` (v1.5)

---

## Overview

Implement GLiNER-based NER extraction using candle inference, following the existing patterns for `LocalCandleEmbeddingProvider`. The implementation adds a new `NerProviderKind::LocalGliner` variant while preserving backward compatibility.

---

## Verification Update (2026-04-02)

An independent review on 2026-04-02 was checked against the current repository state.

### Verified as already covered

- Real-model smoke coverage now exists in `tests/gliner_integration.rs`.
- Eval helpers in `tests/common/mod.rs` already wire the local GLiNER fixture into end-to-end retrieval runs.
- IoU-based NMS is already implemented in `src/service/gliner_entity_extractor.rs`.

### Still open after verification

- `extract_candidates()` still runs `extract_inner()` directly on the tokio worker thread instead of yielding through a blocking boundary.
- Sliding-window overlap is still effectively 1 token, which is too small for entities spanning a window boundary.
- The remaining integration-test gap is narrower than “no integration tests”: we still need a long-text boundary regression, not another generic smoke test.

---

## Completed Work

### PR 1: Configuration + Model Loader ✅

- `NerProviderKind` enum with variants: `Regex`, `Anno`, `LocalGliner`
- `NerConfig` struct with all required fields
- `model_loader.rs` generalized with `MODEL_REQUIRED_FILES` constant
- Factory function with stub error

### PR 2: Core Implementation ✅

- `GlinerEntityExtractor` struct with DebertaV2Model
- Tokenizer loading with dynamic ENT/BOS/EOS resolution
- Span scoring head with correct bilinear architecture:
  - `span_rep_layer`: `2*hidden_size → hidden_size` (linear_no_bias)
  - `prompt_rep_layer`: `hidden_size → num_labels` (linear_no_bias)
  - Scoring via `broadcast_mul` + `sum(1)`
- Per-token scoring (simplified, see "Remaining Work" below)

### Review Fixes Applied ✅

| # | Issue | Fix |
|---|---|---|
| 1 | `linear` vs `linear_no_bias` | Changed to `linear_no_bias` |
| 2 | Span scoring architecture | Bilinear scoring implemented |
| 3 | Hardcoded BOS/EOS | Dynamic resolution from tokenizer |
| 4 | Unused `batch_size` | Removed from struct |
| 5 | `zeros_like().mean(0)` | Fixed to `Tensor::zeros()` |
| 6 | `is_alphabetic()` | Changed to `is_alphanumeric() \|\| "-+.#".contains(c)` |

---

## Rollout Plan

| PR | Status | Description |
|----|--------|-------------|
| PR 1 | ✅ Done | Configuration + model loader + factory stub |
| PR 2 | ✅ Done | Core GLiNER implementation with bilinear scoring |
| PR 3 | ✅ Done | Full span scoring + offset mapping + NMS + sliding window |
| PR 4 | 🟡 Partial | Real-model smoke tests exist in `tests/gliner_integration.rs`; boundary-window regression coverage is still missing |

---

## Open Questions

1. **max_span_width**: Get from model config or use default 12?
2. **Sliding window overlap**: How many tokens to overlap between windows?
3. **Runtime scheduling**: should local GLiNER inference stay inline, or must it always yield via a blocking boundary like LocalCandle?

---

## References

- GLiNER paper: https://arxiv.org/abs/2408.47221
- HF Hub: https://huggingface.co/urchade/gliner_multi-v2.1
- GLiNER Python: https://github.com/urchade/GLiNER
