use std::path::Path;

use sha2::Digest;

use crate::corpus::manifest::{CorpusManifest, PreparedCorpus};
use crate::error::EvalError;

#[async_trait::async_trait]
pub trait CorpusFetcher: Send + Sync {
    async fn fetch(&self, url: &str, revision: &str) -> Result<Vec<u8>, EvalError>;
}

pub async fn prepare_corpus(
    manifest: &CorpusManifest,
    output_root: &Path,
    fetcher: &dyn CorpusFetcher,
) -> Result<PreparedCorpus, EvalError> {
    let corpus_dir = output_root
        .join(&manifest.corpus_id)
        .join(&manifest.revision);

    if corpus_dir.exists() {
        return manifest.validate_at(&corpus_dir);
    }

    std::fs::create_dir_all(&corpus_dir).map_err(|source| EvalError::Io {
        path: corpus_dir.clone(),
        source,
    })?;

    let data = fetcher
        .fetch(&manifest.source_url, &manifest.revision)
        .await?;

    if data.len() as u64 != manifest.byte_size {
        std::fs::remove_dir_all(&corpus_dir).ok();
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
        std::fs::remove_dir_all(&corpus_dir).ok();
        return Err(EvalError::InvalidInput(format!(
            "sha-256 mismatch: expected {}, got {}",
            manifest.sha256, computed
        )));
    }

    let data_path = corpus_dir.join(&manifest.data_file);
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

    let manifest_path = corpus_dir.join("manifest.json");
    let manifest_json = serde_json::to_string_pretty(manifest).map_err(EvalError::Artifact)?;
    std::fs::write(&manifest_path, &manifest_json).map_err(|source| EvalError::Io {
        path: manifest_path,
        source,
    })?;

    manifest.validate_at(&corpus_dir)
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
    }
}
