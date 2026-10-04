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
