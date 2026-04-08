//! Model download and caching for local embedding providers.
//!
//! Downloads model artifacts from HuggingFace Hub on first launch,
//! caches them on disk, and retries on network failures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;

/// Required model files for HuggingFace safetensors models.
pub const MODEL_REQUIRED_FILES: &[&str] = &["tokenizer.json", "config.json", "model.safetensors"];

/// Required model files for GLiNER (tokenizer is fetched separately).
#[allow(dead_code)]
pub const GLINER_MODEL_FILES: &[&str] = &["model.safetensors", "gliner_config.json"];

/// Direct URL for GLiNER tokenizer (the model repo doesn't include it).
#[allow(dead_code)]
pub const GLINER_TOKENIZER_URL: &str =
    "https://huggingface.co/juampahc/gliner_multi-v2.1-onnx/resolve/main/tokenizer.json";

/// Maximum number of download retries per file.
const MAX_RETRIES: u32 = 3;

/// Download timeout per file.
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;

/// Checks if all required model files exist in the cache directory.
#[allow(dead_code)]
pub fn is_model_cached(cache_dir: &Path) -> bool {
    is_model_cached_with_files(cache_dir, MODEL_REQUIRED_FILES)
}

/// Checks if all required model files exist in the cache directory.
pub fn is_model_cached_with_files(cache_dir: &Path, required: &[&str]) -> bool {
    required.iter().all(|f| cache_dir.join(f).is_file())
}

/// Sanitizes a model name for use in a filesystem path.
#[must_use]
#[allow(dead_code)]
pub fn sanitize_model_name(model_name: &str) -> String {
    model_name.replace('/', "--")
}

/// Ensures all model files are present in the cache directory.
pub async fn ensure_model_cached(
    repo_id: &str,
    cache_dir: &Path,
    logger: &StdoutLogger,
) -> Result<PathBuf, MemoryError> {
    ensure_model_cached_with_files(repo_id, cache_dir, MODEL_REQUIRED_FILES, logger).await
}

