use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::config::{EmbeddingConfig, EmbeddingProviderKind};
use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;
use crate::storage::json_f64;

static EMBEDDING_LOGGER: std::sync::OnceLock<StdoutLogger> = std::sync::OnceLock::new();

fn embedding_logger() -> &'static StdoutLogger {
    EMBEDDING_LOGGER.get_or_init(|| StdoutLogger::new("warn"))
}

/// Abstraction over optional embedding providers.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Returns true when the provider is active.
    fn is_enabled(&self) -> bool;

    /// Human-readable provider kind used in logs.
    fn provider_name(&self) -> &'static str;

    /// Expected embedding dimension.
    fn dimension(&self) -> usize;

    /// Requests an embedding vector for the supplied input text.
    async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError>;
}

/// Provider implementation used when embeddings are disabled.
pub struct DisabledEmbeddingProvider {
    dimension: usize,
}

impl DisabledEmbeddingProvider {
    #[must_use]
    pub fn new(dimension: Option<usize>) -> Self {
        Self {
            dimension: dimension.unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSION),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for DisabledEmbeddingProvider {
    fn is_enabled(&self) -> bool {
        false
    }

    fn provider_name(&self) -> &'static str {
        "disabled"
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
        Err(MemoryError::Validation(
            "embedding provider is disabled".to_string(),
        ))
    }
}

struct OpenAiCompatibleEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    dimension: usize,
}

struct OllamaEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    dimension: usize,
}

pub(crate) async fn create_embedding_provider(
    config: &EmbeddingConfig,
    data_dir: &str,
) -> Result<Arc<dyn EmbeddingProvider>, MemoryError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.timeout_secs))
        .build()
        .map_err(|err| {
            MemoryError::ConfigInvalid(format!("invalid embedding HTTP client: {err}"))
        })?;

    let dimension = config
        .dimension
        .unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSION);

    match config.provider {
        EmbeddingProviderKind::Disabled => {
            let provider: Arc<dyn EmbeddingProvider> =
                Arc::new(DisabledEmbeddingProvider::new(config.dimension));
            Ok(provider)
        }
        EmbeddingProviderKind::LocalCandle => {
            let model_dir_str = config.model_dir_or_default(data_dir);
            let model_dir = std::path::Path::new(&model_dir_str);
            let model_name = config
                .model
                .as_deref()
                .unwrap_or("intfloat/multilingual-e5-small");
            let model_dim = dimension;

            // Download model if not cached (with retry/resume on failure)
            let logger = crate::logging::StdoutLogger::new("info");
            let resolved_dir =
                crate::service::model_loader::ensure_model_cached(model_name, model_dir, &logger)
                    .await
                    .map_err(|e| {
                        MemoryError::Storage(format!(
                            "failed to download/cache model {model_name}: {e}"
                        ))
                    })?;

            let provider: Arc<dyn EmbeddingProvider> = Arc::new(LocalCandleEmbeddingProvider::new(
                model_name,
                model_dim,
                config.max_tokens,
                &resolved_dir,
            )?);
            Ok(provider)
        }
        EmbeddingProviderKind::OpenAiCompatible => {
            let provider: Arc<dyn EmbeddingProvider> =
                Arc::new(OpenAiCompatibleEmbeddingProvider {
                    client,
                    base_url: config.base_url.clone().ok_or_else(|| {
                        MemoryError::ConfigMissing("EMBEDDINGS_BASE_URL".to_string())
                    })?,
                    model: config.model.clone().ok_or_else(|| {
                        MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string())
                    })?,
                    api_key: config.api_key.clone(),
                    dimension,
                });
            Ok(provider)
        }
        EmbeddingProviderKind::Ollama => {
            let provider: Arc<dyn EmbeddingProvider> =
                Arc::new(OllamaEmbeddingProvider {
                    client,
                    base_url: config.base_url.clone().ok_or_else(|| {
                        MemoryError::ConfigMissing("EMBEDDINGS_BASE_URL".to_string())
                    })?,
                    model: config.model.clone().ok_or_else(|| {
                        MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string())
                    })?,
                    dimension,
                });
            Ok(provider)
        }
    }
}

