use std::collections::HashMap;
use std::path::Path;

use serde_json::json;

use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;

pub(super) async fn download_file(
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

pub(super) fn log_message(logger: &StdoutLogger, level: LogLevel, msg: &str) {
    let mut event = HashMap::new();
    event.insert("op".to_string(), json!("model_loader"));
    event.insert("message".to_string(), json!(msg));
    logger.log(event, level);
}
