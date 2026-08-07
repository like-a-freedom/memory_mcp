//! CPU-only NuNER ONNX entity extractor (`NER_EXTRACTOR=anno-onnx`).
//!
//! Runs the `deepanwa/NuNerZero_onnx` ONNX export locally through ONNX
//! Runtime (via [`anno::create_onnx_session`]). The extractor never downloads:
//! it consumes a prepared local checkpoint — either a `NER_CACHE_DIR`
//! override (used directly, KISS) or the shared [`NerArtifactStore`] default
//! under `<data_dir>/models/ner`.
//!
//! **Artifact repository deviation:** the PyTorch source model
//! `numind/NuNER_Zero` contains no ONNX files (verified). The ONNX export
//! lives at `deepanwa/NuNerZero_onnx` (`model.onnx`, `tokenizer.json`,
//! `config.json`, `gliner_config.json`), so that repository is this
//! backend's artifact source. Do not construct `anno::NuNER::from_pretrained`
//! (it downloads via HF internally) or `StackedNER::default()` (cache- and
//! download-sensitive).
//!
//! **Inference contract (NuNER token mode):** four int64 inputs —
//! `input_ids`, `attention_mask`, `words_mask`, `text_lengths` — and one
//! float32 output of shape `[1, seq_len, num_entity_types]` (span-mode
//! exports with `max_width=1` emit `[1, seq_len, 1, num_entity_types]`).
//! Each word's first token carries its 1-based word id in `words_mask`;
//! continuation tokens are 0. Decoding takes, per word, the argmax logit
//! across entity types, applies sigmoid, and emits a single-word entity when
//! the probability meets the threshold (mirroring GLiNER's span decoder for
//! `max_width=1`; the threshold domain is probabilities, per the (0, 1)
//! `NER_THRESHOLD` validation).
//!
//! The session is cheap to hold, so this backend stores it inline instead of
//! the shared [`LoadedModel`] lifecycle used by heavy Candle backends.
//! `NER_IDLE_UNLOAD_SECS` (0 = retain) is honored by dropping the session
//! after that many idle seconds and lazily rebuilding it from disk.
//! `NER_MAX_CONCURRENCY` is accepted by configuration but not applied here
//! (KISS): ONNX intra-op threads bound CPU parallelism and the session mutex
//! serializes concurrent runs. Inputs are not chunked; sequences beyond the
//! export's maximum length surface as an ONNX error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use tokenizers::Tokenizer;

use crate::models::EntityCandidate;
use crate::service::model_artifacts::{ArtifactRequirement, NerArtifactSpec};

use super::{
    BackendBoxFuture, EntityExtractor, ExtractorFingerprint, MemoryError, NerBuildContext,
};

/// `[START]` special token id (BOS).
const TOKEN_START: i64 = 1;
/// `[END]` special token id (EOS).
const TOKEN_END: i64 = 2;
/// `<<ENT>>` entity-marker token id (gliner `class_token_index`).
const TOKEN_ENT: i64 = 128002;
/// `<<SEP>>` separator token id.
const TOKEN_SEP: i64 = 128003;
/// Evaluated default confidence threshold (NuNER default).
const DEFAULT_THRESHOLD: f64 = 0.5;
/// ONNX session intra-op threads.
const ONNX_THREADS: usize = 4;

/// Artifact requirements for the verified `deepanwa/NuNerZero_onnx` export.
///
/// `numind/NuNER_Zero` (the PyTorch source) has no ONNX files, so the
/// prepared checkpoint comes from the ONNX export repository instead.
pub(crate) const ANN_ONNX_SPEC: NerArtifactSpec = NerArtifactSpec {
    extractor_id: "anno-onnx",
    repository: "deepanwa/NuNerZero_onnx",
    runtime_version: "nuner-zero-onnx",
    files: &[
        ArtifactRequirement {
            path: "model.onnx",
            sha256: None,
        },
        ArtifactRequirement {
            path: "tokenizer.json",
            sha256: None,
        },
        ArtifactRequirement {
            path: "config.json",
            sha256: None,
        },
    ],
    companion_repository: None,
    companion_files: &[],
};