pub(crate) fn embedding_from_value(value: &Value) -> Option<Vec<f64>> {
    let array = value.as_array()?;
    let mut embedding = Vec::with_capacity(array.len());

    for item in array {
        embedding.push(json_f64(item)?);
    }

    Some(normalize_embedding(embedding))
}

pub(crate) fn cosine_similarity(left: &[f64], right: &[f64]) -> f64 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }

    if left.len() != right.len() {
        use std::collections::HashMap;
        let mut event = HashMap::new();
        event.insert(
            "op".to_string(),
            json!("cosine_similarity.dimension_mismatch"),
        );
        event.insert("left_dim".to_string(), json!(left.len()));
        event.insert("right_dim".to_string(), json!(right.len()));
        embedding_logger().log(event, LogLevel::Warn);
        return 0.0;
    }

    left.iter().zip(right.iter()).map(|(l, r)| l * r).sum()
}

fn normalize_embedding(mut embedding: Vec<f64>) -> Vec<f64> {
    let magnitude = embedding
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    if magnitude <= f64::EPSILON {
        return embedding;
    }

    for value in &mut embedding {
        *value /= magnitude;
    }

    embedding
}

#[async_trait]
impl EmbeddingProvider for OpenAiCompatibleEmbeddingProvider {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "openai-compatible"
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        let mut headers =
            HeaderMap::from_iter([(CONTENT_TYPE, HeaderValue::from_static("application/json"))]);
        if let Some(api_key) = &self.api_key {
            let value = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
                MemoryError::ConfigInvalid(format!("invalid EMBEDDINGS_API_KEY header: {err}"))
            })?;
            headers.insert(AUTHORIZATION, value);
        }

        let response = self
            .client
            .post(format!(
                "{}/embeddings",
                self.base_url.trim_end_matches('/')
            ))
            .headers(headers)
            .json(&json!({"model": self.model, "input": input}))
            .send()
            .await
            .map_err(|err| MemoryError::Storage(format!("embedding request failed: {err}")))?
            .error_for_status()
            .map_err(|err| {
                MemoryError::Storage(format!("embedding request returned error status: {err}"))
            })?;

        let body = response.json::<Value>().await.map_err(|err| {
            MemoryError::Storage(format!("embedding response decode failed: {err}"))
        })?;

        // Don't validate dimension here - it's validated at the storage layer
        parse_openai_embedding_response_without_validation(&body)
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbeddingProvider {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "ollama"
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        let response = self
            .client
            .post(format!(
                "{}/api/embeddings",
                self.base_url.trim_end_matches('/')
            ))
            .json(&json!({"model": self.model, "prompt": input}))
            .send()
            .await
            .map_err(|err| MemoryError::Storage(format!("embedding request failed: {err}")))?
            .error_for_status()
            .map_err(|err| {
                MemoryError::Storage(format!("embedding request returned error status: {err}"))
            })?;

        let body = response.json::<Value>().await.map_err(|err| {
            MemoryError::Storage(format!("embedding response decode failed: {err}"))
        })?;

        // Don't validate dimension here - it's validated at the storage layer
        parse_ollama_embedding_response_without_validation(&body)
    }
}

/// Local embedding provider using Candle inference engine.
///
/// Loads a BERT-family model (e.g., multilingual-e5-small) from disk
/// and runs inference locally without network dependencies.
#[derive(Clone)]
pub struct LocalCandleEmbeddingProvider {
    _model_name: String,
    dimension: usize,
    max_tokens: usize,
    tokenizer: std::sync::Arc<tokenizers::Tokenizer>,
    bert_model: std::sync::Arc<candle_transformers::models::bert::BertModel>,
    device: Device,
}

