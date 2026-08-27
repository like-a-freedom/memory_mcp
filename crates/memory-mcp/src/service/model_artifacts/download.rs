//! Revision resolution and artifact downloading for model-backed extractors.
//!
//! The store depends on `RevisionResolver` and `ArtifactFetcher` traits so
//! lifecycle behavior is testable with fakes. Default HTTP implementations
//! resolve upstream HEAD from the Hugging Face API and stream files with a
//! byte-progress stall watchdog: downloads fail after 60 seconds without
//! forward progress, not after a total wall-clock duration.

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::service::MemoryError;

use super::manifest::ArtifactRequirement;
use super::progress::ModelProgressSink;

/// Total attempts for resolving the latest revision.
pub(crate) const REVISION_RESOLVE_ATTEMPTS: u32 = 2;

/// Backoff between resolve attempts.
pub(crate) const REVISION_RESOLVE_BACKOFF: Duration = Duration::from_millis(500);

/// Total deadline for resolving the latest revision.
pub(crate) const REVISION_RESOLVE_DEADLINE: Duration = Duration::from_secs(10);

/// Seconds without byte progress before a download is considered stalled.
pub(crate) const DOWNLOAD_STALL_TIMEOUT_SECS: u64 = 60;

/// Ticks between stall checks.
const STALL_CHECK_INTERVAL: Duration = Duration::from_secs(1);

/// Injectable wall clock.
pub trait Clock: Send + Sync {
    fn now_secs(&self) -> i64;
}

/// System wall clock in Unix epoch seconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_secs(&self) -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or_default()
    }
}

/// Resolves the latest upstream revision for a repository.
#[async_trait]
pub trait RevisionResolver: Send + Sync {
    /// Returns the resolved revision (commit hash) for `repository`.
    ///
    /// # Errors
    ///
    /// Returns a [`MemoryError::Transient`] when the upstream API is
    /// unreachable so the caller can apply its retry/fallback policy.
    async fn latest(&self, repository: &str) -> Result<String, MemoryError>;
}

/// Fetches one artifact into a local target path.
#[async_trait]
pub trait ArtifactFetcher: Send + Sync {
    /// Downloads `requirement` from `repository` at `revision` into `target`,
    /// reporting byte progress through `progress`. The cancellation token is
    /// observed before the request is sent, between each chunk, and before
    /// the atomic rename of the partial file into place.
    ///
    /// # Errors
    ///
    /// Returns a [`MemoryError::Storage`] on network failure or when no bytes
    /// arrive for [`DOWNLOAD_STALL_TIMEOUT_SECS`]. Returns
    /// [`MemoryError::Transient`] when `cancellation` is observed; this is
    /// classified as a stop (not a failure) by the refresh runtime.
    async fn fetch(
        &self,
        repository: &str,
        revision: &str,
        requirement: &ArtifactRequirement,
        target: &Path,
        progress: &dyn ModelProgressSink,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<(), MemoryError>;
}

/// Resolves HEAD via the Hugging Face model API.
pub struct HfRevisionResolver {
    http: reqwest::Client,
}

impl HfRevisionResolver {
    pub fn new() -> Result<Self, MemoryError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|err| MemoryError::Storage(format!("failed to build http client: {err}")))?;
        Ok(Self { http })
    }
}

#[async_trait]
impl RevisionResolver for HfRevisionResolver {
    async fn latest(&self, repository: &str) -> Result<String, MemoryError> {
        let url = format!("https://huggingface.co/api/models/{repository}");
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|err| MemoryError::Transient(format!("revision lookup failed: {err}")))?;
        if !response.status().is_success() {
            return Err(MemoryError::Transient(format!(
                "revision lookup for {repository} returned {}",
                response.status()
            )));
        }
        let body: serde_json::Value = response
            .json()
            .await
            .map_err(|err| MemoryError::Transient(format!("invalid revision response: {err}")))?;
        let sha = body
            .get("sha")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                MemoryError::Transient(format!("revision response for {repository} lacks `sha`"))
            })?;
        Ok(sha.to_string())
    }
}

/// Verifies a hex SHA-256 digest against an expected value.
pub(crate) fn verify_checksum(actual: &str, expected: &str, path: &str) -> Result<(), MemoryError> {
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(MemoryError::Validation(format!(
            "checksum mismatch for {path}: expected {expected}, got {actual}"
        )))
    }
}

/// Streams files from `https://huggingface.co/{repo}/resolve/{revision}/{path}`
/// with byte progress and a stall watchdog.
pub struct HfArtifactFetcher {
    http: reqwest::Client,
}

impl HfArtifactFetcher {
    pub fn new() -> Result<Self, MemoryError> {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .map_err(|err| MemoryError::Storage(format!("failed to build http client: {err}")))?;
        Ok(Self { http })
    }
}

