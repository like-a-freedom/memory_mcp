//! Deterministic candidate preparation for filesystem ingestion.
//!
//! A candidate is a path under the configured inbox root. Preparation
//! stabilizes the file (size + mtime), hashes the raw bytes, parses them once,
//! and resolves source identity and reference time — all before the revision
//! becomes claimable.
//!
//! The public entry points are consumed by the runtime in later tasks; until
//! then they are exercised only by tests, so dead-code analysis is relaxed.
#![allow(dead_code)]

use std::path::{Component, Path};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, Utc};
use sha2::Digest;

use crate::config::fs_watch::FsWatchConfig;
use crate::error::MemoryError;
use crate::service::fs_watch::{
    STABILITY_REQUIRED_MATCHES, STABILITY_SAMPLE_INTERVAL, STABILITY_TIMEOUT,
};

/// Normalized content prepared from one immutable raw-byte revision.
#[derive(Debug)]
pub(crate) struct PreparedInboxRevision {
    pub relative_path: String,
    pub lineage: String,
    pub content_sha256: String,
    pub source_id: String,
    pub source_type: String,
    pub t_ref: DateTime<Utc>,
    pub prepared_content: String,
}

/// Outcome of preparing one candidate path.
#[derive(Debug)]
pub(crate) enum CandidateOutcome {
    Ready(Box<PreparedInboxRevision>),
    Skipped(CandidateSkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CandidateSkipReason {
    NotRegularFile,
    UnsupportedFormat,
    Symlink,
    Interrupted,
    OutsideRoot,
}

/// Normalizes a candidate path relative to the inbox, rejecting escapes and
/// symlinked components. Always uses `/` separators.
pub(crate) fn normalized_relative_path(inbox: &Path, path: &Path) -> Result<String, MemoryError> {
    let relative = path.strip_prefix(inbox).map_err(|_| {
        MemoryError::Validation(format!(
            "candidate path `{}` is not inside inbox root",
            path.display()
        ))
    })?;
    let mut parts: Vec<String> = Vec::new();
    for component in relative.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::ParentDir => return Err(MemoryError::Validation(format!(
                "candidate path `{}` escapes the inbox root",
                path.display()
            ))),
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => {
                return Err(MemoryError::Validation(format!(
                    "candidate path `{}` is not relative to the inbox root",
                    path.display()
                )));
            }
        }
    }
    if parts.is_empty() {
        return Err(MemoryError::Validation(format!(
            "candidate path `{}` has no relative components",
            path.display()
        )));
    }
    Ok(parts.join("/"))
}

/// Returns `true` when the candidate's symlink metadata is a symlink.
pub(crate) fn is_symlink(path: &Path) -> bool {
    path.symlink_metadata()
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
}

/// Determines the source type for a relative path (`email` for `.eml`,
/// `document` otherwise).
pub(crate) fn source_type_for_relative_path(relative: &str) -> &'static str {
    match Path::new(relative).extension().and_then(|e| e.to_str()) {
        Some("eml") => "email",
        _ => "document",
    }
}