impl LocalCandleEmbeddingProvider {
    /// Creates a new LocalCandle provider.
    ///
    /// Downloads model if not cached, then loads tokenizer and weights.
    pub fn new(
        model_name: &str,
        dimension: usize,
        max_tokens: usize,
        model_dir: &std::path::Path,
    ) -> Result<Self, MemoryError> {
        let tokenizer_path = model_dir.join("tokenizer.json");
        let config_path = model_dir.join("config.json");
        let weights_path = model_dir.join("model.safetensors");

        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| MemoryError::Storage(format!("failed to load tokenizer: {e}")))?;

        let config_str = std::fs::read_to_string(&config_path)
            .map_err(|e| MemoryError::Storage(format!("failed to read config.json: {e}")))?;
        let bert_config: candle_transformers::models::bert::Config =
            serde_json::from_str(&config_str)
                .map_err(|e| MemoryError::Storage(format!("failed to parse bert config: {e}")))?;

        let device = Device::Cpu;
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(
                &[&weights_path],
                candle_transformers::models::bert::DTYPE,
                &device,
            )
        }
        .map_err(|e| MemoryError::Storage(format!("failed to load model weights: {e}")))?;

        // Keep HuggingFace tensor names intact.
        // Candle's BertModel loader expects the standard `embeddings.*` / `encoder.*`
        // layout and already handles optional top-level model prefixes internally.

        let bert_model = candle_transformers::models::bert::BertModel::load(vb, &bert_config)
            .map_err(|e| MemoryError::Storage(format!("failed to build bert model: {e}")))?;

        let bert_model = std::sync::Arc::new(bert_model);
        let tokenizer = std::sync::Arc::new(tokenizer);

        // Probe the model to verify the configured dimension matches actual output.
        // Run a minimal forward pass with synthetic token IDs to avoid tokenizer
        // dependencies — any valid input will produce the correct output dimension.
        let probe = {
            let bert_model_ref = bert_model.clone();
            let device = Device::Cpu;
            // Minimal input: 2 tokens (simulating CLS + SEP)
            let input_ids = Tensor::new(&[0u32, 0u32], &device)
                .map_err(|e| MemoryError::Storage(format!("probe tensor failed: {e}")))?
                .unsqueeze(0)
                .map_err(|e| MemoryError::Storage(format!("probe unsqueeze failed: {e}")))?;
            let type_ids = Tensor::zeros_like(&input_ids)
                .map_err(|e| MemoryError::Storage(format!("probe zeros failed: {e}")))?;
            let attention_mask = Tensor::ones_like(&input_ids)
                .map_err(|e| MemoryError::Storage(format!("probe ones failed: {e}")))?;

            let outputs = bert_model_ref
                .forward(&input_ids, &type_ids, Some(&attention_mask))
                .map_err(|e| MemoryError::Storage(format!("probe forward failed: {e}")))?;

            // Mean pooling
            let mask = attention_mask
                .unsqueeze(2)
                .map_err(|e| MemoryError::Storage(format!("probe mask unsqueeze failed: {e}")))?
                .to_dtype(outputs.dtype())
                .map_err(|e| MemoryError::Storage(format!("probe mask dtype failed: {e}")))?;
            let masked = outputs
                .broadcast_mul(&mask)
                .map_err(|e| MemoryError::Storage(format!("probe broadcast_mul failed: {e}")))?
                .sum(1)
                .map_err(|e| MemoryError::Storage(format!("probe sum failed: {e}")))?;
            let mask_sum = mask
                .sum(1)
                .map_err(|e| MemoryError::Storage(format!("probe mask_sum failed: {e}")))?;
            let pooled = masked
                .broadcast_div(&mask_sum)
                .map_err(|e| MemoryError::Storage(format!("probe broadcast_div failed: {e}")))?;

            // L2 norm
            let norm = pooled
                .sqr()
                .map_err(|e| MemoryError::Storage(format!("probe sqr failed: {e}")))?
                .sum(1)
                .map_err(|e| MemoryError::Storage(format!("probe norm_sum failed: {e}")))?
                .unsqueeze(1)
                .map_err(|e| MemoryError::Storage(format!("probe unsqueeze failed: {e}")))?
                .sqrt()
                .map_err(|e| MemoryError::Storage(format!("probe sqrt failed: {e}")))?;
            let normalized = pooled
                .broadcast_div(&norm)
                .map_err(|e| MemoryError::Storage(format!("probe l2 norm failed: {e}")))?;

            normalized
                .squeeze(0)
                .map_err(|e| MemoryError::Storage(format!("probe squeeze failed: {e}")))?
                .to_vec1::<f32>()
                .map_err(|e| MemoryError::Storage(format!("probe to_vec1 failed: {e}")))?
        };

        if probe.len() != dimension {
            return Err(MemoryError::Storage(format!(
                "configured dimension {dimension} != model output dimension {}",
                probe.len()
            )));
        }

        Ok(Self {
            _model_name: model_name.to_string(),
            dimension,
            max_tokens,
            tokenizer,
            bert_model,
            device: Device::Cpu,
        })
    }

    /// Runs the embedding pipeline: tokenize → forward → mean pool → L2 normalize.
    fn embed_inner(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        // E5 models expect "query: " prefix for query embeddings
        let prefixed = format!("query: {input}");

        let encoding = self
            .tokenizer
            .encode(prefixed.as_str(), true)
            .map_err(|e| MemoryError::Storage(format!("tokenization failed: {e}")))?;

        let input_ids = encoding.get_ids();
        let attention_mask = encoding.get_attention_mask();
        let token_type_ids = vec![0u32; input_ids.len()];

        let input_ids = Tensor::new(input_ids, &self.device)
            .map_err(|e| MemoryError::Storage(format!("tensor creation failed: {e}")))?
            .unsqueeze(0)
            .map_err(|e| MemoryError::Storage(format!("unsqueeze failed: {e}")))?;

        let attention_mask = Tensor::new(attention_mask, &self.device)
            .map_err(|e| MemoryError::Storage(format!("tensor creation failed: {e}")))?
            .unsqueeze(0)
            .map_err(|e| MemoryError::Storage(format!("unsqueeze failed: {e}")))?;

        let token_type_ids = Tensor::new(token_type_ids, &self.device)
            .map_err(|e| MemoryError::Storage(format!("tensor creation failed: {e}")))?
            .unsqueeze(0)
            .map_err(|e| MemoryError::Storage(format!("unsqueeze failed: {e}")))?;

        let outputs = self
            .bert_model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))
            .map_err(|e| MemoryError::Storage(format!("bert forward failed: {e}")))?;

        // Mean pooling over token embeddings using attention mask
        let mask = attention_mask
            .unsqueeze(2)
            .map_err(|e| MemoryError::Storage(format!("unsqueeze failed: {e}")))?
            .to_dtype(outputs.dtype())
            .map_err(|e| MemoryError::Storage(format!("dtype conversion failed: {e}")))?;

        let masked = (outputs
            .broadcast_mul(&mask)
            .map_err(|e| MemoryError::Storage(format!("broadcast_mul failed: {e}")))?)
        .sum(1)
        .map_err(|e| MemoryError::Storage(format!("sum failed: {e}")))?;

        let mask_sum = mask
            .sum(1)
            .map_err(|e| MemoryError::Storage(format!("mask sum failed: {e}")))?;

        let pooled = masked
            .broadcast_div(&mask_sum)
            .map_err(|e| MemoryError::Storage(format!("broadcast_div failed: {e}")))?;

        // L2 normalization (per-sample, safe for batch_size > 1)
        let norm = pooled
            .sqr()
            .map_err(|e| MemoryError::Storage(format!("sqr failed: {e}")))?
            .sum(1)
            .map_err(|e| MemoryError::Storage(format!("sum failed: {e}")))?
            .unsqueeze(1)
            .map_err(|e| MemoryError::Storage(format!("unsqueeze failed: {e}")))?
            .sqrt()
            .map_err(|e| MemoryError::Storage(format!("sqrt failed: {e}")))?;

        let normalized = pooled
            .broadcast_div(&norm)
            .map_err(|e| MemoryError::Storage(format!("l2 norm failed: {e}")))?;

        // Convert f32 → f64
        let vec_f32 = normalized
            .squeeze(0)
            .map_err(|e| MemoryError::Storage(format!("squeeze failed: {e}")))?
            .to_vec1::<f32>()
            .map_err(|e| MemoryError::Storage(format!("to_vec1 failed: {e}")))?;

        let vec_f64: Vec<f64> = vec_f32.into_iter().map(f64::from).collect();

        Ok(vec_f64)
    }
}

