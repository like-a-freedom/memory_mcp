# GLiNER NER Specification

**Version:** 1.5  
**Date:** March 30, 2026  
**Status:** ✅ Fully Implemented

---

## 1. Motivation

Current NER implementation uses heuristic approaches (`RegexEntityExtractor`, `AnnoEntityExtractor`) that provide limited entity type coverage. Adding GLiNER (Graph-based Language Model for Named Entity Recognition) enables:

- Zero-shot entity extraction across arbitrary label sets
- Contextual understanding beyond pattern matching
- Support for multilingual entities
- Consistent with existing candle-based embedding infrastructure

---

## 2. Design Goals

1. **Drop-in replacement** for existing NER providers
2. **Same download/cache pattern** as `LocalCandleEmbeddingProvider`
3. **Configurable labels** via environment variables
4. **Threshold control** for entity confidence filtering
5. **Zero external services** — all inference runs locally via candle

---

## 3. Architecture

```
┌─────────────────────────────────────────────┐
│           EntityExtractor trait             │
├─────────────────────────────────────────────┤
│ RegexEntityExtractor  │ AnnoExtractor      │
│ + extract_candidates_batch (default impl)   │
└─────────────────────────────────────────────┘
                              │
                              ▼ (new)
┌─────────────────────────────────────────────┐
│        GlinerEntityExtractor               │
│  - DebertaV2Model (candle-transformers)     │
│  - tokenizer (tokenizers)                   │
│  - span scoring head (bilinear)             │
└─────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────┐
│        model_loader.rs                       │
│  - hf-hub (URL resolution only)             │
│  - reqwest (actual download with retry)     │
│  - shared with LocalCandleEmbeddingProvider │
└─────────────────────────────────────────────┘
```

> **Critical**: GLiNER (`urchade/gliner_multi-v2.1`) uses **DeBERTa-v3**, not BERT.
> Must verify `candle-transformers` has `DebertaV2Model` before Phase 3.
> Alternative: ONNX export via `ort` crate if DeBERTa support unavailable.

> **DRY**: Model loading is unified via `model_loader.rs`. Both embeddings and NER
> use the same `hf-hub` + `reqwest` pattern (hf-hub for URL resolution, reqwest for
> actual download with our TLS + redirect config).

---

## 4. Configuration

### 4.1 NerProviderKind

```rust
pub enum NerProviderKind {
    Regex,        // existing
    Anno,         // existing (default)
    LocalGliner,  // NEW
}
```

### 4.2 NerConfig

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `provider` | `NerProviderKind` | `Anno` | NER backend |
| `model` | `Option<String>` | `urchade/gliner_multi-v2.1` | HuggingFace repo |
| `model_dir` | `Option<String>` | auto | Override cache dir |
| `labels` | `Vec<String>` | person, company, location, product, event, technology | Entity types |
| `threshold` | `f64` | 0.5 | Confidence cutoff |
| `batch_size` | `usize` | 4 | Texts per forward pass (CPU optimization) |

### 4.3 Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `NER_PROVIDER` | `anno` | `regex`, `anno`, `local-gliner` |
| `NER_MODEL` | `urchade/gliner_multi-v2.1` | HF repo ID |
| `NER_MODEL_DIR` | auto | Override cache path |
| `NER_LABELS` | `person,company,location,product,event,technology` | Comma-separated |
| `NER_THRESHOLD` | `0.5` | 0.0–1.0 |
| `NER_BATCH_SIZE` | `4` | Texts per inference pass (CPU) |

---

## 5. Model Requirements

### 5.1 Supported Models

Primary: `urchade/gliner_multi-v2.1` — multilingual DeBERTa-v3 based GLiNER model.

Alternative: Any GLiNER-compatible model from HuggingFace with:
- `tokenizer.json`
- `config.json`
- `model.safetensors`
- `labels.json` (optional, can be provided at runtime)

### 5.2 Required Files

```rust
pub const GLINER_REQUIRED_FILES: &[&str] = &[
    "tokenizer.json",
    "config.json", 
    "model.safetensors",
];
```

Note: `labels.json` is optional — labels can be passed at runtime via config.

---

## 6. Inference Pipeline

### 6.1 Tokenization

GLiNER uses a **token-level prompt** where each label is encoded as a separate prefix span with a special separator token. 

> **Verification Required**: The separator token varies by model version:
> - `gliner_multi-v2.1` uses `<<ENT>>` (double angle brackets), **not** `[ENT]`
> - Other models may use `@` or other tokens
>
> Verify from model's `tokenizer.json` before Phase 3:
> ```bash
> python -c "
> from tokenizers import Tokenizer
> t = Tokenizer.from_pretrained('urchade/gliner_multi-v2.1')
> print([k for k in t.get_vocab().keys() if 'ENT' in k or '<<' in k])
> "
> ```

Prompt format:
```
[CLS] label_1 <<ENT>> label_2 <<ENT>> ... [SEP] token_1 token_2 ... [SEP]
```

**NOT** a simple concatenated string like `"person, company </s> Alice..."`.

### 6.2 Forward Pass

1. Encode each label with `<ENT>` separator → label embeddings (use verified token from §6.1)
2. Encode text portion → token embeddings
3. Run DeBERTa-v3 forward → `hidden_states`
4. Apply **bilinear span scoring head** (required weights from model)
5. Compute start/end scores for each label × position pair

### 6.3 Required Model Weights

