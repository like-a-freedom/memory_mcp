use std::collections::HashMap;
use std::error::Error as StdError;
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

/// Wall-clock budget for the startup dimension probe against a remote
/// provider. The probe runs on the critical startup path, so it must fail
/// fast (single attempt, no backoff loop) and let the server degrade to
/// lexical/graph-only retrieval instead of stalling for the full runtime
/// retry budget when the provider is unreachable.
pub(crate) const REMOTE_EMBEDDING_PROBE_TIMEOUT_SECS: u64 = 10;
const MAX_REMOTE_ERROR_BODY_BYTES: usize = 4096;
const REDACTED_REMOTE_ERROR_VALUE: &str = "[REDACTED]";

fn is_sensitive_remote_error_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "api_key",
        "apikey",
        "authorization",
        "input",
        "password",
        "prompt",
        "secret",
        "token",
    ]
    .iter()
    .any(|sensitive| key.contains(sensitive))
}

fn redact_remote_error_json(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if is_sensitive_remote_error_key(key) {
                    *value = json!(REDACTED_REMOTE_ERROR_VALUE);
                } else {
                    redact_remote_error_json(value);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                redact_remote_error_json(item);
            }
        }
        _ => {}
    }
}

fn sanitize_remote_error_payload(payload: &[u8]) -> String {
    let raw = String::from_utf8_lossy(payload).trim().to_string();
    if raw.is_empty() {
        return "<empty>".to_string();
    }

    match serde_json::from_str::<Value>(&raw) {
        Ok(mut value) => {
            redact_remote_error_json(&mut value);
            match serde_json::to_string(&value) {
                Ok(serialized) => serialized,
                Err(_) => "<invalid-json-response>".to_string(),
            }
        }
        Err(_) => raw.replace('\n', "\\n").replace('\r', "\\r"),
    }
}

async fn read_remote_error_payload(response: &mut reqwest::Response) -> String {
    let mut payload = Vec::new();
    let mut truncated = false;

    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = MAX_REMOTE_ERROR_BODY_BYTES.saturating_sub(payload.len());
                if remaining == 0 {
                    truncated = true;
                    break;
                }
                if chunk.len() > remaining {
                    payload.extend_from_slice(&chunk[..remaining]);
                    truncated = true;
                    break;
                }
                payload.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => {
                return format!("<response body read failed: {error}>");
            }
        }
    }

    let mut sanitized = sanitize_remote_error_payload(&payload);
    if truncated {
        sanitized.push_str("…<truncated>");
    }
    sanitized
}

pub(super) fn redact_endpoint_for_log(endpoint: &str) -> String {
    let endpoint = endpoint
        .split_once('#')
        .map_or(endpoint, |(without_fragment, _)| without_fragment);
    let endpoint = endpoint
        .split_once('?')
        .map_or(endpoint, |(without_query, _)| without_query);

    let Some(scheme_end) = endpoint.find("://") else {
        return endpoint.to_string();
    };
    let authority_start = scheme_end + 3;
    let path_start = endpoint[authority_start..]
        .find('/')
        .map_or(endpoint.len(), |offset| authority_start + offset);
    let authority = &endpoint[authority_start..path_start];
    let Some(credentials_end) = authority.rfind('@') else {
        return endpoint.to_string();
    };

    format!(
        "{}***@{}",
        &endpoint[..authority_start],
        &endpoint[authority_start + credentials_end + 1..]
    )
}

fn remote_error_context(endpoint: &str, model: &str, response_payload: &str) -> String {
    format!(
        "endpoint={}; model={model}; response_payload={response_payload}",
        redact_endpoint_for_log(endpoint)
    )
}

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

fn log_remote_retry(
    provider: &str,
    attempt: u32,
    max_attempts: u32,
    delay: Duration,
    message: &str,
) {
    let mut event = HashMap::new();
    event.insert("op".to_string(), json!("embedding.request.retry"));
    event.insert("provider".to_string(), json!(provider));
    event.insert("attempt".to_string(), json!(attempt));
    event.insert("max_attempts".to_string(), json!(max_attempts));
    event.insert("delay_ms".to_string(), json!(delay.as_millis() as u64));
    event.insert("reason".to_string(), json!(message));
    super::embedding_logger().log(event, LogLevel::Warn);
}