#[async_trait]
impl EmbeddingProvider for LocalCandleEmbeddingProvider {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "local-candle"
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        let provider = self.clone();
        let input_owned = input.to_string();
        tokio::task::spawn_blocking(move || provider.embed_sync(&input_owned))
            .await
            .map_err(|e| MemoryError::Storage(format!("embedding task panicked: {e}")))?
    }
}

impl LocalCandleEmbeddingProvider {
    /// Synchronous embedding pipeline: tokenize → check length → embed (single or chunked).
    fn embed_sync(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        // E5 prefix "query: " adds ~2 tokens; account for it in the length check
        const E5_PREFIX_TOKENS: usize = 2;

        let encoding = self
            .tokenizer
            .encode(input, true)
            .map_err(|e| MemoryError::Storage(format!("tokenization failed: {e}")))?;

        let token_count = encoding.len();

        if token_count + E5_PREFIX_TOKENS <= self.max_tokens {
            return self.embed_inner(input);
        }

        let chunk_size = (self.max_tokens as f64 * 0.8) as usize;
        let overlap = (chunk_size as f64 * 0.1) as usize;

        let token_ids = encoding.get_ids().to_vec();
        let chunks = split_tokens_with_overlap(&token_ids, chunk_size, overlap);

        let mut embeddings = Vec::with_capacity(chunks.len());
        for chunk_ids in chunks {
            let chunk_text = self
                .tokenizer
                .decode(&chunk_ids, true)
                .map_err(|e| MemoryError::Storage(format!("decode failed: {e}")))?;
            let emb = self.embed_inner(&chunk_text)?;
            embeddings.push(emb);
        }

        mean_pool_embeddings(&embeddings)
    }
}