#[async_trait]
impl ArtifactFetcher for HfArtifactFetcher {
    async fn fetch(
        &self,
        repository: &str,
        revision: &str,
        requirement: &ArtifactRequirement,
        target: &Path,
        progress: &dyn ModelProgressSink,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<(), MemoryError> {
        if cancellation.is_cancelled() {
            return Err(MemoryError::Transient(
                "NER artifact refresh cancelled".to_string(),
            ));
        }
        let url = format!(
            "https://huggingface.co/{repository}/resolve/{revision}/{}",
            requirement.path
        );
        // `error_for_status()` is fused with the `send` future so we cannot
        // await `send` inside `tokio::select!` cleanly. Poll the future
        // inside the loop to surface cancellation between retries.
        let send_fut = self.http.get(&url).send();
        let response = tokio::select! {
            result = send_fut => {
                result.map_err(|err| MemoryError::Storage(format!(
                    "download {} failed: {err}",
                    requirement.path
                )))?
            }
            _ = cancellation.cancelled() => {
                return Err(MemoryError::Transient(
                    "NER artifact refresh cancelled".to_string(),
                ));
            }
        }
        .error_for_status()
        .map_err(|err| {
            MemoryError::Storage(format!("download {} failed: {err}", requirement.path))
        })?;

        let total_bytes = response
            .content_length()
            .unwrap_or_default()
            .max(requirement.path.len() as u64); // avoid divide-by-zero on unknown length

        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                MemoryError::Storage(format!(
                    "cannot create download directory {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let tmp = target.with_extension("part");
        let _part_guard = PartialFileGuard::new(&tmp);
        let mut file = tokio::fs::File::create(&tmp).await.map_err(|err| {
            MemoryError::Storage(format!("cannot create {}: {err}", tmp.display()))
        })?;
        let mut response = response;
        use tokio::io::AsyncWriteExt;
        let mut downloaded: u64 = 0;
        let mut last_progress: u64 = 0;
        let mut last_byte_at = std::time::Instant::now();
        let stall_timeout = Duration::from_secs(DOWNLOAD_STALL_TIMEOUT_SECS);
        let mut hasher = Sha256::new();

        loop {
            let chunk_fut = response.chunk();
            let tick = tokio::time::timeout(STALL_CHECK_INTERVAL, chunk_fut);
            let next = tokio::select! {
                result = tick => result,
                _ = cancellation.cancelled() => {
                    return Err(MemoryError::Transient(
                        "NER artifact refresh cancelled".to_string(),
                    ));
                }
            };
            match next {
                Err(_) => {
                    // Interval elapsed without a chunk: check for stall.
                    if last_byte_at.elapsed() >= stall_timeout {
                        return Err(MemoryError::Storage(format!(
                            "download of {} stalled: no bytes for {stall_timeout:?}",
                            requirement.path
                        )));
                    }
                }
                Ok(Ok(Some(chunk))) => {
                    hasher.update(&chunk);
                    file.write_all(&chunk).await.map_err(|err| {
                        MemoryError::Storage(format!("cannot write {}: {err}", tmp.display()))
                    })?;
                    downloaded += chunk.len() as u64;
                    last_byte_at = std::time::Instant::now();
                    let percent = ((downloaded as f64 / total_bytes as f64) * 100.0) as u8;
                    if downloaded - last_progress >= total_bytes / 20 || percent >= 100 {
                        progress.emit(&super::progress::ModelProgressEvent::download(
                            requirement.path,
                            Some(revision.to_string()),
                            downloaded,
                            total_bytes,
                            percent,
                        ));
                        last_progress = downloaded;
                    }
                }
                Ok(Ok(None)) => break,
                Ok(Err(err)) => {
                    return Err(MemoryError::Storage(format!(
                        "download of {} failed: {err}",
                        requirement.path
                    )));
                }
            }
        }
        if cancellation.is_cancelled() {
            return Err(MemoryError::Transient(
                "NER artifact refresh cancelled".to_string(),
            ));
        }
        file.flush().await.map_err(|err| {
            MemoryError::Storage(format!("cannot flush {}: {err}", tmp.display()))
        })?;
        if let Some(expected) = requirement.sha256 {
            let actual = hex::encode(hasher.finalize());
            if let Err(err) = verify_checksum(&actual, expected, requirement.path) {
                return Err(err);
            }
        }
        // Drop the file handle so the rename below is atomic.
        drop(file);
        tokio::fs::rename(&tmp, target).await.map_err(|err| {
            MemoryError::Storage(format!(
                "cannot finalize {} -> {}: {err}",
                tmp.display(),
                target.display()
            ))
        })?;
        // Successfully renamed: the guard must NOT remove the destination.
        _part_guard.commit();
        Ok(())
    }
}

/// RAII guard that removes a partial download file on drop unless
/// [`PartialFileGuard::commit`] is called. Cancellation, transport errors,
/// and panic unwinds all observe the cleanup.
pub struct PartialFileGuard {
    path: PathBuf,
    committed: bool,
}

impl PartialFileGuard {
    fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            committed: false,
        }
    }

    /// Disarms the guard so the file is preserved on drop.
    fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for PartialFileGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        // Best-effort cleanup; the file may already be gone.
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_clock_returns_positive_epoch() {
        let clock = SystemClock;
        assert!(clock.now_secs() > 1_600_000_000);
    }

    #[test]
    fn resolve_constants_are_sane() {
        assert_eq!(REVISION_RESOLVE_ATTEMPTS, 2);
        assert_eq!(DOWNLOAD_STALL_TIMEOUT_SECS, 60);
        assert!(REVISION_RESOLVE_DEADLINE <= Duration::from_secs(10));
    }

    #[test]
    fn verify_checksum_accepts_match_and_rejects_mismatch() {
        let bytes = b"checkpoint-content";
        let good = hex::encode(Sha256::digest(bytes));

        assert!(verify_checksum(&good, &good, "model.bin").is_ok());
        // Case-insensitive hex comparison.
        assert!(verify_checksum(&good.to_uppercase(), &good, "model.bin").is_ok());

        let bad = "0".repeat(64);
        let err = verify_checksum(&good, &bad, "model.bin").expect_err("mismatch rejected");
        match err {
            MemoryError::Validation(message) => {
                assert!(message.contains("checksum mismatch"));
                assert!(message.contains("model.bin"));
            }
            other => panic!("expected Validation error, got {other}"),
        }
    }
}
