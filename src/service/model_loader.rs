//! Model download and caching for local embedding providers.
//!
//! Downloads model artifacts from HuggingFace Hub on first launch,
//! caches them on disk, and retries on network failures.
//!
//! Uses `hf-hub` for URL resolution (handles CDN routing, auth tokens, revisions)
//! and `reqwest` for actual HTTP (our TLS + redirect config).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;

/// Required model files for HuggingFace safetensors models.
pub const MODEL_REQUIRED_FILES: &[&str] = &["tokenizer.json", "config.json", "model.safetensors"];

/// Required model files for GLiNER (tokenizer is fetched separately).
/// Now using model.safetensors as it was updated in Dec 2025 with full weights.
pub const GLINER_MODEL_FILES: &[&str] = &["model.safetensors", "gliner_config.json"];

/// Direct URL for GLiNER tokenizer (the model repo doesn't include it).
/// Use /resolve/main/ path which follows Git LFS redirects properly.
pub const GLINER_TOKENIZER_URL: &str =
    "https://huggingface.co/juampahc/gliner_multi-v2.1-onnx/resolve/main/tokenizer.json";

/// Maximum number of download retries per file.
const MAX_RETRIES: u32 = 3;

/// Download timeout per file.
const DOWNLOAD_TIMEOUT_SECS: u64 = 120;

#[allow(dead_code)]
/// Checks if all required model files exist in the cache directory.
pub fn is_model_cached(cache_dir: &Path) -> bool {
    is_model_cached_with_files(cache_dir, MODEL_REQUIRED_FILES)
}

/// Checks if all required model files exist in the cache directory.
pub fn is_model_cached_with_files(cache_dir: &Path, required: &[&str]) -> bool {
    required.iter().all(|f| cache_dir.join(f).is_file())
}

#[allow(dead_code)]
/// Sanitizes a model name for use in a filesystem path.
/// Replaces "/" with "--" to avoid creating nested directories.
pub fn sanitize_model_name(model_name: &str) -> String {
    model_name.replace('/', "--")
}

/// Ensures all model files are present in the cache directory.
///
/// Downloads missing files from HuggingFace Hub with retry logic.
/// Returns the cache directory path on success.
///
/// # Errors
///
/// Returns [`MemoryError::Storage`] if download fails after all retries.
pub async fn ensure_model_cached(
    repo_id: &str,
    cache_dir: &Path,
    logger: &StdoutLogger,
) -> Result<PathBuf, MemoryError> {
    ensure_model_cached_with_files(repo_id, cache_dir, MODEL_REQUIRED_FILES, logger).await
}

