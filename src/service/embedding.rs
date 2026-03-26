use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use crate::config::{EmbeddingConfig, EmbeddingProviderKind};
use crate::service::MemoryError;
use crate::storage::json_f64;

pub(crate) const SEMANTIC_MATCH_THRESHOLD: f64 = 0.25;

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
    pub fn new(dimension: usize) -> Self {
        Self { dimension }
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

pub(crate) fn create_embedding_provider(
    config: &EmbeddingConfig,
) -> Result<Arc<dyn EmbeddingProvider>, MemoryError> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(config.timeout_secs))
        .build()
        .map_err(|err| {
            MemoryError::ConfigInvalid(format!("invalid embedding HTTP client: {err}"))
        })?;

    match config.provider {
        EmbeddingProviderKind::Disabled => {
            Ok(Arc::new(DisabledEmbeddingProvider::new(config.dimension)))
        }
        EmbeddingProviderKind::OpenAiCompatible => {
            Ok(Arc::new(OpenAiCompatibleEmbeddingProvider {
                client,
                base_url: config
                    .base_url
                    .clone()
                    .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_BASE_URL".to_string()))?,
                model: config
                    .model
                    .clone()
                    .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string()))?,
                api_key: config.api_key.clone(),
                dimension: config.dimension,
            }))
        }
        EmbeddingProviderKind::Ollama => Ok(Arc::new(OllamaEmbeddingProvider {
            client,
            base_url: config
                .base_url
                .clone()
                .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_BASE_URL".to_string()))?,
            model: config
                .model
                .clone()
                .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string()))?,
            dimension: config.dimension,
        })),
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

    let dimensions = left.len().min(right.len());
    (0..dimensions).map(|idx| left[idx] * right[idx]).sum()
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

        parse_openai_embedding_response(&body, self.dimension)
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

        parse_ollama_embedding_response(&body, self.dimension)
    }
}

fn parse_openai_embedding_response(
    body: &Value,
    expected_dimension: usize,
) -> Result<Vec<f64>, MemoryError> {
    let embedding = body
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("embedding"))
        .and_then(embedding_from_value)
        .ok_or_else(|| {
            MemoryError::Storage("embedding response missing data[0].embedding".to_string())
        })?;

    validate_dimension(embedding, expected_dimension)
}

fn parse_ollama_embedding_response(
    body: &Value,
    expected_dimension: usize,
) -> Result<Vec<f64>, MemoryError> {
    let embedding = body
        .get("embedding")
        .and_then(embedding_from_value)
        .ok_or_else(|| {
            MemoryError::Storage("embedding response missing embedding array".to_string())
        })?;

    validate_dimension(embedding, expected_dimension)
}

fn validate_dimension(
    embedding: Vec<f64>,
    expected_dimension: usize,
) -> Result<Vec<f64>, MemoryError> {
    if embedding.len() != expected_dimension {
        return Err(MemoryError::Storage(format!(
            "embedding dimension mismatch: expected {expected_dimension}, got {}",
            embedding.len()
        )));
    }

    Ok(embedding)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_openai_embedding_response_reads_first_vector() {
        let embedding = parse_openai_embedding_response(
            &json!({
                "data": [
                    {"embedding": [0.1, 0.2, 0.3]}
                ]
            }),
            3,
        )
        .expect("embedding");

        assert_eq!(
            embedding,
            vec![0.2672612419124244, 0.5345224838248488, 0.8017837257372731]
        );
    }

    #[test]
    fn parse_ollama_embedding_response_reads_vector() {
        let embedding = parse_ollama_embedding_response(&json!({"embedding": [0.4, 0.5, 0.6]}), 3)
            .expect("embedding");

        assert_eq!(
            embedding,
            vec![0.4558423058385518, 0.5698028822981898, 0.6837634587578276]
        );
    }

    #[test]
    fn validate_dimension_rejects_mismatch() {
        let error = validate_dimension(vec![0.1, 0.2], 3).expect_err("dimension mismatch");

        assert!(
            matches!(error, MemoryError::Storage(message) if message.contains("dimension mismatch"))
        );
    }
}