/// Stable public selector/backend name for this extractor.
#[must_use]
pub(crate) fn provider_name() -> &'static str {
    "anno-onnx"
}

/// Builds the durable fingerprint for the configured labels and threshold.
#[must_use]
pub(crate) fn fingerprint_for(labels: &[String], threshold: f64) -> ExtractorFingerprint {
    ExtractorFingerprint {
        selector: provider_name().to_string(),
        backend: provider_name().to_string(),
        repository: Some("deepanwa/NuNerZero_onnx".to_string()),
        revision: None,
        artifact_identity: None,
        labels: labels.to_vec(),
        threshold: Some(threshold),
        revision_status: None,
        validation_status: None,
        runtime_version: env!("CARGO_PKG_VERSION").to_string(),
        effective_device: Some("cpu".to_string()),
    }
}

/// Trims, lowercases, and deduplicates labels in first-declared order
/// (mirrors the `NER_LABELS` normalization in `config::ner`).
#[must_use]
pub(crate) fn normalize_labels(labels: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut result = Vec::new();
    for label in labels
        .iter()
        .map(|label| label.trim().to_ascii_lowercase())
        .filter(|label| !label.is_empty())
    {
        if seen.insert(label.clone()) {
            result.push(label);
        }
    }
    result
}

/// Token-mode encoded prompt: `(input_ids, attention_mask, words_mask, text_lengths)`.
type EncodedPrompt = (Vec<i64>, Vec<i64>, Vec<i64>, i64);

/// Encodes the NuNER token-mode prompt.
///
/// Layout: `[START] (<<ENT>> label)* <<SEP>> word* [END]`. Each word's first
/// token gets its 1-based word id in `words_mask`; special tokens, labels,
/// and continuation tokens get 0. `attention_mask` is all ones.
///
/// Returns `(input_ids, attention_mask, words_mask, text_lengths)` where
/// `text_lengths` is the number of words.
pub(crate) fn encode_prompt(
    tokenizer: &Tokenizer,
    text_words: &[&str],
    entity_types: &[&str],
) -> Result<EncodedPrompt, MemoryError> {
    if text_words.is_empty() || entity_types.is_empty() {
        return Err(MemoryError::Validation(
            "anno-onnx: cannot encode an empty prompt (no words or no entity types)".to_string(),
        ));
    }

    let mut input_ids = Vec::with_capacity(128);
    let mut words_mask = Vec::with_capacity(128);

    input_ids.push(TOKEN_START);
    words_mask.push(0);

    for entity_type in entity_types {
        input_ids.push(TOKEN_ENT);
        words_mask.push(0);
        let encoding = tokenizer
            .encode(entity_type.to_string(), false)
            .map_err(|e| MemoryError::Validation(format!("anno-onnx tokenizer error: {e}")))?;
        for token_id in encoding.get_ids() {
            input_ids.push(i64::from(*token_id));
            words_mask.push(0);
        }
    }

    input_ids.push(TOKEN_SEP);
    words_mask.push(0);

    let mut word_id: i64 = 0;
    for word in text_words {
        let encoding = tokenizer
            .encode(word.to_string(), false)
            .map_err(|e| MemoryError::Validation(format!("anno-onnx tokenizer error: {e}")))?;
        word_id += 1;
        for (token_idx, token_id) in encoding.get_ids().iter().enumerate() {
            input_ids.push(i64::from(*token_id));
            words_mask.push(if token_idx == 0 { word_id } else { 0 });
        }
    }

    input_ids.push(TOKEN_END);
    words_mask.push(0);

    let seq_len = input_ids.len();
    let attention_mask = vec![1; seq_len];
    Ok((input_ids, attention_mask, words_mask, word_id))
}

/// Logistic sigmoid over a raw logit.
fn sigmoid(logit: f32) -> f32 {
    1.0 / (1.0 + (-logit).exp())
}