GLiNER span head weights must be loaded from `model.safetensors`. Key prefixes:
- `span_rep_layer.*` — span representation projection
- `prompt_rep_layer.*` — label/prompt representation

> **Critical**: There is **no fallback** for span scoring head. Weights are required.
> Must inspect actual weight keys before Phase 3 implementation.

### 6.4 Span Decoding

1. Compute start/end scores for each label × token position
2. Greedy span extraction (max score span per label)
3. **CRITICAL: Apply attention mask to exclude pad positions** — failure to mask pad positions causes false entities at end of short texts (this is the bug fixed in GLiNER v0.1.9)
4. Filter by threshold
5. Deduplicate by canonical name

---

## 7. Error Handling

| Error | Handling |
|-------|----------|
| Model download failure | Retry 3x, then error with clear message |
| Tokenization failure | Return empty candidates |
| Model load failure | Propagate as `MemoryError::Storage` |
| Inference failure | Log error, return empty candidates |

---

## 8. Performance Targets

- **First-run download**: ~200–400MB (model size)
- **Cache**: Persisted to `<data_dir>/models/ner/<sanitized_model_name>`
- **Inference latency**: 
  - Single text (512 tokens): <800ms on modern CPU (M2 Pro / 8-core x86)
  - Batch (4 × 128 tokens): <1s on modern CPU
- **Memory footprint**: ~1GB RAM for model + tokenizer
- **Batch size**: 4 texts per forward pass (configurable via `NER_BATCH_SIZE`)

> **Note**: DeBERTa-v3-base with disentangled attention is ~2x slower than BERT-base.
> Latency scales with max sequence length in batch. Batch of 4×512 tokens may exceed 1s.

---

## 9. Backward Compatibility

- Default `NER_PROVIDER=anno` preserves existing behavior
- Existing `RegexEntityExtractor` and `AnnoEntityExtractor` remain unchanged
- No breaking changes to `EntityExtractor` trait
- Batch method added with default impl (no changes to existing implementors)

## 10. EntityExtractor Trait Extension

Add batch processing support to enable efficient transformer inference:

```rust
#[async_trait]
pub trait EntityExtractor: Send + Sync {
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError>;

    // Default impl — sequential calls for Regex/Anno
    // Callers with String slices must convert: contents.iter().map(|s| s.as_str()).collect::<Vec<_>>()
    async fn extract_candidates_batch(&self, contents: &[&str]) -> Result<Vec<Vec<EntityCandidate>>, MemoryError> {
        let mut results = Vec::with_capacity(contents.len());
        for content in contents {
            results.push(self.extract_candidates(content).await?);
        }
        Ok(results)
    }
}
```

> **Note**: The trait uses `&[&str]` for zero-copy efficiency. Callers holding `String` must convert:
> ```rust
> let strings: Vec<String> = ...;
> let slices: Vec<&str> = strings.iter().map(|s| s.as_str()).collect();
> extractor.extract_candidates_batch(&slices).await
> ```

`GlinerEntityExtractor` overrides `extract_candidates_batch` with true batch inference:
- Pad all texts in batch to max length in batch (not global max_length)
- attention_mask = 0 for pad tokens
- Mask span scores for pad positions (critical — bug #63 was padding-related)

---

## 11. DRY: Unified Model Loading

Model loading is shared between embeddings and NER to avoid duplication:

### 11.1 Current Pattern (from `model_loader.rs`)

```rust
// hf-hub handles URL resolution (CDN routing, auth, revisions)
let api = hf_hub::api::tokio::ApiBuilder::new().build()?;
let repo = api.model(repo_id.to_string());
let url = repo.url(file_name);

// reqwest handles actual HTTP with our TLS + redirect config
let http = reqwest::Client::builder()
    .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
    .redirect(reqwest::redirect::Policy::limited(10))
    .build()?;
```

### 11.2 Required Refactor

Generalize `ensure_model_cached()` to accept `required_files` parameter:

```rust
pub async fn ensure_model_cached(
    repo_id: &str,
    cache_dir: &Path,
    required_files: &[&str],  // NEW: parameterized
    logger: &StdoutLogger,
) -> Result<PathBuf, MemoryError>
```

### 11.3 Shared Constants

```rust
/// Standard required files for HuggingFace safetensors models.
/// Used by both LocalCandleEmbeddingProvider and GlinerEntityExtractor.
pub const MODEL_REQUIRED_FILES: &[&str] = &[
    "tokenizer.json", "config.json", "model.safetensors"
];
```

If a future model variant requires additional files (e.g., `gliner_config.json`), extend with:
```rust
pub const GLINER_EXTRA_FILES: &[&str] = &["gliner_config.json"];
// Usage: MODEL_REQUIRED_FILES.iter().copied().chain(GLINER_EXTRA_FILES.iter().copied())
//   or: [MODEL_REQUIRED_FILES, GLINER_EXTRA_FILES].concat()
```

---

## 12. Monitoring

- Log model download progress
- Log inference latency on each extraction (batch and single-text)
- Log entity count and types extracted

---

## 13. References

- GLiNER paper: https://arxiv.org/abs/2408.47221
- HF Hub: https://huggingface.co/urchade/gliner_multi-v2.1
- GLiNER GitHub (Python): https://github.com/urchade/GLiNER
- GLiNER v0.1.9 batch padding fix: https://github.com/urchade/GLiNER/issues/63
- candle-transformers: https://github.com/huggingface/candle/tree/main/candle-transformers
