//! Embedding provider abstractions and implementations.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use super::value_helpers::json_f64;
use crate::config::{EmbeddingConfig, EmbeddingProviderKind, build_embedding_signature};
use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;

mod local;
mod remote;

use local::LocalCandleEmbeddingProvider;
use remote::{
    OllamaEmbeddingProvider, OpenAiCompatibleEmbeddingProvider, detect_ollama_embedding_dimension,
    detect_openai_embedding_dimension,
};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedEmbeddingTarget {
    pub(crate) provider_label: &'static str,
    pub(crate) model: Option<String>,
    pub(crate) dimension: usize,
    pub(crate) signature: String,
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

fn build_embedding_http_client(timeout_secs: u64) -> Result<reqwest::Client, MemoryError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|err| MemoryError::ConfigInvalid(format!("invalid embedding HTTP client: {err}")))
}

fn validate_dimension_override(
    dimension_override: Option<usize>,
    actual_dimension: usize,
) -> Result<usize, MemoryError> {
    match dimension_override {
        Some(expected_dimension) if expected_dimension != actual_dimension => {
            Err(MemoryError::ConfigInvalid(format!(
                "embedding dimension mismatch: expected {expected_dimension}, got {actual_dimension}"
            )))
        }
        Some(expected_dimension) => Ok(expected_dimension),
        None => Ok(actual_dimension),
    }
}

async fn resolve_embedding_dimension(
    config: &EmbeddingConfig,
    data_dir: &str,
) -> Result<usize, MemoryError> {
    match config.provider {
        EmbeddingProviderKind::Disabled => Ok(config.fallback_dimension()),
        EmbeddingProviderKind::LocalCandle => {
            let model_dir_str = config.model_dir_or_default(data_dir);
            let actual_dimension =
                local::detect_embedding_dimension(std::path::Path::new(&model_dir_str))?;
            validate_dimension_override(config.dimension_override, actual_dimension)
        }
        EmbeddingProviderKind::OpenAiCompatible => {
            let client = build_embedding_http_client(config.timeout_secs)?;
            let actual_dimension =
                detect_openai_embedding_dimension(
                    &client,
                    config.base_url.as_deref().ok_or_else(|| {
                        MemoryError::ConfigMissing("EMBEDDINGS_BASE_URL".to_string())
                    })?,
                    config.model.as_deref().ok_or_else(|| {
                        MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string())
                    })?,
                    config.api_key.as_deref(),
                )
                .await?;
            validate_dimension_override(config.dimension_override, actual_dimension)
        }
        EmbeddingProviderKind::Ollama => {
            let client = build_embedding_http_client(config.timeout_secs)?;
            let actual_dimension =
                detect_ollama_embedding_dimension(
                    &client,
                    config.base_url.as_deref().ok_or_else(|| {
                        MemoryError::ConfigMissing("EMBEDDINGS_BASE_URL".to_string())
                    })?,
                    config.model.as_deref().ok_or_else(|| {
                        MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string())
                    })?,
                )
                .await?;
            validate_dimension_override(config.dimension_override, actual_dimension)
        }
    }
}

pub(crate) async fn resolve_embedding_target_identity(
    config: &EmbeddingConfig,
    data_dir: &str,
) -> Result<ResolvedEmbeddingTarget, MemoryError> {
    let dimension = resolve_embedding_dimension(config, data_dir).await?;
    let signature = build_embedding_signature(
        config.provider_label(),
        config.model.as_deref(),
        config.base_url.as_deref(),
        dimension,
    );

    Ok(ResolvedEmbeddingTarget {
        provider_label: config.provider_label(),
        model: config.model.clone(),
        dimension,
        signature,
    })
}

#[cfg(test)]
pub(crate) async fn resolve_dimension_override_or_probe(
    dimension_override: Option<usize>,
    provider: &dyn EmbeddingProvider,
) -> Result<usize, MemoryError> {
    let actual_dimension = provider.embed("dimension probe").await?.len();
    validate_dimension_override(dimension_override, actual_dimension)
}