/// Decodes the token-mode output into single-word spans.
///
/// `scores` is the flattened float32 output and `shape` its dimensions.
/// Accepted shapes: `[1, seq_len, num_classes]` (token mode) and
/// `[1, seq_len, 1, num_classes]` (span mode with `max_width=1`). For each
/// word, takes the argmax logit across entity types, applies sigmoid, and
/// emits `(word_idx, class_idx, probability)` when the probability meets
/// `threshold`. Each word is its own span (`max_width=1`).
pub(crate) fn decode_scores(
    scores: &[f32],
    shape: &[usize],
    num_words: usize,
    num_classes: usize,
    threshold: f64,
) -> Result<Vec<(usize, usize, f64)>, MemoryError> {
    let (len, stride) = match shape {
        [batch, len, classes] if *batch == 1 && *classes == num_classes => (*len, num_classes),
        [batch, len, width, classes] if *batch == 1 && *width == 1 && *classes == num_classes => {
            (*len, num_classes)
        }
        _ => {
            return Err(MemoryError::Validation(format!(
                "anno-onnx: unexpected output shape {shape:?} for {num_classes} labels; \
                 expected [1, len, {num_classes}] or [1, len, 1, {num_classes}]"
            )));
        }
    };

    let expected = len.saturating_mul(stride);
    if scores.len() < expected {
        return Err(MemoryError::Validation(format!(
            "anno-onnx: output tensor has {} values but shape {shape:?} requires {expected}",
            scores.len()
        )));
    }

    let mut spans = Vec::new();
    for word_idx in 0..num_words.min(len) {
        let base = word_idx * stride;
        let mut best_class = 0usize;
        let mut best_logit = f32::NEG_INFINITY;
        for class_idx in 0..num_classes {
            let logit = scores[base + class_idx];
            if logit > best_logit {
                best_logit = logit;
                best_class = class_idx;
            }
        }
        let prob = f64::from(sigmoid(best_logit));
        if prob >= threshold {
            spans.push((word_idx, best_class, prob));
        }
    }
    Ok(spans)
}

/// Builds an ONNX Runtime session from a local `model.onnx`.
///
/// Uses [`anno::create_onnx_session`] with a CPU-only configuration. Note:
/// `anno::OnnxSessionConfig` is `#[non_exhaustive]`, so the thread count is
/// applied by mutating a `Default` value rather than a struct literal.
fn load_session(model_dir: &Path) -> Result<ort::session::Session, MemoryError> {
    let model_path = model_dir.join("model.onnx");
    if !model_path.is_file() {
        return Err(MemoryError::ConfigInvalid(format!(
            "anno-onnx: model.onnx not found under {}",
            model_dir.display()
        )));
    }
    let mut config = anno::OnnxSessionConfig::default();
    config.num_threads = ONNX_THREADS;
    anno::create_onnx_session(&model_path, config)
        .map_err(|e| MemoryError::ConfigInvalid(format!("anno-onnx session load failed: {e}")))
}

/// Builds an int64 ONNX tensor from `(shape, data)` via the version-stable
/// constructor (same path as anno's own `ort_compat`).
fn tensor_i64(shape: Vec<usize>, data: Vec<i64>) -> ort::Result<ort::value::Tensor<i64>> {
    ort::value::Tensor::from_array((shape, data.into_boxed_slice()))
}

/// CPU-only NuNER ONNX extractor. Holds the ONNX session inline (the session
/// is cheap to retain) and honors `idle_unload_secs` by dropping the session
/// after that many idle seconds and rebuilding it lazily from disk.
pub struct AnnoOnnxEntityExtractor {
    model_dir: PathBuf,
    session: Mutex<Option<ort::session::Session>>,
    last_used: Mutex<Instant>,
    tokenizer: Tokenizer,
    labels: Vec<String>,
    threshold: f64,
    idle_unload_secs: u64,
}