/// Ensures a GLiNER model is fully cached with tokenizer from a direct URL.
///
/// GLiNER models (e.g., `urchade/gliner_multi-v2.1`) contain only `model.safetensors`
/// and `gliner_config.json`. The tokenizer is downloaded separately from a known URL.
///
/// # Errors
///
/// Returns [`MemoryError::Storage`] if any download fails.
pub async fn ensure_gliner_model_cached(
    model_repo: &str,
    cache_dir: &Path,
    logger: &StdoutLogger,
) -> Result<PathBuf, MemoryError> {
    // 1. Download model weights + gliner config from model repo
    ensure_model_cached_with_files(model_repo, cache_dir, GLINER_MODEL_FILES, logger).await?;

    // 2. Download tokenizer directly by URL (model repo doesn't have it)
    let tokenizer_path = cache_dir.join("tokenizer.json");
    if !tokenizer_path.is_file() {
        log_info(logger, "Downloading GLiNER tokenizer from remote URL");
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
                    log_info(
                        logger,
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
                        log_warn(
                            logger,
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
        log_info(logger, "tokenizer.json already present, skipping");
    }

    Ok(cache_dir.to_path_buf())
}

/// Ensures all model files are present in the cache directory.
///
/// Downloads missing files from HuggingFace Hub with retry logic.
/// Returns the cache directory path on success.
///
/// # Errors
///
/// Returns [`MemoryError::Storage`] if download fails after all retries.
pub async fn ensure_model_cached_with_files(
    repo_id: &str,
    cache_dir: &Path,
    required_files: &[&str],
    logger: &StdoutLogger,
) -> Result<PathBuf, MemoryError> {
    if is_model_cached_with_files(cache_dir, required_files) {
        log_info(
            logger,
            &format!("Model already cached at {}", cache_dir.display()),
        );
        return Ok(cache_dir.to_path_buf());
    }

    log_info(
        logger,
        &format!("Downloading model {repo_id} to {}", cache_dir.display()),
    );

    std::fs::create_dir_all(cache_dir).map_err(|e| {
        MemoryError::Storage(format!(
            "failed to create model cache dir {}: {e}",
            cache_dir.display()
        ))
    })?;

    // hf-hub handles URL resolution (CDN routing, auth, revisions)
    let api = hf_hub::api::tokio::ApiBuilder::new()
        .build()
        .map_err(|e| MemoryError::Storage(format!("failed to init hf-hub api: {e}")))?;

    let repo = api.model(repo_id.to_string());

    // reqwest handles actual HTTP with our TLS + redirect config
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| MemoryError::Storage(format!("failed to build http client: {e}")))?;

    for file_name in required_files {
        let target_path = cache_dir.join(file_name);
        if target_path.is_file() {
            log_info(logger, &format!("{file_name} already present, skipping"));
            continue;
        }

        // hf-hub resolves the canonical URL (handles CDN, auth tokens, revisions)
        let url = repo.url(file_name);

        let mut last_err = None;
        for attempt in 1..=MAX_RETRIES {
            match download_file(&http, &url, &target_path, logger, file_name).await {
                Ok(bytes) => {
                    log_info(logger, &format!("Downloaded {file_name} ({bytes} bytes)"));
                    last_err = None;
                    break;
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&target_path);
                    last_err = Some(e);
                    if attempt < MAX_RETRIES {
                        let delay = Duration::from_secs(2u64.pow(attempt));
                        log_warn(
                            logger,
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

    log_info(logger, &format!("Model cached at {}", cache_dir.display()));
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

    // Get total size for progress reporting
    let total_size = response
        .headers()
        .get(reqwest::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    if let Some(total) = total_size {
        log_info(
            logger,
            &format!("Downloading {} ({} MB)...", file_name, total / 1_000_000),
        );
    } else {
        log_info(logger, &format!("Downloading {}...", file_name));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| MemoryError::Storage(format!("failed to read response: {e}")))?;

    let size = bytes.len() as u64;

    // Log progress at start
    log_info(
        logger,
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

fn log_info(logger: &crate::logging::StdoutLogger, msg: &str) {
    logger.log(
        crate::log_event!("model_loader", "info", "message" => msg),
        LogLevel::Info,
    );
}

fn log_warn(logger: &crate::logging::StdoutLogger, msg: &str) {
    logger.log(
        crate::log_event!("model_loader", "warn", "message" => msg),
        LogLevel::Warn,
    );
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
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), "test").unwrap();
        assert!(!is_model_cached(dir.path()));
    }

    #[test]
    fn is_model_cached_returns_true_when_all_present() {
        let dir = tempfile::tempdir().unwrap();
        for f in MODEL_REQUIRED_FILES {
            std::fs::write(dir.path().join(f), "test").unwrap();
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
    fn gliner_model_cached_checks_correct_files() {
        let dir = tempfile::tempdir().unwrap();
        // Only model files — not cached yet
        for f in GLINER_MODEL_FILES {
            std::fs::write(dir.path().join(f), "test").unwrap();
        }
        // Without tokenizer, not fully cached
        assert!(!dir.path().join("tokenizer.json").is_file());

        // Add tokenizer file — now cached
        std::fs::write(dir.path().join("tokenizer.json"), "test").unwrap();
        assert!(dir.path().join("tokenizer.json").is_file());
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