async fn with_remote_embedding_retry<T, F, Fut>(
    provider: &'static str,
    max_attempts: u32,
    mut operation: F,
) -> Result<T, MemoryError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, RemoteRequestFailure>>,
{
    for attempt in 1..=max_attempts {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(RemoteRequestFailure::Retryable(failure)) if attempt < max_attempts => {
                let delay = remote_retry_delay(attempt, failure.retry_after);
                log_remote_retry(provider, attempt, max_attempts, delay, &failure.message);
                tokio::time::sleep(delay).await;
            }
            Err(RemoteRequestFailure::Retryable(failure)) => {
                return Err(MemoryError::Transient(format!(
                    "{} after {} attempts",
                    failure.message, max_attempts
                )));
            }
            Err(RemoteRequestFailure::Fatal(err)) => return Err(err),
        }
    }

    Err(MemoryError::Transient(
        "remote embedding retry loop exhausted without a response".to_string(),
    ))
}

fn map_send_error(err: reqwest::Error) -> RemoteRequestFailure {
    let classification = classify_send_error(&err);
    if classification.is_retryable_transport() {
        return RemoteRequestFailure::Retryable(RetryableRemoteRequestFailure {
            message: format!("embedding request transport failure: {classification}"),
            retry_after: None,
        });
    }

    RemoteRequestFailure::Fatal(MemoryError::Storage(format!(
        "embedding request failed: {classification}"
    )))
}

/// Classifies a `reqwest::Error` and produces a structured message that
/// distinguishes the failure category (timeout, connect, dns, request,
/// body, decode, redirect, status, builder) and includes the URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendErrorCategory {
    Timeout,
    Connect,
    Dns,
    Request,
    Body,
    Decode,
    Redirect,
    Builder,
    Other,
}

impl SendErrorCategory {
    fn label(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect => "connect",
            Self::Dns => "dns",
            Self::Request => "request",
            Self::Body => "body",
            Self::Decode => "decode",
            Self::Redirect => "redirect",
            Self::Builder => "builder",
            Self::Other => "transport",
        }
    }

    fn is_retryable_transport(self) -> bool {
        matches!(
            self,
            Self::Timeout | Self::Connect | Self::Dns | Self::Request | Self::Body
        )
    }
}

struct ClassifiedSendError {
    category: SendErrorCategory,
    url: Option<String>,
    source: Option<String>,
    display: String,
}

impl ClassifiedSendError {
    fn is_retryable_transport(&self) -> bool {
        self.category.is_retryable_transport()
    }
}

impl std::fmt::Display for ClassifiedSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.category.label(), self.display)?;
        if let Some(url) = &self.url {
            write!(f, "; url={}", redact_endpoint_for_log(url))?;
        }
        if let Some(source) = &self.source {
            write!(f, "; source={source}")?;
        }
        Ok(())
    }
}

fn classify_send_error(err: &reqwest::Error) -> ClassifiedSendError {
    let category = if err.is_timeout() {
        SendErrorCategory::Timeout
    } else if err.is_connect() {
        // `is_connect` covers TCP-level failures including DNS resolution
        // errors reported as `ConnectError` on most platforms. We
        // distinguish the "host cannot be resolved" case by inspecting
        // the chain for a resolver error.
        if error_chain_contains(err, "resolve") || error_chain_contains(err, "dns") {
            SendErrorCategory::Dns
        } else {
            SendErrorCategory::Connect
        }
    } else if err.is_request() {
        SendErrorCategory::Request
    } else if err.is_body() {
        SendErrorCategory::Body
    } else if err.is_decode() {
        SendErrorCategory::Decode
    } else if err.is_redirect() {
        SendErrorCategory::Redirect
    } else if err.is_builder() {
        SendErrorCategory::Builder
    } else {
        SendErrorCategory::Other
    };

    let url = err.url().map(|u| u.to_string());
    let source = err.source().map(|s| s.to_string());
    let display = err.to_string();
    ClassifiedSendError {
        category,
        url,
        source,
        display,
    }
}

fn error_chain_contains(err: &reqwest::Error, needle: &str) -> bool {
    let mut current: Option<&dyn std::error::Error> = Some(err);
    while let Some(e) = current {
        if e.to_string()
            .to_lowercase()
            .contains(&needle.to_lowercase())
        {
            return true;
        }
        current = e.source();
    }
    false
}