impl AnnoOnnxEntityExtractor {
    /// Loads the tokenizer and ONNX session from `model_dir` (must contain
    /// `tokenizer.json` and `model.onnx`).
    pub(crate) fn new(
        model_dir: PathBuf,
        labels: Vec<String>,
        threshold: f64,
        idle_unload_secs: u64,
        logger: crate::logging::StdoutLogger,
    ) -> Result<Self, MemoryError> {
        let labels = normalize_labels(&labels);

        let tokenizer_path = model_dir.join("tokenizer.json");
        if !tokenizer_path.is_file() {
            return Err(MemoryError::ConfigInvalid(format!(
                "anno-onnx: tokenizer.json not found under {}",
                model_dir.display()
            )));
        }
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| MemoryError::ConfigInvalid(format!("anno-onnx tokenizer load: {e}")))?;
        let session = load_session(&model_dir)?;

        logger.log(
            crate::service::log_event(
                "ner.anno_onnx.ready",
                serde_json::json!({
                    "model_dir": model_dir.display().to_string(),
                    "labels": labels,
                    "threshold": threshold,
                    "idle_unload_secs": idle_unload_secs,
                }),
                serde_json::json!({}),
                None,
                None,
                None,
            ),
            crate::logging::LogLevel::Info,
        );

        Ok(Self {
            model_dir,
            session: Mutex::new(Some(session)),
            last_used: Mutex::new(Instant::now()),
            tokenizer,
            labels,
            threshold: threshold.clamp(0.0, 1.0),
            idle_unload_secs,
        })
    }

    /// Returns the session, unloading a stale session first (when
    /// `idle_unload_secs > 0`) and lazily rebuilding from disk on demand.
    fn session(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, Option<ort::session::Session>>, MemoryError> {
        let mut guard = self.session.lock().unwrap_or_else(|e| e.into_inner());
        if self.idle_unload_secs > 0 {
            let idle = Instant::now()
                .duration_since(*self.last_used.lock().unwrap_or_else(|e| e.into_inner()));
            if idle >= Duration::from_secs(self.idle_unload_secs) {
                *guard = None;
            }
        }
        if guard.is_none() {
            *guard = Some(load_session(&self.model_dir)?);
        }
        *self.last_used.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        Ok(guard)
    }

    /// Runs the four tensor inputs and returns the decoded output scores and
    /// shape as owned data (ONNX `SessionOutputs` borrow the session, so they
    /// cannot outlive the session guard).
    fn run_inference(
        &self,
        input_ids: ort::value::Tensor<i64>,
        attention_mask: ort::value::Tensor<i64>,
        words_mask: ort::value::Tensor<i64>,
        text_lengths: ort::value::Tensor<i64>,
    ) -> Result<(Vec<f32>, Vec<usize>), MemoryError> {
        let mut guard = self.session()?;
        let session = guard.as_mut().ok_or_else(|| {
            MemoryError::Validation("anno-onnx: session is unavailable".to_string())
        })?;
        let outputs = session
            .run(ort::inputs![
                "input_ids" => input_ids.into_dyn(),
                "attention_mask" => attention_mask.into_dyn(),
                "words_mask" => words_mask.into_dyn(),
                "text_lengths" => text_lengths.into_dyn(),
            ])
            .map_err(|e| MemoryError::Validation(format!("anno-onnx inference failed: {e}")))?;

        let value = outputs
            .iter()
            .find(|(name, _)| name.contains("logits"))
            .map(|(_, v)| v)
            .or_else(|| outputs.iter().next().map(|(_, v)| v))
            .ok_or_else(|| MemoryError::Validation("anno-onnx: no output tensor".to_string()))?;

        let (_, data) = value
            .try_extract_tensor::<f32>()
            .map_err(|e| MemoryError::Validation(format!("anno-onnx output extract: {e}")))?;
        let scores: Vec<f32> = data.to_vec();

        let shape: Vec<usize> = match value.dtype() {
            ort::value::ValueType::Tensor { shape, .. } => {
                shape.iter().map(|&d| d as usize).collect()
            }
            _ => {
                return Err(MemoryError::Validation(
                    "anno-onnx: expected a tensor output".to_string(),
                ));
            }
        };
        Ok((scores, shape))
    }

    /// Runs NuNER token-mode inference and maps decoded spans to candidates.
    fn extract_inner(
        &self,
        content: &str,
        labels: &[String],
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        if content.trim().is_empty() || labels.is_empty() {
            return Ok(Vec::new());
        }
        let text_words: Vec<&str> = content.split_whitespace().collect();
        if text_words.is_empty() {
            return Ok(Vec::new());
        }
        let label_strs: Vec<&str> = labels.iter().map(String::as_str).collect();

        let (input_ids, attention_mask, words_mask, text_lengths) =
            encode_prompt(&self.tokenizer, &text_words, &label_strs)?;
        let seq_len = input_ids.len();

        let (scores, shape) = self.run_inference(
            tensor_i64(vec![1, seq_len], input_ids)
                .map_err(|e| MemoryError::Validation(format!("anno-onnx tensor build: {e}")))?,
            tensor_i64(vec![1, seq_len], attention_mask)
                .map_err(|e| MemoryError::Validation(format!("anno-onnx tensor build: {e}")))?,
            tensor_i64(vec![1, seq_len], words_mask)
                .map_err(|e| MemoryError::Validation(format!("anno-onnx tensor build: {e}")))?,
            tensor_i64(vec![1, 1], vec![text_lengths])
                .map_err(|e| MemoryError::Validation(format!("anno-onnx tensor build: {e}")))?,
        )?;

        let spans = decode_scores(
            &scores,
            &shape,
            text_words.len(),
            labels.len(),
            self.threshold,
        )?;

        // Deterministic, deduplicated candidates keyed by canonical name
        // (first label wins), matching the other local extractors.
        let mut candidates = BTreeMap::new();
        for (word_idx, class_idx, _prob) in spans {
            let canonical_name = text_words[word_idx].trim();
            if canonical_name.is_empty() {
                continue;
            }
            candidates.insert(
                canonical_name.to_string(),
                EntityCandidate {
                    entity_type: labels[class_idx].clone(),
                    canonical_name: canonical_name.to_string(),
                    aliases: Vec::new(),
                },
            );
        }
        Ok(candidates.into_values().collect())
    }
}