/// Prepares a stable candidate for ingestion.
///
/// Returns `Skipped` for symlinks, unsupported formats, and non-regular files;
/// `Interrupted` when the cancellation token fires during stabilization.
pub(crate) async fn prepare_candidate(
    inbox: &FsWatchConfig,
    path: &Path,
    stability: Option<(Duration, u8, Duration)>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<CandidateOutcome, MemoryError> {
    // Stabilization loop: sample size + mtime until two consecutive samples
    // match (or the timeout fires), reading the bytes only after stability.
    let (sample_interval, required_matches, timeout) = stability.unwrap_or((
        STABILITY_SAMPLE_INTERVAL,
        STABILITY_REQUIRED_MATCHES,
        STABILITY_TIMEOUT,
    ));
    if is_symlink(path) {
        return Ok(CandidateOutcome::Skipped(CandidateSkipReason::Symlink));
    }
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(_) => return Ok(CandidateOutcome::Skipped(CandidateSkipReason::NotRegularFile)),
    };
    if !metadata.is_file() {
        return Ok(CandidateOutcome::Skipped(CandidateSkipReason::NotRegularFile));
    }

    let relative_path = normalized_relative_path(&inbox.inbox, path)?;
    let mut last_sample: Option<(u64, Option<SystemTime>)> = None;
    let mut consecutive_matches: u8 = 0;
    let started = Instant::now();

    loop {
        if cancel.is_cancelled() {
            return Ok(CandidateOutcome::Skipped(CandidateSkipReason::Interrupted));
        }
        let metadata = match path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(_) => return Ok(CandidateOutcome::Skipped(CandidateSkipReason::NotRegularFile)),
        };
        let sample = (
            metadata.len(),
            metadata.modified().ok(),
        );
        if let Some(previous) = last_sample
            && previous == sample
        {
            consecutive_matches += 1;
        } else {
            consecutive_matches = 1;
        }
        if consecutive_matches >= required_matches {
            break;
        }
        if started.elapsed() >= timeout {
            return Ok(CandidateOutcome::Skipped(CandidateSkipReason::NotRegularFile));
        }
        last_sample = Some(sample);
        tokio::time::sleep(sample_interval).await;
    }

    if cancel.is_cancelled() {
        return Ok(CandidateOutcome::Skipped(CandidateSkipReason::Interrupted));
    }

    // Hash raw bytes, then parse those exact bytes once.
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(_) => return Ok(CandidateOutcome::Skipped(CandidateSkipReason::NotRegularFile)),
    };
    let content_sha256 = hex::encode(sha2::Sha256::digest(&bytes));

    // Format detection and parsing via the shared content-extraction module.
    let (source_type, prepared_content) = match crate::service::content_extraction::parse_bytes_for_watch(&relative_path, &bytes) {
        Ok(parsed) => parsed,
        Err(_) => return Ok(CandidateOutcome::Skipped(CandidateSkipReason::UnsupportedFormat)),
    };

    let t_ref = crate::service::content_extraction::watch_reference_time(&relative_path, &bytes, &metadata);

    let lineage = format!("fs:{relative_path}");
    let source_id = format!("{lineage}:{content_sha256}");

    Ok(CandidateOutcome::Ready(Box::new(PreparedInboxRevision {
        relative_path,
        lineage,
        content_sha256,
        source_id,
        source_type: source_type.to_string(),
        t_ref,
        prepared_content,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn config_for(dir: &std::path::Path) -> FsWatchConfig {
        FsWatchConfig {
            inbox: dir.to_path_buf(),
        }
    }

    fn no_cancel() -> tokio_util::sync::CancellationToken {
        tokio_util::sync::CancellationToken::new()
    }

    #[test]
    fn normalized_relative_path_uses_forward_slashes() {
        let root = Path::new("/tmp/inbox");
        let nested = root.join("docs").join("spec.md");
        assert_eq!(
            normalized_relative_path(root, &nested).unwrap(),
            "docs/spec.md"
        );
    }

    #[test]
    fn normalized_relative_path_rejects_escapes() {
        let root = Path::new("/tmp/inbox");
        let outside = Path::new("/etc/passwd");
        let err = normalized_relative_path(root, outside).unwrap_err();
        assert!(err.to_string().contains("not inside inbox root"));

        let escape = root.join("..").join("secret.md");
        let err = normalized_relative_path(root, &escape).unwrap_err();
        assert!(err.to_string().contains("escapes"));
    }

    #[test]
    fn symlink_candidate_is_skipped() {
        let dir = tempdir().expect("temp");
        let target = dir.path().join("target.txt");
        std::fs::write(&target, "hello").expect("write target");
        let link = dir.path().join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(&target, &link).expect("symlink");

        let config = config_for(dir.path());
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let outcome = rt.block_on(prepare_candidate(
            &config,
            &link,
            None,
            &no_cancel(),
        ));
        match outcome.expect("outcome") {
            CandidateOutcome::Skipped(reason) => assert_eq!(reason, CandidateSkipReason::Symlink),
            other => panic!("expected symlink skip, got {other:?}"),
        }
    }

    #[test]
    fn unsupported_extension_candidate_is_skipped() {
        let dir = tempdir().expect("temp");
        let file = dir.path().join("notes.json");
        std::fs::write(&file, "{}").expect("write file");

        let config = config_for(dir.path());
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let outcome = rt.block_on(prepare_candidate(
            &config,
            &file,
            None,
            &no_cancel(),
        ));
        match outcome.expect("outcome") {
            CandidateOutcome::Skipped(reason) => {
                assert_eq!(reason, CandidateSkipReason::UnsupportedFormat)
            }
            other => panic!("expected unsupported skip, got {other:?}"),
        }
    }

    #[test]
    fn revision_identity_uses_relative_lineage_and_raw_bytes() {
        let dir = tempdir().expect("temp");
        let file = dir.path().join("spec.md");
        std::fs::write(&file, "version one").expect("write file");

        let config = config_for(dir.path());
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let outcome = rt.block_on(prepare_candidate(&config, &file, None, &no_cancel()));
        let CandidateOutcome::Ready(prepared) = outcome.expect("outcome") else {
            panic!("expected ready");
        };
        assert_eq!(prepared.lineage, "fs:spec.md");
        assert_eq!(
            prepared.source_id,
            format!("{}:{}", prepared.lineage, prepared.content_sha256)
        );
        assert_eq!(prepared.source_type, "document");
        assert!(prepared.prepared_content.contains("version one"));
    }

    #[test]
    fn rename_produces_new_lineage_same_bytes() {
        let dir = tempdir().expect("temp");
        let first = dir.path().join("a.md");
        std::fs::write(&first, "same bytes").expect("write");
        let config = config_for(dir.path());

        let rt = tokio::runtime::Runtime::new().expect("rt");
        let CandidateOutcome::Ready(first_prep) =
            rt.block_on(prepare_candidate(&config, &first, None, &no_cancel()))
                .expect("first")
        else {
            panic!("expected ready");
        };

        std::fs::rename(&first, dir.path().join("b.md")).expect("rename");
        let CandidateOutcome::Ready(second_prep) = rt
            .block_on(prepare_candidate(
                &config,
                &dir.path().join("b.md"),
                None,
                &no_cancel(),
            ))
            .expect("second")
        else {
            panic!("expected ready");
        };

        assert_eq!(first_prep.content_sha256, second_prep.content_sha256);
        assert_ne!(first_prep.lineage, second_prep.lineage);
        assert_eq!(second_prep.lineage, "fs:b.md");
    }

    #[tokio::test]
    async fn still_changing_file_waits_for_stability() {
        let dir = tempdir().expect("temp");
        let file = dir.path().join("growing.txt");
        std::fs::write(&file, "one").expect("write");

        let config = config_for(dir.path());
        // The writer mutates the file continuously; the candidate must not be
        // ready while the writer is still active, and must succeed only after
        // the writer stops.
        let stop_writer = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let file_for_writer = file.clone();
        let stop_for_writer = stop_writer.clone();
        let writer = tokio::spawn(async move {
            let mut index = 0u32;
            while !stop_for_writer.load(std::sync::atomic::Ordering::SeqCst) {
                index += 1;
                std::fs::write(&file_for_writer, format!("content {index}")).expect("rewrite");
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        });

        let candidate = tokio::spawn({
            let config = config.clone();
            let file = file.clone();
            let cancel = no_cancel();
            async move {
                prepare_candidate(
                    &config,
                    &file,
                    Some((
                        std::time::Duration::from_millis(40),
                        2,
                        std::time::Duration::from_secs(10),
                    )),
                    &cancel,
                )
                .await
                .expect("outcome")
            }
        });

        // While the writer is active, the candidate must not resolve.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !candidate.is_finished(),
            "candidate must not resolve while the file is still changing"
        );

        stop_writer.store(true, std::sync::atomic::Ordering::SeqCst);
        writer.await.expect("writer stopped");

        let outcome = candidate
            .await
            .expect("candidate resolved after writer stopped");
        let CandidateOutcome::Ready(prepared) = outcome else {
            panic!("expected ready after stability");
        };
        assert!(
            prepared.prepared_content.contains("content"),
            "expected prepared content to include the writer output, got {:?}",
            prepared.prepared_content
        );
    }

    #[tokio::test]
    async fn cancellation_returns_interrupted_skip() {
        let dir = tempdir().expect("temp");
        let file = dir.path().join("slow.txt");
        std::fs::write(&file, "data").expect("write");

        let config = config_for(dir.path());
        let cancel = tokio_util::sync::CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let handle = tokio::spawn(async move {
            prepare_candidate(
                &config,
                &file,
                Some((
                    std::time::Duration::from_millis(50),
                    2,
                    std::time::Duration::from_secs(30),
                )),
                &cancel_for_task,
            )
            .await
            .expect("outcome")
        });
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        cancel.cancel();
        match handle.await.expect("task") {
            CandidateOutcome::Skipped(reason) => {
                assert_eq!(reason, CandidateSkipReason::Interrupted)
            }
            other => panic!("expected interrupted skip, got {other:?}"),
        }
    }
}