async fn response_json(
    mut response: reqwest::Response,
    endpoint: &str,
    model: &str,
) -> Result<Value, RemoteRequestFailure> {
    let status = response.status();
    if !status.is_success() {
        let retry_after = retry_after_duration(response.headers());
        let status_code = status.as_u16();
        let response_payload = read_remote_error_payload(&mut response).await;
        let context = remote_error_context(endpoint, model, &response_payload);
        if is_retryable_status(status) {
            return Err(RemoteRequestFailure::Retryable(
                RetryableRemoteRequestFailure {
                    message: format!(
                        "embedding request returned retryable status {status_code}; {context}"
                    ),
                    retry_after,
                },
            ));
        }

        return Err(RemoteRequestFailure::Fatal(MemoryError::Storage(format!(
            "embedding request returned error status {status_code}; {context}"
        ))));
    }

    response.json::<Value>().await.map_err(|err| {
        RemoteRequestFailure::Fatal(MemoryError::Storage(format!(
            "embedding response decode failed; endpoint={}; model={model}; error={err}",
            redact_endpoint_for_log(endpoint)
        )))
    })
}

/// Fetches and parses an OpenAI-compatible embedding, retrying on both
/// transport errors and transient data errors (e.g. empty `data[0].embedding`
/// from overloaded or rate-limited providers).
async fn request_openai_embedding_and_parse(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    input: &str,
) -> Result<Vec<f64>, MemoryError> {
    let mut headers =
        HeaderMap::from_iter([(CONTENT_TYPE, HeaderValue::from_static("application/json"))]);
    if let Some(api_key) = api_key {
        let value = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
            MemoryError::ConfigInvalid(format!("invalid EMBEDDINGS_API_KEY header: {err}"))
        })?;
        headers.insert(AUTHORIZATION, value);
    }

    let endpoint = format!("{}/embeddings", base_url.trim_end_matches('/'));
    with_remote_embedding_retry(
        "openai-compatible",
        MAX_REMOTE_EMBEDDING_ATTEMPTS,
        || async {
            let body = {
                let response = client
                    .post(&endpoint)
                    .headers(headers.clone())
                    .json(&json!({"model": model, "input": input}))
                    .send()
                    .await
                    .map_err(map_send_error)?;
                response_json(response, &endpoint, model).await?
            };

            // Parsing failure (missing data) is retryable — providers may
            // return empty payloads under load.
            match parse_openai_embedding(&body) {
                Ok(embedding) => Ok(embedding),
                Err(_) => Err(RemoteRequestFailure::Retryable(
                    RetryableRemoteRequestFailure {
                        message: format!(
                            "embedding response missing data[0].embedding; {}",
                            remote_error_context(
                                &endpoint,
                                model,
                                "<successful response with unexpected schema>"
                            )
                        ),
                        retry_after: None,
                    },
                )),
            }
        },
    )
    .await
}

