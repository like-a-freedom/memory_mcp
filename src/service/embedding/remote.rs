use std::collections::HashMap;
use std::future::Future;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue, RETRY_AFTER};
use serde_json::{Value, json};

use super::{EmbeddingProvider, MemoryError, embedding_from_value};
use crate::logging::LogLevel;

const MAX_REMOTE_EMBEDDING_ATTEMPTS: u32 = 5;
const BASE_REMOTE_EMBEDDING_DELAY_MS: u64 = 2000;
const MAX_REMOTE_EMBEDDING_DELAY_MS: u64 = 15_000;

#[derive(Debug, Clone)]
struct RetryableRemoteRequestFailure {
    message: String,
    retry_after: Option<Duration>,
}

enum RemoteRequestFailure {
    Retryable(RetryableRemoteRequestFailure),
    Fatal(MemoryError),
}

fn is_retryable_status(status: StatusCode) -> bool {
    matches!(status.as_u16(), 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn retry_after_duration(headers: &HeaderMap) -> Option<Duration> {
    headers
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn remote_retry_delay(attempt: u32, retry_after: Option<Duration>) -> Duration {
    retry_after.unwrap_or_else(|| {
        Duration::from_millis(
            BASE_REMOTE_EMBEDDING_DELAY_MS
                .saturating_mul(1u64 << attempt.saturating_sub(1).min(6))
                .min(MAX_REMOTE_EMBEDDING_DELAY_MS),
        )
    })
}

fn log_remote_retry(provider: &str, attempt: u32, delay: Duration, message: &str) {
    let mut event = HashMap::new();
    event.insert("op".to_string(), json!("embedding.request.retry"));
    event.insert("provider".to_string(), json!(provider));
    event.insert("attempt".to_string(), json!(attempt));
    event.insert(
        "max_attempts".to_string(),
        json!(MAX_REMOTE_EMBEDDING_ATTEMPTS),
    );
    event.insert("delay_ms".to_string(), json!(delay.as_millis() as u64));
    event.insert("reason".to_string(), json!(message));
    super::embedding_logger().log(event, LogLevel::Warn);
}

async fn with_remote_embedding_retry<T, F, Fut>(
    provider: &'static str,
    mut operation: F,
) -> Result<T, MemoryError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, RemoteRequestFailure>>,
{
    for attempt in 1..=MAX_REMOTE_EMBEDDING_ATTEMPTS {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(RemoteRequestFailure::Retryable(failure))
                if attempt < MAX_REMOTE_EMBEDDING_ATTEMPTS =>
            {
                let delay = remote_retry_delay(attempt, failure.retry_after);
                log_remote_retry(provider, attempt, delay, &failure.message);
                tokio::time::sleep(delay).await;
            }
            Err(RemoteRequestFailure::Retryable(failure)) => {
                return Err(MemoryError::Transient(format!(
                    "{} after {} attempts",
                    failure.message, MAX_REMOTE_EMBEDDING_ATTEMPTS
                )));
            }
            Err(RemoteRequestFailure::Fatal(err)) => return Err(err),
        }
    }

    unreachable!("remote embedding retry loop should always return")
}

fn map_send_error(err: reqwest::Error) -> RemoteRequestFailure {
    if err.is_timeout() || err.is_connect() || err.is_request() || err.is_body() {
        return RemoteRequestFailure::Retryable(RetryableRemoteRequestFailure {
            message: format!("embedding request transport failure: {err}"),
            retry_after: None,
        });
    }

    RemoteRequestFailure::Fatal(MemoryError::Storage(format!(
        "embedding request failed: {err}"
    )))
}

async fn response_json(response: reqwest::Response) -> Result<Value, RemoteRequestFailure> {
    let status = response.status();
    if !status.is_success() {
        let retry_after = retry_after_duration(response.headers());
        let status_code = status.as_u16();
        if is_retryable_status(status) {
            return Err(RemoteRequestFailure::Retryable(
                RetryableRemoteRequestFailure {
                    message: format!("embedding request returned retryable status {status_code}"),
                    retry_after,
                },
            ));
        }

        return Err(RemoteRequestFailure::Fatal(MemoryError::Storage(format!(
            "embedding request returned error status {status_code}"
        ))));
    }

    response.json::<Value>().await.map_err(|err| {
        RemoteRequestFailure::Fatal(MemoryError::Storage(format!(
            "embedding response decode failed: {err}"
        )))
    })
}

async fn request_openai_body(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    input: &str,
) -> Result<Value, MemoryError> {
    let mut headers =
        HeaderMap::from_iter([(CONTENT_TYPE, HeaderValue::from_static("application/json"))]);
    if let Some(api_key) = api_key {
        let value = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
            MemoryError::ConfigInvalid(format!("invalid EMBEDDINGS_API_KEY header: {err}"))
        })?;
        headers.insert(AUTHORIZATION, value);
    }

    with_remote_embedding_retry("openai-compatible", || async {
        let response = client
            .post(format!("{}/embeddings", base_url.trim_end_matches('/')))
            .headers(headers.clone())
            .json(&json!({"model": model, "input": input}))
            .send()
            .await
            .map_err(map_send_error)?;
        response_json(response).await
    })
    .await
}