impl std::fmt::Debug for AnnoOnnxEntityExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnnoOnnxEntityExtractor")
            .field("model_dir", &self.model_dir)
            .field("labels", &self.labels)
            .field("threshold", &self.threshold)
            .field("idle_unload_secs", &self.idle_unload_secs)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl EntityExtractor for AnnoOnnxEntityExtractor {
    fn provider_name(&self) -> &'static str {
        provider_name()
    }

    fn fingerprint(&self) -> ExtractorFingerprint {
        fingerprint_for(&self.labels, self.threshold)
    }

    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        self.extract_inner(content, &self.labels)
    }

    async fn extract_candidates_with_labels(
        &self,
        content: &str,
        zero_shot_labels: &[String],
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        let labels = normalize_labels(zero_shot_labels);
        if labels.is_empty() {
            return Ok(Vec::new());
        }
        self.extract_inner(content, &labels)
    }
}

/// Builds the anno-onnx backend from configuration.
///
/// A configured `NER_CACHE_DIR` is used directly as the model directory
/// (test fixtures land here, KISS). Without one, the shared artifact store
/// prepares `deepanwa/NuNerZero_onnx` under `<data_dir>/models/ner` and the
/// extractor consumes the prepared checkpoint root.
pub(crate) fn build(
    config: crate::config::NerExtractorConfig,
    context: NerBuildContext,
) -> BackendBoxFuture {
    Box::pin(async move {
        let crate::config::NerExtractorConfig::AnnoOnnx(model) = config else {
            return Err(MemoryError::ConfigInvalid(
                "anno_onnx::build requires NER_EXTRACTOR=anno-onnx".to_string(),
            ));
        };

        let threshold = model.threshold.unwrap_or(DEFAULT_THRESHOLD);
        let model_dir = match model.cache_dir {
            Some(dir) => dir,
            None => {
                let store_root = context.data_dir.join("models").join("ner");
                let progress: Arc<dyn crate::service::model_artifacts::ModelProgressSink> =
                    Arc::new(crate::service::model_artifacts::CliProgressSink::new());
                let store =
                    crate::service::model_artifacts::NerArtifactStore::new(store_root, progress)?;
                let checkpoint = store.prepare(&ANN_ONNX_SPEC).await?;
                checkpoint.root
            }
        };

        let extractor = AnnoOnnxEntityExtractor::new(
            model_dir,
            model.labels,
            threshold,
            model.idle_unload_secs,
            context.logger,
        )?;

        Ok(Arc::new(extractor) as Arc<dyn EntityExtractor>)
    })
}

