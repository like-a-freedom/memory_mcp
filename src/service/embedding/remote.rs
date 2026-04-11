use async_trait::async_trait;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};

use super::{EmbeddingProvider, MemoryError, embedding_from_value};

pub(super) struct OpenAiCompatibleEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
    dimension: usize,
}

impl OpenAiCompatibleEmbeddingProvider {
    pub(super) fn new(
        client: reqwest::Client,
        base_url: String,
        model: String,
        api_key: Option<String>,
        dimension: usize,
    ) -> Self {
        Self {
            client,
            base_url,
            model,
            api_key,
            dimension,
        }
    }
}

pub(super) struct OllamaEmbeddingProvider {
    client: reqwest::Client,
    base_url: String,
    model: String,
    dimension: usize,
}

impl OllamaEmbeddingProvider {
    pub(super) fn new(
        client: reqwest::Client,
        base_url: String,
        model: String,
        dimension: usize,
    ) -> Self {
        Self {
            client,
            base_url,
            model,
            dimension,
        }
    }
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
    use serde_json::json;

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