fn split_tokens_with_overlap(tokens: &[u32], chunk_size: usize, overlap: usize) -> Vec<Vec<u32>> {
    if tokens.len() <= chunk_size {
        return vec![tokens.to_vec()];
    }

    let mut chunks = Vec::new();
    let mut start = 0;

    while start < tokens.len() {
        let end = (start + chunk_size).min(tokens.len());
        chunks.push(tokens[start..end].to_vec());

        if end == tokens.len() {
            break;
        }

        start += chunk_size - overlap;
    }

    chunks
}

fn mean_pool_embeddings(embeddings: &[Vec<f64>]) -> Result<Vec<f64>, MemoryError> {
    if embeddings.is_empty() {
        return Err(MemoryError::Storage("no embeddings to pool".to_string()));
    }

    if embeddings.len() == 1 {
        return Ok(embeddings[0].clone());
    }

    let dim = embeddings[0].len();
    let mut pooled = vec![0.0; dim];

    for emb in embeddings {
        if emb.len() != dim {
            return Err(MemoryError::Storage(format!(
                "embedding dimension mismatch: expected {}, got {}",
                dim,
                emb.len()
            )));
        }
        for (i, val) in emb.iter().enumerate() {
            pooled[i] += val;
        }
    }

    let count = embeddings.len() as f64;
    for val in &mut pooled {
        *val /= count;
    }

    let norm: f64 = pooled.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm > 0.0 {
        for val in &mut pooled {
            *val /= norm;
        }
    }

    Ok(pooled)
}

fn parse_openai_embedding_response_without_validation(
    body: &Value,
) -> Result<Vec<f64>, MemoryError> {
    body.get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("embedding"))
        .and_then(embedding_from_value)
        .ok_or_else(|| {
            MemoryError::Storage("embedding response missing data[0].embedding".to_string())
        })
}