#[cfg(test)]
mod tests {
    use tokenizers::Tokenizer;
    use tokenizers::models::wordpiece::WordPiece;

    use super::*;

    /// Builds a tiny deterministic wordpiece tokenizer for prompt-encoding
    /// tests. Specials are deliberately NOT registered so `encode(_, false)`
    /// emits exactly the vocab ids we assert on.
    fn test_tokenizer() -> Tokenizer {
        let vocab = [
            ("[PAD]".to_string(), 0u32),
            ("[UNK]".to_string(), 1),
            ("alice".to_string(), 100),
            ("smith".to_string(), 101),
            ("##s".to_string(), 102),
            ("openai".to_string(), 103),
            ("person".to_string(), 104),
            ("company".to_string(), 105),
        ];
        let wordpiece = WordPiece::builder()
            .vocab(vocab)
            .unk_token("[UNK]".to_string())
            .build()
            .expect("wordpiece");
        Tokenizer::new(wordpiece)
    }

    #[test]
    fn normalize_labels_trims_lowercases_and_dedupes_in_order() {
        let labels = normalize_labels(&[
            " Person".to_string(),
            "person".to_string(),
            "COMPANY".to_string(),
            "".to_string(),
            " company".to_string(),
            "location".to_string(),
        ]);
        assert_eq!(labels, vec!["person", "company", "location"]);
    }

    #[test]
    fn normalize_labels_empty_input_is_empty() {
        assert!(normalize_labels(&[]).is_empty());
    }

    #[test]
    fn encode_prompt_emits_special_tokens_and_word_mask() {
        let tokenizer = test_tokenizer();
        let words = ["alice", "smiths", "openai"];
        let labels = ["person", "company"];
        let (input_ids, attention_mask, words_mask, text_lengths) =
            encode_prompt(&tokenizer, &words, &labels).expect("encode");

        // [START] <<ENT>> person <<ENT>> company <<SEP>> alice smith ##s openai [END]
        assert_eq!(
            input_ids,
            vec![
                1, // [START]
                128002, 104, // <<ENT>> person
                128002, 105,    // <<ENT>> company
                128003, // <<SEP>>
                100,    // alice
                101, 102, // smiths -> smith ##s
                103, // openai
                2,   // [END]
            ]
        );
        assert_eq!(words_mask, vec![0, 0, 0, 0, 0, 0, 1, 2, 0, 3, 0]);
        assert_eq!(attention_mask.len(), input_ids.len());
        assert!(attention_mask.iter().all(|&m| m == 1));
        assert_eq!(text_lengths, 3);
    }

    #[test]
    fn encode_prompt_rejects_empty_words_or_labels() {
        let tokenizer = test_tokenizer();
        assert!(encode_prompt(&tokenizer, &[], &["person"]).is_err());
        assert!(encode_prompt(&tokenizer, &["alice"], &[]).is_err());
    }