pub(crate) async fn create_embedding_provider_with_dimension(
    config: &EmbeddingConfig,
    data_dir: &str,
    resolved_dimension: usize,
) -> Result<Arc<dyn EmbeddingProvider>, MemoryError> {
    let client = build_embedding_http_client(config.timeout_secs)?;

    match config.provider {
        EmbeddingProviderKind::Disabled => {
            Ok(Arc::new(DisabledEmbeddingProvider::new(resolved_dimension))
                as Arc<dyn EmbeddingProvider>)
        }
        EmbeddingProviderKind::LocalCandle => {
            let model_dir_str = config.model_dir_or_default(data_dir);
            let model_dir = std::path::Path::new(&model_dir_str);
            let model_name = config
                .model
                .as_deref()
                .unwrap_or("intfloat/multilingual-e5-small");
            let logger = crate::logging::StdoutLogger::new("info");
            let resolved_dir =
                crate::service::model_loader::ensure_model_cached(model_name, model_dir, &logger)
                    .await
                    .map_err(|err| {
                        MemoryError::Storage(format!(
                            "failed to download/cache model {model_name}: {err}"
                        ))
                    })?;

            Ok(Arc::new(LocalCandleEmbeddingProvider::new(
                model_name,
                resolved_dimension,
                config.max_tokens,
                &resolved_dir,
            )?) as Arc<dyn EmbeddingProvider>)
        }
        EmbeddingProviderKind::OpenAiCompatible => {
            Ok(Arc::new(OpenAiCompatibleEmbeddingProvider::new(
                client,
                config
                    .base_url
                    .clone()
                    .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_BASE_URL".to_string()))?,
                config
                    .model
                    .clone()
                    .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string()))?,
                config.api_key.clone(),
                resolved_dimension,
            )) as Arc<dyn EmbeddingProvider>)
        }
        EmbeddingProviderKind::Ollama => Ok(Arc::new(OllamaEmbeddingProvider::new(
            client,
            config
                .base_url
                .clone()
                .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_BASE_URL".to_string()))?,
            config
                .model
                .clone()
                .ok_or_else(|| MemoryError::ConfigMissing("EMBEDDINGS_MODEL".to_string()))?,
            resolved_dimension,
        )) as Arc<dyn EmbeddingProvider>),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EmbeddingProviderKind;
    use async_trait::async_trait;

    struct ProbeOnlyTestProvider {
        embedding: Vec<f64>,
    }

    impl ProbeOnlyTestProvider {
        fn new(embedding: Vec<f64>) -> Self {
            Self { embedding }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for ProbeOnlyTestProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "probe-only"
        }

        fn dimension(&self) -> usize {
            self.embedding.len()
        }

        async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
            Ok(self.embedding.clone())
        }
    }

    #[test]
    fn cosine_similarity_dimension_mismatch_returns_zero() {
        let a = vec![1.0, 2.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn cosine_similarity_single_element() {
        let a = vec![1.0];
        let b = vec![1.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 1.0);
    }

    #[test]
    fn embedding_from_value_parses_plain_array() {
        let value = json!([0.1, 0.2, 0.3]);
        let result = embedding_from_value(&value);
        assert!(result.is_some());
        let emb = result.unwrap();
        assert_eq!(emb.len(), 3);
    }

    #[test]
    fn embedding_from_value_returns_none_for_non_array() {
        let value = json!({"embedding": [0.1, 0.2]});
        assert!(embedding_from_value(&value).is_none());
    }

    #[test]
    fn embedding_from_value_handles_wrapped_numbers() {
        let value = json!([{"Number": 0.5}, {"Number": 0.5}]);
        let result = embedding_from_value(&value);
        assert!(result.is_some());
    }

    #[test]
    fn embedding_from_value_returns_none_for_invalid_element() {
        let value = json!([0.1, "not_a_number", 0.3]);
        assert!(embedding_from_value(&value).is_none());
    }

    #[test]
    fn normalize_embedding_unit_vector() {
        let v = vec![1.0, 0.0, 0.0];
        let normalized = normalize_embedding(v.clone());
        assert!((normalized[0] - 1.0).abs() < 1e-9);
        assert!((normalized[1] - 0.0).abs() < 1e-9);
    }

    #[test]
    fn normalize_embedding_zero_vector_returns_unchanged() {
        let v = vec![0.0, 0.0, 0.0];
        let normalized = normalize_embedding(v.clone());
        assert_eq!(normalized, v);
    }

    #[test]
    fn normalize_embedding_produces_unit_length() {
        let v = vec![3.0, 4.0];
        let normalized = normalize_embedding(v);
        let magnitude = normalized.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((magnitude - 1.0).abs() < 1e-9);
    }

    #[test]
    fn disabled_provider_returns_false_for_is_enabled() {
        let provider = DisabledEmbeddingProvider::new(1536);
        assert!(!provider.is_enabled());
    }

    #[test]
    fn disabled_provider_returns_correct_dimension() {
        let provider = DisabledEmbeddingProvider::new(384);
        assert_eq!(provider.dimension(), 384);
    }

    #[test]
    fn disabled_provider_embed_returns_error() {
        let provider = DisabledEmbeddingProvider::new(1536);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(provider.embed("test"));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn local_candle_detects_dimension_from_model_metadata_when_override_missing() {
        let tempdir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            tempdir.path().join("config.json"),
            serde_json::json!({
                "hidden_size": 384,
                "model_type": "bert"
            })
            .to_string(),
        )
        .expect("write config");

        let config = EmbeddingConfig {
            provider: EmbeddingProviderKind::LocalCandle,
            model: Some("intfloat/multilingual-e5-small".to_string()),
            model_dir: Some(tempdir.path().to_string_lossy().to_string()),
            dimension_override: None,
            ..EmbeddingConfig::default()
        };

        let resolved = resolve_embedding_target_identity(&config, ".")
            .await
            .expect("resolved target");
        assert_eq!(resolved.dimension, 384);
    }

    #[tokio::test]
    async fn remote_probe_detects_dimension_when_override_missing() {
        let provider = ProbeOnlyTestProvider::new(vec![0.1, 0.2, 0.3, 0.4]);
        let resolved = resolve_dimension_override_or_probe(None, &provider)
            .await
            .expect("detected dimension");

        assert_eq!(resolved, 4);
    }

    #[tokio::test]
    async fn explicit_dimension_override_mismatch_fails_fast() {
        let provider = ProbeOnlyTestProvider::new(vec![0.1, 0.2, 0.3, 0.4]);
        let err = resolve_dimension_override_or_probe(Some(3), &provider)
            .await
            .expect_err("override mismatch should fail");

        assert!(err.to_string().contains("embedding dimension mismatch"));
    }
}
