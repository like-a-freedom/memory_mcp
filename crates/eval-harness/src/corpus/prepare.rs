use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::Digest;

use crate::corpus::manifest::{CorpusManifest, PreparedCorpus};
use crate::error::EvalError;

struct StagingDirectory(PathBuf);

impl Drop for StagingDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[async_trait::async_trait]
pub trait CorpusFetcher: Send + Sync {
    async fn fetch(&self, url: &str, revision: &str) -> Result<Vec<u8>, EvalError>;
}

pub async fn prepare_corpus(
    manifest: &CorpusManifest,
    output_root: &Path,
    fetcher: &dyn CorpusFetcher,
) -> Result<PreparedCorpus, EvalError> {
    let source_url = manifest.resolve_source_url()?;
    let corpus_dir = output_root
        .join(&manifest.corpus_id)
        .join(&manifest.revision);

    if corpus_dir.exists() {
        return manifest.validate_at(&corpus_dir);
    }

    let data = fetcher.fetch(&source_url, &manifest.revision).await?;

    if data.len() as u64 != manifest.byte_size {
        return Err(EvalError::InvalidInput(format!(
            "fetched {} bytes but manifest declares {}",
            data.len(),
            manifest.byte_size
        )));
    }

    let mut hasher = sha2::Sha256::new();
    hasher.update(&data);
    let computed = hex::encode(hasher.finalize());

    if computed != manifest.sha256 {
        return Err(EvalError::InvalidInput(format!(
            "sha-256 mismatch: expected {}, got {}",
            manifest.sha256, computed
        )));
    }

    let parent = output_root.join(&manifest.corpus_id);
    std::fs::create_dir_all(&parent).map_err(|source| EvalError::Io {
        path: parent.clone(),
        source,
    })?;
    static NEXT_STAGE: AtomicU64 = AtomicU64::new(0);
    let stage = loop {
        let candidate = parent.join(format!(
            ".prepare-{}-{}",
            std::process::id(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::create_dir(&candidate) {
            Ok(()) => break StagingDirectory(candidate),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(source) => {
                return Err(EvalError::Io {
                    path: candidate,
                    source,
                });
            }
        }
    };
    let data_path = stage.0.join(&manifest.data_file);
    if let Some(parent) = data_path.parent() {
        std::fs::create_dir_all(parent).map_err(|source| EvalError::Io {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    std::fs::write(&data_path, &data).map_err(|source| EvalError::Io {
        path: data_path.clone(),
        source,
    })?;

    for auxiliary in &manifest.auxiliary_files {
        let auxiliary_data = fetcher
            .fetch(&auxiliary.source_url, &auxiliary.revision)
            .await?;
        if auxiliary_data.len() as u64 != auxiliary.byte_size {
            return Err(EvalError::InvalidInput(format!(
                "fetched auxiliary {} bytes but manifest declares {}",
                auxiliary_data.len(),
                auxiliary.byte_size
            )));
        }
        let mut auxiliary_hasher = sha2::Sha256::new();
        auxiliary_hasher.update(&auxiliary_data);
        let auxiliary_hash = hex::encode(auxiliary_hasher.finalize());
        if auxiliary_hash != auxiliary.sha256 {
            return Err(EvalError::InvalidInput(format!(
                "auxiliary sha-256 mismatch: expected {}, got {}",
                auxiliary.sha256, auxiliary_hash
            )));
        }
        let auxiliary_path = stage.0.join(&auxiliary.data_file);
        if let Some(parent) = auxiliary_path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| EvalError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        std::fs::write(&auxiliary_path, auxiliary_data).map_err(|source| EvalError::Io {
            path: auxiliary_path,
            source,
        })?;
    }

    let manifest_path = stage.0.join("manifest.json");
    let manifest_json = serde_json::to_string_pretty(manifest).map_err(EvalError::Artifact)?;
    std::fs::write(&manifest_path, &manifest_json).map_err(|source| EvalError::Io {
        path: manifest_path,
        source,
    })?;

    manifest.validate_at(&stage.0)?;
    match std::fs::rename(&stage.0, &corpus_dir) {
        Ok(()) => manifest.validate_at(&corpus_dir),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && corpus_dir.exists() => {
            manifest.validate_at(&corpus_dir)
        }
        Err(source) => Err(EvalError::Io {
            path: corpus_dir,
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeFetcher {
        data: Vec<u8>,
    }

    #[async_trait::async_trait]
    impl CorpusFetcher for FakeFetcher {
        async fn fetch(&self, _url: &str, _revision: &str) -> Result<Vec<u8>, EvalError> {
            Ok(self.data.clone())
        }
    }

    struct ErrorFetcher;

    #[async_trait::async_trait]
    impl CorpusFetcher for ErrorFetcher {
        async fn fetch(&self, _url: &str, _revision: &str) -> Result<Vec<u8>, EvalError> {
            Err(EvalError::Suite("fetch failed".into()))
        }
    }

    fn test_manifest(content: &[u8]) -> CorpusManifest {
        let mut hasher = sha2::Sha256::new();
        hasher.update(content);
        let hash = hex::encode(hasher.finalize());

        CorpusManifest::parse(
            &serde_json::json!({
                "schema_version": "memory-mcp-corpus/v1",
                "corpus_id": "test-corpus",
                "source_url": "https://example.com/data.json",
                "revision": "rev1",
                "sha256": hash,
                "license": "MIT",
                "byte_size": content.len(),
                "case_count": 1,
                "adapter_version": "1",
                "data_file": "data.json"
            })
            .to_string(),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn bad_download_is_never_published() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = test_manifest(b"expected content");
        let result = prepare_corpus(
            &manifest,
            dir.path(),
            &FakeFetcher {
                data: b"different content".to_vec(),
            },
        )
        .await;
        assert!(result.is_err());
        assert!(!dir.path().join("test-corpus/rev1/data.json").exists());
    }

    #[tokio::test]
    async fn successful_preparation() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"hello world";
        let manifest = test_manifest(content);
        let prepared = prepare_corpus(
            &manifest,
            dir.path(),
            &FakeFetcher {
                data: content.to_vec(),
            },
        )
        .await
        .unwrap();
        assert!(prepared.data_path.exists());
    }

    #[tokio::test]
    async fn repeatable_preparation() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"hello world";
        let manifest = test_manifest(content);
        let fetcher = FakeFetcher {
            data: content.to_vec(),
        };

        let first = prepare_corpus(&manifest, dir.path(), &fetcher)
            .await
            .unwrap();
        let second = prepare_corpus(&manifest, dir.path(), &fetcher)
            .await
            .unwrap();
        assert_eq!(first.data_path, second.data_path);
    }

    #[tokio::test]
    async fn fetcher_error_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = test_manifest(b"content");
        let result = prepare_corpus(&manifest, dir.path(), &ErrorFetcher).await;
        assert!(result.is_err());
        assert!(!dir.path().join("test-corpus/rev1").exists());
        let retried = prepare_corpus(
            &manifest,
            dir.path(),
            &FakeFetcher {
                data: b"content".to_vec(),
            },
        )
        .await;
        assert!(retried.is_ok(), "failed download must permit retry");
    }

    #[tokio::test]
    async fn mutable_source_url_is_rejected_before_fetch_or_publish() {
        let dir = tempfile::tempdir().unwrap();
        let mut manifest = test_manifest(b"content");
        manifest.source_url = "https://example.com/main/data.json".into();
        let result = prepare_corpus(
            &manifest,
            dir.path(),
            &FakeFetcher {
                data: b"content".to_vec(),
            },
        )
        .await;
        assert!(result.is_err());
        assert!(!dir.path().join("test-corpus/rev1/data.json").exists());
    }

    #[tokio::test]
    async fn known_corpus_with_wrong_case_count_is_not_published_or_accepted_from_cache() {
        let dir = tempfile::tempdir().unwrap();
        let content = b"[]";
        let mut manifest = test_manifest(content);
        manifest.corpus_id = "longmemeval-cleaned".into();
        let result = prepare_corpus(
            &manifest,
            dir.path(),
            &FakeFetcher {
                data: content.to_vec(),
            },
        )
        .await;
        assert!(
            result.is_err(),
            "zero normalized cases contradict declared count"
        );
        let cache = dir.path().join("longmemeval-cleaned/rev1");
        assert!(!cache.exists());
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join("data.json"), content).unwrap();
        assert!(
            prepare_corpus(&manifest, dir.path(), &ErrorFetcher)
                .await
                .is_err()
        );
    }
}
