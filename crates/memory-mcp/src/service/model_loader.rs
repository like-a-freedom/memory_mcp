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

/// Maximum number of download retries per file.
const MAX_RETRIES: u32 = 3;

/// Checks if all required model files exist in the cache directory.
pub fn is_model_cached_with_files(cache_dir: &Path, required: &[&str]) -> bool {
    required.iter().all(|f| cache_dir.join(f).is_file())
}

/// Emits a `model_loader` log event.
fn log_message(logger: &StdoutLogger, level: LogLevel, msg: &str) {
    let mut event = HashMap::new();
    event.insert("op".to_string(), json!("model_loader"));
    event.insert("message".to_string(), json!(msg));
    logger.log(event, level);
}

/// Ensures all model files are present in the cache directory.
pub async fn ensure_model_cached(
    repo_id: &str,
    cache_dir: &Path,
    logger: &StdoutLogger,
) -> Result<PathBuf, MemoryError> {
    ensure_model_cached_with_files(repo_id, cache_dir, MODEL_REQUIRED_FILES, logger).await
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

    let api = hf_hub::HFClient::builder()
        .cache_dir(cache_dir.to_path_buf())
        .build()
        .map_err(|e| MemoryError::Storage(format!("failed to init hf-hub api: {e}")))?;
    let (owner, name) = hf_hub::split_id(repo_id);
    let repo = api.model(owner, name);

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

        let mut last_err: Option<String> = None;
        for attempt in 1..=MAX_RETRIES {
            match repo
                .download_file()
                .filename(file_name.to_string())
                .send()
                .await
            {
                Ok(downloaded_path) => {
                    match tokio::fs::copy(&downloaded_path, &target_path).await {
                        Ok(_) => {
                            let bytes = std::fs::metadata(&target_path)
                                .map(|metadata| metadata.len())
                                .unwrap_or_default();
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
                            last_err = Some(format!("failed to copy cached download: {e}"));
                        }
                    }
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&target_path);
                    last_err = Some(e.to_string());
                }
            }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_model_cached_with_files_returns_false_for_missing_dir() {
        assert!(!is_model_cached_with_files(
            Path::new("/nonexistent/path"),
            MODEL_REQUIRED_FILES
        ));
    }

    #[test]
    fn is_model_cached_with_files_returns_false_for_partial_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("tokenizer.json"), "test").expect("write tokenizer");
        assert!(!is_model_cached_with_files(
            dir.path(),
            MODEL_REQUIRED_FILES
        ));
    }

    #[test]
    fn is_model_cached_with_files_returns_true_when_all_present() {
        let dir = tempfile::tempdir().expect("tempdir");
        for f in MODEL_REQUIRED_FILES {
            std::fs::write(dir.path().join(f), "test").expect("write model file");
        }
        assert!(is_model_cached_with_files(dir.path(), MODEL_REQUIRED_FILES));
    }

    #[test]
    fn is_model_cached_with_files_custom_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        let required = &["a.txt", "b.txt"];
        std::fs::write(dir.path().join("a.txt"), "test").expect("write a");
        std::fs::write(dir.path().join("b.txt"), "test").expect("write b");
        assert!(is_model_cached_with_files(dir.path(), required));

        std::fs::remove_file(dir.path().join("b.txt")).expect("remove b");
        assert!(!is_model_cached_with_files(dir.path(), required));
    }

    #[test]
    fn is_model_cached_with_files_empty_list() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(is_model_cached_with_files(dir.path(), &[]));
    }

    #[test]
    fn model_required_files_contains_expected_entries() {
        assert!(MODEL_REQUIRED_FILES.contains(&"tokenizer.json"));
        assert!(MODEL_REQUIRED_FILES.contains(&"config.json"));
        assert!(MODEL_REQUIRED_FILES.contains(&"model.safetensors"));
        assert_eq!(MODEL_REQUIRED_FILES.len(), 3);
    }

    #[test]
    fn is_model_cached_with_files_returns_false_for_empty_dir() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(!is_model_cached_with_files(
            dir.path(),
            MODEL_REQUIRED_FILES
        ));
    }

    #[test]
    fn is_model_cached_with_files_returns_false_for_dir_with_subdirs_only() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("subdir")).expect("create subdir");
        assert!(!is_model_cached_with_files(
            dir.path(),
            MODEL_REQUIRED_FILES
        ));
    }
}