/// Ensures a GLiNER model is fully cached with tokenizer from a direct URL.
#[allow(dead_code)]
pub async fn ensure_gliner_model_cached(
    model_repo: &str,
    cache_dir: &Path,
    logger: &StdoutLogger,
) -> Result<PathBuf, MemoryError> {
    ensure_model_cached_with_files(model_repo, cache_dir, GLINER_MODEL_FILES, logger).await?;

    let tokenizer_path = cache_dir.join("tokenizer.json");
    if !tokenizer_path.is_file() {
        log_message(
            logger,
            LogLevel::Info,
            "Downloading GLiNER tokenizer from remote URL",
        );
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|e| MemoryError::Storage(format!("failed to build http client: {e}")))?;

        let mut last_err = None;
        for attempt in 1..=MAX_RETRIES {
            match download_file(
                &http,
                GLINER_TOKENIZER_URL,
                &tokenizer_path,
                logger,
                "tokenizer.json",
            )
            .await
            {
                Ok(bytes) => {
                    log_message(
                        logger,
                        LogLevel::Info,
                        &format!("Downloaded tokenizer.json ({bytes} bytes)"),
                    );
                    last_err = None;
                    break;
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&tokenizer_path);
                    last_err = Some(e);
                    if attempt < MAX_RETRIES {
                        let delay = Duration::from_secs(2u64.pow(attempt));
                        log_message(
                            logger,
                            LogLevel::Warn,
                            &format!(
                                "Download tokenizer.json failed (attempt {attempt}/{MAX_RETRIES}), retrying in {delay:?}"
                            ),
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        if let Some(e) = last_err {
            return Err(MemoryError::Storage(format!(
                "failed to download tokenizer.json after {MAX_RETRIES} retries: {e}"
            )));
        }
    } else {
        log_message(
            logger,
            LogLevel::Info,
            "tokenizer.json already present, skipping",
        );
    }

    Ok(cache_dir.to_path_buf())
}

/// Ensures all model files are present in the cache directory.
pub async fn ensure_model_cached_with_files(
    repo_id: &str,
    cache_dir: &Path,
    required_files: &[&str],
    logger: &StdoutLogger,
) -> Result<PathBuf, MemoryError> {
    if is_model_cached_with_files(cache_dir, required_files) {
        log_message(
            logger,
            LogLevel::Info,
            &format!("Model already cached at {}", cache_dir.display()),
        );
        return Ok(cache_dir.to_path_buf());
    }

    log_message(
        logger,
        LogLevel::Info,
        &format!("Downloading model {repo_id} to {}", cache_dir.display()),
    );

    std::fs::create_dir_all(cache_dir).map_err(|e| {
        MemoryError::Storage(format!(
            "failed to create model cache dir {}: {e}",
            cache_dir.display()
        ))
    })?;

    let api = hf_hub::api::tokio::ApiBuilder::new()
        .build()
        .map_err(|e| MemoryError::Storage(format!("failed to init hf-hub api: {e}")))?;
    let repo = api.model(repo_id.to_string());

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| MemoryError::Storage(format!("failed to build http client: {e}")))?;

    for file_name in required_files {
        let target_path = cache_dir.join(file_name);
        if target_path.is_file() {
            log_message(
                logger,
                LogLevel::Info,
                &format!("{file_name} already present, skipping"),
            );
            continue;
        }

        let url = repo.url(file_name);
        let mut last_err = None;
        for attempt in 1..=MAX_RETRIES {
            match download_file(&http, &url, &target_path, logger, file_name).await {
                Ok(bytes) => {
                    log_message(
                        logger,
                        LogLevel::Info,
                        &format!("Downloaded {file_name} ({bytes} bytes)"),
                    );
                    last_err = None;
                    break;
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&target_path);
                    last_err = Some(e);
                    if attempt < MAX_RETRIES {
                        let delay = Duration::from_secs(2u64.pow(attempt));
                        log_message(
                            logger,
                            LogLevel::Warn,
                            &format!(
                                "Download {file_name} failed (attempt {attempt}/{MAX_RETRIES}), retrying in {delay:?}"
                            ),
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        if let Some(e) = last_err {
            return Err(MemoryError::Storage(format!(
                "failed to download {file_name} after {MAX_RETRIES} retries: {e}"
            )));
        }
    }

    log_message(
        logger,
        LogLevel::Info,
        &format!("Model cached at {}", cache_dir.display()),
    );
    Ok(cache_dir.to_path_buf())
}

async fn download_file(
    client: &reqwest::Client,
    url: &str,
    target: &Path,
    logger: &StdoutLogger,
    file_name: &str,
) -> Result<usize, MemoryError> {
    let tmp_path = target.with_extension("tmp");

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| MemoryError::Storage(format!("request failed: {e}")))?
        .error_for_status()
        .map_err(|e| MemoryError::Storage(format!("http error: {e}")))?;

    let total_size = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    if let Some(total) = total_size {
        log_message(
            logger,
            LogLevel::Info,
            &format!("Downloading {} ({} MB)...", file_name, total / 1_000_000),
        );
    } else {
        log_message(
            logger,
            LogLevel::Info,
            &format!("Downloading {file_name}..."),
        );
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| MemoryError::Storage(format!("failed to read response: {e}")))?;

    let size = bytes.len() as u64;
    log_message(
        logger,
        LogLevel::Info,
        &format!(
            "Downloaded {} ({} bytes / {} MB)",
            file_name,
            size,
            size / 1_000_000
        ),
    );

    std::fs::write(&tmp_path, &bytes).map_err(|e| {
        MemoryError::Storage(format!("failed to write {}: {e}", tmp_path.display()))
    })?;

    std::fs::rename(&tmp_path, target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp_path);
        MemoryError::Storage(format!(
            "failed to rename {} -> {}: {e}",
            tmp_path.display(),
            target.display()
        ))
    })?;

    Ok(size as usize)
}

fn log_message(logger: &StdoutLogger, level: LogLevel, msg: &str) {
    let mut event = HashMap::new();
    event.insert("op".to_string(), json!("model_loader"));
    event.insert("message".to_string(), json!(msg));
    logger.log(event, level);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_model_cached_returns_false_for_missing_dir() {
        assert!(!is_model_cached(Path::new("/nonexistent/path")));
    }

    #[test]
    fn is_model_cached_returns_false_for_partial_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("tokenizer.json"), "test").expect("write tokenizer");
        assert!(!is_model_cached(dir.path()));
    }

    #[test]
    fn is_model_cached_returns_true_when_all_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        for f in MODEL_REQUIRED_FILES {
            std::fs::write(dir.path().join(f), "test").expect("write model file");
        }
        assert!(is_model_cached(dir.path()));
    }

    #[test]
    fn sanitize_model_name_replaces_slash() {
        assert_eq!(
            sanitize_model_name("urchade/gliner_multi-v2.1"),
            "urchade--gliner_multi-v2.1"
        );
    }

    #[test]
    fn sanitize_model_name_preserves_no_slash() {
        assert_eq!(
            sanitize_model_name("bert-base-uncased"),
            "bert-base-uncased"
        );
    }

    #[test]
    fn gliner_model_files_contains_expected_entries() {
        assert!(GLINER_MODEL_FILES.contains(&"model.safetensors"));
        assert!(GLINER_MODEL_FILES.contains(&"gliner_config.json"));
        assert!(!GLINER_MODEL_FILES.contains(&"config.json"));
    }

    #[test]
    fn gliner_tokenizer_url_is_valid() {
        assert!(GLINER_TOKENIZER_URL.starts_with("https://huggingface.co/"));
        assert!(GLINER_TOKENIZER_URL.contains("tokenizer.json"));
    }
}