async fn request_ollama_body(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    input: &str,
) -> Result<Value, MemoryError> {
    with_remote_embedding_retry("ollama", || async {
        let response = client
            .post(format!("{}/api/embeddings", base_url.trim_end_matches('/')))
            .json(&json!({"model": model, "prompt": input}))
            .send()
            .await
            .map_err(map_send_error)?;
        response_json(response).await
    })
    .await
}

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

pub(super) async fn detect_openai_embedding_dimension(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<usize, MemoryError> {
    let body = request_openai_body(client, base_url, model, api_key, "dimension probe").await?;

    Ok(parse_openai_embedding(&body)?.len())
}

pub(super) async fn detect_ollama_embedding_dimension(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
) -> Result<usize, MemoryError> {
    let body = request_ollama_body(client, base_url, model, "dimension probe").await?;

    Ok(parse_ollama_embedding(&body)?.len())
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
        let body = request_openai_body(
            &self.client,
            &self.base_url,
            &self.model,
            self.api_key.as_deref(),
            input,
        )
        .await?;

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
        let body = request_ollama_body(&self.client, &self.base_url, &self.model, input).await?;

        parse_ollama_embedding_response(&body, self.dimension)
    }
}

fn parse_openai_embedding_response(
    body: &Value,
    expected_dimension: usize,
) -> Result<Vec<f64>, MemoryError> {
    let embedding = parse_openai_embedding(body)?;

    validate_dimension(embedding, expected_dimension)
}

fn parse_openai_embedding(body: &Value) -> Result<Vec<f64>, MemoryError> {
    body.get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(|item| item.get("embedding"))
        .and_then(embedding_from_value)
        .ok_or_else(|| {
            MemoryError::Storage("embedding response missing data[0].embedding".to_string())
        })
}

fn parse_ollama_embedding_response(
    body: &Value,
    expected_dimension: usize,
) -> Result<Vec<f64>, MemoryError> {
    let embedding = parse_ollama_embedding(body)?;

    validate_dimension(embedding, expected_dimension)
}

fn parse_ollama_embedding(body: &Value) -> Result<Vec<f64>, MemoryError> {
    body.get("embedding")
        .and_then(embedding_from_value)
        .ok_or_else(|| {
            MemoryError::Storage("embedding response missing embedding array".to_string())
        })
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
    use std::time::Duration;

    use reqwest::StatusCode;
    use serde_json::json;

    use super::*;

    #[test]
    fn retryable_status_recognizes_rate_limits_and_provider_outages() {
        assert!(is_retryable_status(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(StatusCode::SERVICE_UNAVAILABLE));
        assert!(is_retryable_status(StatusCode::GATEWAY_TIMEOUT));
        assert!(!is_retryable_status(StatusCode::BAD_REQUEST));
        assert!(!is_retryable_status(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn remote_retry_delay_prefers_retry_after_header() {
        assert_eq!(
            remote_retry_delay(2, Some(Duration::from_secs(7))),
            Duration::from_secs(7)
        );
    }

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
