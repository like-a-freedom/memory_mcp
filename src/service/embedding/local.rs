use std::sync::Arc;

use async_trait::async_trait;
use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;

use super::{EmbeddingProvider, MemoryError};

#[derive(Clone)]
pub(super) struct LocalCandleEmbeddingProvider {
    _model_name: String,
    dimension: usize,
    max_tokens: usize,
    tokenizer: Arc<tokenizers::Tokenizer>,
    bert_model: Arc<candle_transformers::models::bert::BertModel>,
    device: Device,
}

impl LocalCandleEmbeddingProvider {
    /// Creates a new local Candle provider.
    pub(super) fn new(
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

        let bert_model = candle_transformers::models::bert::BertModel::load(vb, &bert_config)
            .map_err(|e| MemoryError::Storage(format!("failed to build bert model: {e}")))?;

        Ok(Self {
            _model_name: model_name.to_string(),
            dimension,
            max_tokens,
            tokenizer: Arc::new(tokenizer),
            bert_model: Arc::new(bert_model),
            device,
        })
    }

    fn embed_inner(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
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

        let mask = attention_mask
            .unsqueeze(2)
            .map_err(|e| MemoryError::Storage(format!("unsqueeze failed: {e}")))?
            .to_dtype(outputs.dtype())
            .map_err(|e| MemoryError::Storage(format!("dtype conversion failed: {e}")))?;

        let masked = outputs
            .broadcast_mul(&mask)
            .map_err(|e| MemoryError::Storage(format!("broadcast_mul failed: {e}")))?
            .sum(1)
            .map_err(|e| MemoryError::Storage(format!("sum failed: {e}")))?;

        let mask_sum = mask
            .sum(1)
            .map_err(|e| MemoryError::Storage(format!("mask sum failed: {e}")))?;

        let pooled = masked
            .broadcast_div(&mask_sum)
            .map_err(|e| MemoryError::Storage(format!("broadcast_div failed: {e}")))?;

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

        let vec_f32 = normalized
            .squeeze(0)
            .map_err(|e| MemoryError::Storage(format!("squeeze failed: {e}")))?
            .to_vec1::<f32>()
            .map_err(|e| MemoryError::Storage(format!("to_vec1 failed: {e}")))?;

        Ok(vec_f32.into_iter().map(f64::from).collect())
    }

    fn embed_sync(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
        const E5_PREFIX_TOKENS: usize = 2;

        let encoding = self
            .tokenizer
            .encode(input, true)
            .map_err(|e| MemoryError::Storage(format!("tokenization failed: {e}")))?;

        if encoding.get_ids().len() + E5_PREFIX_TOKENS <= self.max_tokens {
            return self.embed_inner(input);
        }

        let chunk_size = ((self.max_tokens as f64) * 0.8) as usize;
        if chunk_size == 0 {
            return self.embed_inner(input);
        }
        let overlap = (((chunk_size as f64) * 0.1) as usize).min(chunk_size.saturating_sub(1));
        let token_ids = encoding.get_ids().to_vec();
        let chunks = split_tokens_with_overlap(&token_ids, chunk_size, overlap);

        let mut embeddings = Vec::with_capacity(chunks.len());
        for chunk_ids in chunks {
            let chunk_text = self
                .tokenizer
                .decode(&chunk_ids, true)
                .map_err(|e| MemoryError::Storage(format!("decode failed: {e}")))?;
            embeddings.push(self.embed_inner(&chunk_text)?);
        }

        mean_pool_embeddings(&embeddings)
    }
}

fn split_tokens_with_overlap(tokens: &[u32], chunk_size: usize, overlap: usize) -> Vec<Vec<u32>> {
    if tokens.len() <= chunk_size || chunk_size == 0 {
        return vec![tokens.to_vec()];
    }

    let step = chunk_size.saturating_sub(overlap).max(1);
    let mut chunks = Vec::new();
    let mut start = 0;

    while start < tokens.len() {
        let end = (start + chunk_size).min(tokens.len());
        chunks.push(tokens[start..end].to_vec());
        if end == tokens.len() {
            break;
        }
        start += step;
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

    let dimension = embeddings[0].len();
    let mut pooled = vec![0.0; dimension];

    for embedding in embeddings {
        if embedding.len() != dimension {
            return Err(MemoryError::Storage(format!(
                "embedding dimension mismatch: expected {}, got {}",
                dimension,
                embedding.len()
            )));
        }

        for (index, value) in embedding.iter().enumerate() {
            pooled[index] += value;
        }
    }

    let count = embeddings.len() as f64;
    for value in &mut pooled {
        *value /= count;
    }

    let norm = pooled.iter().map(|x| x * x).sum::<f64>().sqrt();
    if norm > 0.0 {
        for value in &mut pooled {
            *value /= norm;
        }
    }

    Ok(pooled)
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;

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
}