    #[test]
    fn decode_scores_emits_thresholded_argmax_per_word() {
        // 3 words x 2 classes, token-mode 3D output.
        let scores = [
            5.0, -5.0, // word 0: person
            -5.0, 5.0, // word 1: company
            -5.0, -5.0, // word 2: below threshold
        ];
        let spans = decode_scores(&scores, &[1, 3, 2], 3, 2, 0.5).expect("decode");
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].0, spans[0].1), (0, 0));
        assert!(spans[0].2 > 0.9);
        assert_eq!((spans[1].0, spans[1].1), (1, 1));
        assert!(spans[1].2 > 0.9);
    }

    #[test]
    fn decode_scores_supports_span_shape_with_max_width_one() {
        // [1, words, max_width=1, classes] flattened identically to token mode.
        let scores = [5.0, -5.0, -5.0, 5.0];
        let spans = decode_scores(&scores, &[1, 2, 1, 2], 2, 2, 0.5).expect("decode");
        assert_eq!(spans.len(), 2);
        assert_eq!((spans[0].0, spans[0].1), (0, 0));
        assert_eq!((spans[1].0, spans[1].1), (1, 1));
        for (_, _, prob) in spans {
            assert!((prob - 0.993_307_149_075_715_3).abs() < 1e-5);
        }
    }

    #[test]
    fn decode_scores_respects_threshold() {
        // logit 1.5 -> sigmoid ~0.8176; excluded at 0.9, included at 0.8.
        let scores = [1.5, 1.5];
        let strict = decode_scores(&scores, &[1, 1, 2], 1, 2, 0.9).expect("decode");
        assert!(strict.is_empty());
        let lenient = decode_scores(&scores, &[1, 1, 2], 1, 2, 0.8).expect("decode");
        assert_eq!(lenient.len(), 1);
        assert_eq!(lenient[0].1, 0);
    }

    #[test]
    fn decode_scores_rejects_degenerate_shapes() {
        assert!(decode_scores(&[0.0], &[], 1, 1, 0.5).is_err());
        // width != 1 is unsupported (max_width=1 contract).
        assert!(decode_scores(&[0.0; 4], &[1, 1, 2, 1], 1, 1, 0.5).is_err());
        // class-count mismatch with labels.
        assert!(decode_scores(&[0.0; 3], &[1, 1, 2], 1, 3, 0.5).is_err());
    }

    #[test]
    fn provider_name_is_stable() {
        assert_eq!(provider_name(), "anno-onnx");
    }

    #[test]
    fn fingerprint_carries_onnx_identity_fields() {
        let fp = fingerprint_for(&["person".to_string(), "company".to_string()], 0.5);
        assert_eq!(fp.selector, "anno-onnx");
        assert_eq!(fp.backend, "anno-onnx");
        assert_eq!(fp.repository.as_deref(), Some("deepanwa/NuNerZero_onnx"));
        assert_eq!(fp.revision, None);
        assert_eq!(fp.artifact_identity, None);
        assert_eq!(fp.labels, vec!["person", "company"]);
        assert_eq!(fp.threshold, Some(0.5));
        assert_eq!(fp.revision_status, None);
        assert_eq!(fp.validation_status, None);
        assert_eq!(fp.effective_device.as_deref(), Some("cpu"));
        assert_eq!(fp.runtime_version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn artifact_spec_points_at_deepanwa_onnx_export() {
        assert_eq!(ANN_ONNX_SPEC.extractor_id, "anno-onnx");
        assert_eq!(ANN_ONNX_SPEC.repository, "deepanwa/NuNerZero_onnx");
        assert_eq!(ANN_ONNX_SPEC.runtime_version, "nuner-zero-onnx");
        let paths: Vec<_> = ANN_ONNX_SPEC.files.iter().map(|f| f.path).collect();
        assert_eq!(paths, vec!["model.onnx", "tokenizer.json", "config.json"]);
    }

    #[test]
    fn new_with_missing_model_dir_fails_with_config_invalid() {
        let logger = crate::logging::StdoutLogger::new("error");
        let missing =
            std::env::temp_dir().join(format!("anno-onnx-missing-{}", std::process::id()));
        let result =
            AnnoOnnxEntityExtractor::new(missing, vec!["person".to_string()], 0.5, 0, logger);
        assert!(matches!(result, Err(MemoryError::ConfigInvalid(_))));
    }
}