async fn request_ollama_body(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    input: &str,
) -> Result<Value, MemoryError> {
    let endpoint = format!("{}/api/embeddings", base_url.trim_end_matches('/'));
    with_remote_embedding_retry("ollama", MAX_REMOTE_EMBEDDING_ATTEMPTS, || async {
        let response = client
            .post(&endpoint)
            .json(&json!({"model": model, "prompt": input}))
            .send()
            .await
            .map_err(map_send_error)?;
        response_json(response, &endpoint, model).await
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

/// One-shot OpenAI-compatible embedding request without the runtime retry
/// loop. Used by the startup dimension probe, which must fail fast when the
/// provider is unreachable instead of blocking server startup.
async fn request_openai_embedding_once(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
    input: &str,
) -> Result<Vec<f64>, MemoryError> {
    let mut headers =
        HeaderMap::from_iter([(CONTENT_TYPE, HeaderValue::from_static("application/json"))]);
    if let Some(api_key) = api_key {
        let value = HeaderValue::from_str(&format!("Bearer {api_key}")).map_err(|err| {
            MemoryError::ConfigInvalid(format!("invalid EMBEDDINGS_API_KEY header: {err}"))
        })?;
        headers.insert(AUTHORIZATION, value);
    }

    let endpoint = format!("{}/embeddings", base_url.trim_end_matches('/'));
    let response = client
        .post(&endpoint)
        .headers(headers)
        .json(&json!({"model": model, "input": input}))
        .send()
        .await
        .map_err(|err| {
            let classification = classify_send_error(&err);
            MemoryError::Transient(format!(
                "embedding probe transport failure: {classification}"
            ))
        })?;
    let body =
        response_json(response, &endpoint, model)
            .await
            .map_err(|failure| match failure {
                RemoteRequestFailure::Retryable(retryable) => {
                    MemoryError::Transient(retryable.message)
                }
                RemoteRequestFailure::Fatal(err) => err,
            })?;

    parse_openai_embedding(&body)
}

/// One-shot Ollama embedding request without the runtime retry loop. Used
/// by the startup dimension probe, which must fail fast when the provider is
/// unreachable instead of blocking server startup.
async fn request_ollama_body_once(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    input: &str,
) -> Result<Value, MemoryError> {
    let endpoint = format!("{}/api/embeddings", base_url.trim_end_matches('/'));
    let response = client
        .post(&endpoint)
        .json(&json!({"model": model, "prompt": input}))
        .send()
        .await
        .map_err(|err| {
            let classification = classify_send_error(&err);
            MemoryError::Transient(format!(
                "embedding probe transport failure: {classification}"
            ))
        })?;

    response_json(response, &endpoint, model)
        .await
        .map_err(|failure| match failure {
            RemoteRequestFailure::Retryable(retryable) => MemoryError::Transient(retryable.message),
            RemoteRequestFailure::Fatal(err) => err,
        })
}

pub(super) async fn detect_openai_embedding_dimension(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
    api_key: Option<&str>,
) -> Result<usize, MemoryError> {
    let embedding = tokio::time::timeout(
        Duration::from_secs(REMOTE_EMBEDDING_PROBE_TIMEOUT_SECS),
        request_openai_embedding_once(client, base_url, model, api_key, "dimension probe"),
    )
    .await
    .map_err(|_| {
        MemoryError::Transient(format!(
            "embedding dimension probe timed out after {REMOTE_EMBEDDING_PROBE_TIMEOUT_SECS}s"
        ))
    })??;

    Ok(embedding.len())
}

pub(super) async fn detect_ollama_embedding_dimension(
    client: &reqwest::Client,
    base_url: &str,
    model: &str,
) -> Result<usize, MemoryError> {
    let body = tokio::time::timeout(
        Duration::from_secs(REMOTE_EMBEDDING_PROBE_TIMEOUT_SECS),
        request_ollama_body_once(client, base_url, model, "dimension probe"),
    )
    .await
    .map_err(|_| {
        MemoryError::Transient(format!(
            "embedding dimension probe timed out after {REMOTE_EMBEDDING_PROBE_TIMEOUT_SECS}s"
        ))
    })??;

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
        let embedding = request_openai_embedding_and_parse(
            &self.client,
            &self.base_url,
            &self.model,
            self.api_key.as_deref(),
            input,
        )
        .await?;

        validate_dimension(embedding, self.dimension)
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
    fn parse_openai_embedding_reads_first_vector() {
        let embedding = parse_openai_embedding(&json!({
            "data": [
                {"embedding": [0.1, 0.2, 0.3]}
            ]
        }))
        .expect("embedding");

        assert_eq!(embedding.len(), 3);
    }

    #[test]
    fn parse_ollama_embedding_reads_vector() {
        let embedding =
            parse_ollama_embedding(&json!({"embedding": [0.4, 0.5, 0.6]})).expect("embedding");

        assert_eq!(embedding.len(), 3);
    }

    #[test]
    fn validate_dimension_rejects_mismatch() {
        let error = validate_dimension(vec![0.1, 0.2], 3).expect_err("dimension mismatch");

        assert!(
            matches!(error, MemoryError::Storage(message) if message.contains("dimension mismatch"))
        );
    }

    #[test]
    fn parse_openai_embedding_missing_data_error_is_storage() {
        // Even though the error is now retried via
        // request_openai_embedding_and_parse, parse_openai_embedding
        // itself still returns Storage — the retry wrapper
        // catches it and converts to Retryable.
        let err =
            parse_openai_embedding(&json!({"data": []})).expect_err("missing data should fail");
        assert!(err.to_string().contains("missing data[0].embedding"));
    }

    #[test]
    fn parse_openai_embedding_extracts_valid_vector() {
        let embedding = parse_openai_embedding(
            &json!({"data": [{"embedding": [0.1, 0.2, 0.3]}], "model": "test"}),
        )
        .expect("valid embedding");
        assert_eq!(embedding.len(), 3);
    }

    #[test]
    fn parse_openai_embedding_returns_none_for_null_data() {
        assert!(parse_openai_embedding(&json!({"data": null})).is_err());
    }

    #[test]
    fn parse_openai_embedding_returns_none_for_wrong_shape() {
        // Plain array, not {"data": [...]}
        assert!(parse_openai_embedding(&json!([0.1, 0.2, 0.3])).is_err());
    }

    #[test]
    fn remote_endpoint_redacts_credentials_and_query_parameters() {
        assert_eq!(
            redact_endpoint_for_log("https://user:secret@example.test/v1?api_key=hidden#fragment"),
            "https://***@example.test/v1"
        );
    }

    #[test]
    fn remote_error_payload_redacts_sensitive_json_fields() {
        let payload = sanitize_remote_error_payload(
            br#"{"error":{"message":"route not found"},"api_key":"secret","input":"private memory"}"#,
        );

        assert!(payload.contains("route not found"));
        assert!(payload.contains("[REDACTED]"));
        assert!(!payload.contains("secret"));
        assert!(!payload.contains("private memory"));
    }

    #[tokio::test]
    async fn remote_probe_error_includes_endpoint_model_and_response_payload() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind listener");
        let address = listener.local_addr().expect("listener address");
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept request");
            let mut request = [0u8; 4096];
            let _ = socket.read(&mut request).await.expect("read request");
            let body = br#"{"error":{"message":"route not found"}}"#;
            let response = format!(
                "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            socket
                .write_all(response.as_bytes())
                .await
                .expect("write headers");
            socket.write_all(body).await.expect("write body");
        });

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("http client");
        let base_url = format!("http://{address}/v1");
        let error = detect_openai_embedding_dimension(&client, &base_url, "test-model", None)
            .await
            .expect_err("404 probe should fail");
        let message = error.to_string();

        assert!(message.contains("status 404"));
        assert!(message.contains("/v1/embeddings"));
        assert!(message.contains("model=test-model"));
        assert!(message.contains("route not found"));
        server.await.expect("server task");
    }

    /// The probe must classify the transport error category and include the
    /// endpoint URL in the message, so an operator can distinguish a
    /// connection refused, request timeout, or DNS failure.
    #[tokio::test]
    async fn remote_probe_transport_error_classifies_category_and_endpoint() {
        // Bind and immediately drop so the port is closed; any connection
        // attempt will receive ECONNREFUSED.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("addr");
        drop(listener);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("http client");
        let base_url = format!("http://{address}/v1");
        let error = detect_openai_embedding_dimension(&client, &base_url, "test-model", None)
            .await
            .expect_err("closed port must fail the probe");
        let message = error.to_string();

        // The probe must distinguish "connection refused" from "timeout"
        // and "DNS" — the operator needs that to diagnose the failure.
        assert!(
            message.contains("connect")
                || message.contains("refused")
                || message.contains("timeout"),
            "transport probe error must classify the failure category, got: {message}"
        );
        // The redacted endpoint URL must appear in the message.
        assert!(
            message.contains(&format!("{}/v1/embeddings", address))
                || message.contains(&address.to_string()),
            "transport probe error must include the redacted endpoint URL, got: {message}"
        );
    }

    /// The probe must include the endpoint URL even when the host cannot be
    /// resolved (DNS failure), so the operator can spot a misconfiguration.
    #[tokio::test]
    async fn remote_probe_transport_error_surfaces_dns_failure_with_endpoint() {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .expect("http client");
        let base_url = "http://this-host-does-not-exist.invalid./v1";
        let error = detect_openai_embedding_dimension(&client, base_url, "test-model", None)
            .await
            .expect_err("DNS failure must fail the probe");
        let message = error.to_string();

        assert!(
            message.contains("dns") || message.contains("resolve") || message.contains("connect"),
            "DNS failure must classify the transport error, got: {message}"
        );
        assert!(
            message.contains("/v1/embeddings") || message.contains("invalid"),
            "DNS failure must include the endpoint URL, got: {message}"
        );
    }
}