fn parse_ollama_embedding_response_without_validation(
    body: &Value,
) -> Result<Vec<f64>, MemoryError> {
    body.get("embedding")
        .and_then(embedding_from_value)
        .ok_or_else(|| {
            MemoryError::Storage("embedding response missing embedding array".to_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    const TEST_HIDDEN_SIZE: usize = 2;
    const TEST_INTERMEDIATE_SIZE: usize = 4;
    const TEST_MAX_POSITION_EMBEDDINGS: usize = 8;
    const TEST_NUM_ATTENTION_HEADS: usize = 1;
    const TEST_NUM_HIDDEN_LAYERS: usize = 1;
    const TEST_TYPE_VOCAB_SIZE: usize = 2;
    const TEST_VOCAB_SIZE: usize = 8;

    fn write_minimal_tokenizer(path: &Path) {
        let tokenizer = tokenizers::Tokenizer::new(tokenizers::models::bpe::BPE::default());
        tokenizer.save(path, false).expect("save tokenizer");
    }

    fn write_minimal_bert_config(path: &Path) {
        let config = json!({
            "vocab_size": TEST_VOCAB_SIZE,
            "hidden_size": TEST_HIDDEN_SIZE,
            "num_hidden_layers": TEST_NUM_HIDDEN_LAYERS,
            "num_attention_heads": TEST_NUM_ATTENTION_HEADS,
            "intermediate_size": TEST_INTERMEDIATE_SIZE,
            "hidden_act": "gelu",
            "hidden_dropout_prob": 0.1,
            "max_position_embeddings": TEST_MAX_POSITION_EMBEDDINGS,
            "type_vocab_size": TEST_TYPE_VOCAB_SIZE,
            "initializer_range": 0.02,
            "layer_norm_eps": 1e-12,
            "pad_token_id": 0,
            "position_embedding_type": "absolute",
            "use_cache": true,
            "classifier_dropout": null,
            "model_type": "bert"
        });

        std::fs::write(path, serde_json::to_vec(&config).expect("serialize config"))
            .expect("write config");
    }

    fn prefixed_test_tensors() -> Vec<(&'static str, Vec<usize>)> {
        vec![
            (
                "embeddings.word_embeddings.weight",
                vec![TEST_VOCAB_SIZE, TEST_HIDDEN_SIZE],
            ),
            (
                "embeddings.position_embeddings.weight",
                vec![TEST_MAX_POSITION_EMBEDDINGS, TEST_HIDDEN_SIZE],
            ),
            (
                "embeddings.token_type_embeddings.weight",
                vec![TEST_TYPE_VOCAB_SIZE, TEST_HIDDEN_SIZE],
            ),
            ("embeddings.LayerNorm.weight", vec![TEST_HIDDEN_SIZE]),
            ("embeddings.LayerNorm.bias", vec![TEST_HIDDEN_SIZE]),
            (
                "encoder.layer.0.attention.self.query.weight",
                vec![TEST_HIDDEN_SIZE, TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.attention.self.query.bias",
                vec![TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.attention.self.key.weight",
                vec![TEST_HIDDEN_SIZE, TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.attention.self.key.bias",
                vec![TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.attention.self.value.weight",
                vec![TEST_HIDDEN_SIZE, TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.attention.self.value.bias",
                vec![TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.attention.output.dense.weight",
                vec![TEST_HIDDEN_SIZE, TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.attention.output.dense.bias",
                vec![TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.attention.output.LayerNorm.weight",
                vec![TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.attention.output.LayerNorm.bias",
                vec![TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.intermediate.dense.weight",
                vec![TEST_INTERMEDIATE_SIZE, TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.intermediate.dense.bias",
                vec![TEST_INTERMEDIATE_SIZE],
            ),
            (
                "encoder.layer.0.output.dense.weight",
                vec![TEST_HIDDEN_SIZE, TEST_INTERMEDIATE_SIZE],
            ),
            ("encoder.layer.0.output.dense.bias", vec![TEST_HIDDEN_SIZE]),
            (
                "encoder.layer.0.output.LayerNorm.weight",
                vec![TEST_HIDDEN_SIZE],
            ),
            (
                "encoder.layer.0.output.LayerNorm.bias",
                vec![TEST_HIDDEN_SIZE],
            ),
        ]
    }

    fn write_minimal_prefixed_bert_weights(path: &Path) {
        let mut header = serde_json::Map::new();
        let mut data = Vec::new();

        for (name, shape) in prefixed_test_tensors() {
            let element_count = shape.iter().product::<usize>();
            let byte_len = element_count * std::mem::size_of::<f32>();
            let start = data.len() as u64;
            data.resize(data.len() + byte_len, 0);
            let end = data.len() as u64;

            header.insert(
                name.to_string(),
                json!({
                    "dtype": "F32",
                    "shape": shape,
                    "data_offsets": [start, end]
                }),
            );
        }

        let header_bytes = serde_json::to_vec(&header).expect("serialize safetensors header");
        let mut encoded = Vec::with_capacity(8 + header_bytes.len() + data.len());
        encoded.extend_from_slice(&(header_bytes.len() as u64).to_le_bytes());
        encoded.extend_from_slice(&header_bytes);
        encoded.extend_from_slice(&data);

        std::fs::write(path, encoded).expect("write safetensors");
    }

    #[test]
    fn local_candle_provider_loads_prefixed_bert_weights() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_tokenizer(&dir.path().join("tokenizer.json"));
        write_minimal_bert_config(&dir.path().join("config.json"));
        write_minimal_prefixed_bert_weights(&dir.path().join("model.safetensors"));

        let result =
            LocalCandleEmbeddingProvider::new("test/model", TEST_HIDDEN_SIZE, 384, dir.path());

        if let Err(error) = result {
            panic!("expected prefixed bert weights to load successfully, got {error}");
        }
    }

    #[test]
    fn parse_openai_embedding_response_without_validation_reads_vector() {
        let embedding = parse_openai_embedding_response_without_validation(&json!({
            "data": [
                {"embedding": [0.1, 0.2, 0.3]}
            ]
        }))
        .expect("embedding");

        assert_eq!(
            embedding,
            vec![0.2672612419124244, 0.5345224838248488, 0.8017837257372731]
        );
    }

    #[test]
    fn parse_ollama_embedding_response_without_validation_reads_vector() {
        let embedding = parse_ollama_embedding_response_without_validation(
            &json!({"embedding": [0.4, 0.5, 0.6]}),
        )
        .expect("embedding");

        assert_eq!(
            embedding,
            vec![0.4558423058385518, 0.5698028822981898, 0.6837634587578276]
        );
    }

    #[test]
    fn cosine_similarity_identical_vectors() {
        let a = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &a) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!((cosine_similarity(&a, &b)).abs() < f64::EPSILON);
    }

    #[test]
    fn cosine_similarity_opposite_vectors() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn cosine_similarity_empty_vectors() {
        assert_eq!(cosine_similarity(&[], &[1.0]), 0.0);
        assert_eq!(cosine_similarity(&[1.0], &[]), 0.0);
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn cosine_similarity_dimension_mismatch() {
        let a = vec![1.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert_eq!(cosine_similarity(&a, &b), 0.0);
    }

    #[test]
    fn embedding_from_value_parses_number_array() {
        let val = json!([1.0, 0.0, 0.0]);
        let emb = embedding_from_value(&val);
        assert!(emb.is_some());
        let emb = emb.unwrap();
        assert_eq!(emb.len(), 3);
    }

    #[test]
    fn embedding_from_value_normalizes_vector() {
        let val = json!([3.0, 4.0]);
        let emb = embedding_from_value(&val).unwrap();
        let mag = emb.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((mag - 1.0).abs() < 1e-10);
    }

    #[test]
    fn embedding_from_value_returns_none_for_non_array() {
        assert!(embedding_from_value(&json!("string")).is_none());
        assert!(embedding_from_value(&json!(123)).is_none());
        assert!(embedding_from_value(&json!({"a": 1})).is_none());
    }

    #[test]
    fn embedding_from_value_handles_empty_array() {
        let emb = embedding_from_value(&json!([])).unwrap();
        assert!(emb.is_empty());
    }

    #[test]
    fn disabled_embedding_provider_returns_disabled() {
        let provider = DisabledEmbeddingProvider::new(None);
        assert!(!provider.is_enabled());
        assert_eq!(provider.provider_name(), "disabled");
    }

    #[test]
    fn disabled_embedding_provider_embed_returns_error() {
        let provider = DisabledEmbeddingProvider::new(None);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(provider.embed("test"));
        assert!(result.is_err());
    }

    #[test]
    fn disabled_embedding_provider_uses_default_dimension() {
        let provider = DisabledEmbeddingProvider::new(None);
        assert_eq!(
            provider.dimension(),
            crate::config::DEFAULT_EMBEDDING_DIMENSION
        );
    }

    #[test]
    fn disabled_embedding_provider_uses_custom_dimension() {
        let provider = DisabledEmbeddingProvider::new(Some(512));
        assert_eq!(provider.dimension(), 512);
    }

    #[test]
    fn split_tokens_with_overlap_handles_short_input() {
        let tokens = vec![1u32, 2, 3, 4, 5];
        let result = split_tokens_with_overlap(&tokens, 10, 1);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn split_tokens_with_overlap_handles_long_input() {
        let tokens: Vec<u32> = (0..20).collect();
        let result = split_tokens_with_overlap(&tokens, 8, 2);
        assert!(result.len() > 1);
    }

    #[test]
    fn split_tokens_with_overlap_respects_exact_chunks() {
        let tokens: Vec<u32> = (0..15).collect();
        let result = split_tokens_with_overlap(&tokens, 5, 1);
        assert!(result.len() > 1);
    }

    #[test]
    fn mean_pool_embeddings_handles_single_embedding() {
        let emb = vec![1.0, 2.0, 3.0];
        let result = mean_pool_embeddings(&[emb]).unwrap();
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn mean_pool_embeddings_normalizes_result() {
        let emb1 = vec![3.0, 4.0];
        let emb2 = vec![3.0, 4.0];
        let result = mean_pool_embeddings(&[emb1, emb2]).unwrap();
        let norm: f64 = result.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 0.001);
    }

    #[test]
    fn mean_pool_embeddings_rejects_mismatched_dimensions() {
        let emb1 = vec![1.0, 2.0];
        let emb2 = vec![1.0, 2.0, 3.0];
        let result = mean_pool_embeddings(&[emb1, emb2]);
        assert!(result.is_err());
    }

    #[test]
    fn local_candle_provider_rejects_wrong_dimension() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_tokenizer(&dir.path().join("tokenizer.json"));
        write_minimal_bert_config(&dir.path().join("config.json"));
        write_minimal_prefixed_bert_weights(&dir.path().join("model.safetensors"));

        // Model outputs TEST_HIDDEN_SIZE (2), but we claim 999
        let result = LocalCandleEmbeddingProvider::new("test/model", 999, 384, dir.path());

        match result {
            Ok(_) => panic!(
                "expected dimension mismatch error when configured dimension != model output"
            ),
            Err(e) => {
                let msg = e.to_string();
                assert!(
                    msg.contains("dimension"),
                    "error should mention dimension mismatch, got: {msg}"
                );
            }
        }
    }

    #[test]
    fn local_candle_provider_accepts_correct_dimension() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_minimal_tokenizer(&dir.path().join("tokenizer.json"));
        write_minimal_bert_config(&dir.path().join("config.json"));
        write_minimal_prefixed_bert_weights(&dir.path().join("model.safetensors"));

        // Model outputs TEST_HIDDEN_SIZE (2), and we configure 2
        let result =
            LocalCandleEmbeddingProvider::new("test/model", TEST_HIDDEN_SIZE, 384, dir.path());

        assert!(
            result.is_ok(),
            "expected provider to load successfully when dimension matches model output"
        );
    }
}
