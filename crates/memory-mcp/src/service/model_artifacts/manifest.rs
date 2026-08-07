//! Artifact manifest and checkpoint identity contracts.
//!
//! Extractors declare the exact repository files they need; the artifact
//! service verifies completeness and integrity before activation. Identity is
//! a stable SHA-256 over sorted `path:size:sha256` entries so a changed file,
//! size, or checksum yields a different checkpoint identity.

use std::collections::BTreeMap;
use std::path::Path;

use crate::service::MemoryError;

/// One artifact file required by an extractor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArtifactRequirement {
    /// Path relative to the checkpoint root, e.g. `pytorch_model.bin`.
    pub path: &'static str,
    /// Optional expected SHA-256; verified when present.
    pub sha256: Option<&'static str>,
}

/// Everything needed to fetch and validate one extractor's artifacts.
#[derive(Debug, Clone)]
pub(crate) struct NerArtifactSpec {
    /// Stable extractor identity, e.g. `vago-lfm2.5-gliner`.
    pub extractor_id: &'static str,
    /// Hugging Face repository, e.g. `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`.
    pub repository: &'static str,
    /// Required files and their optional pinned checksums.
    pub files: &'static [ArtifactRequirement],
    /// Runtime/model-family version recorded in fingerprints.
    pub runtime_version: &'static str,
}

/// How trustworthy the resolved upstream revision is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RevisionStatus {
    /// Upstream HEAD was resolved and verified this startup.
    Latest,
    /// Upstream was unreachable; a previously verified revision is in use.
    UnverifiedLatest,
    /// The resolved HEAD failed validation and the previous good revision is in use.
    LatestIncompatible,
}

/// How a revision was validated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ValidationStatus {
    /// Matches a release-known Python reference on RU/EN/mixed fixtures.
    ReleaseParityVerified,
    /// Passed the embedded RU/EN runtime regression corpus only.
    RuntimeRegressionVerified,
}

/// A fully prepared, validated local checkpoint ready for a backend loader.
#[derive(Debug, Clone)]
pub(crate) struct PreparedCheckpoint {
    /// Local root containing all required artifact files.
    pub root: std::path::PathBuf,
    /// Repository the checkpoint was resolved from.
    pub repository: String,
    /// Resolved upstream revision (commit hash) or a release tag.
    pub revision: String,
    /// Stable content identity over sorted `path:size:sha256` entries.
    pub artifact_identity: String,
    /// How the revision was resolved.
    pub revision_status: RevisionStatus,
    /// How the revision was validated.
    pub validation_status: ValidationStatus,
}

/// Computes a stable SHA-256 identity from actual on-disk artifacts.
///
/// Entries are sorted by relative path; each is rendered as
/// `path:size:sha256`. A missing or zero-byte required file is an error.
pub(crate) fn artifact_identity(
    root: &Path,
    requirements: &[ArtifactRequirement],
) -> Result<String, MemoryError> {
    use sha2::{Digest, Sha256};

    let mut entries = BTreeMap::new();
    for requirement in requirements {
        let full = root.join(requirement.path);
        let bytes = std::fs::read(&full).map_err(|err| {
            MemoryError::Storage(format!(
                "missing artifact {} under {}: {err}",
                requirement.path,
                root.display()
            ))
        })?;
        if bytes.is_empty() {
            return Err(MemoryError::Validation(format!(
                "artifact {} is zero bytes",
                requirement.path
            )));
        }
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let checksum = hex::encode(hasher.finalize());
        entries.insert(
            requirement.path.to_string(),
            format!("{}:{}:{}", requirement.path, bytes.len(), checksum),
        );
    }

    let mut hasher = Sha256::new();
    for (_, entry) in entries {
        hasher.update(entry.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hex::encode(hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn identity_is_stable_and_order_independent() {
        let dir = TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join("a.txt"), b"alpha").expect("write a");
        std::fs::write(dir.path().join("b.txt"), b"beta").expect("write b");

        let files = &[
            ArtifactRequirement {
                path: "a.txt",
                sha256: None,
            },
            ArtifactRequirement {
                path: "b.txt",
                sha256: None,
            },
        ];
        let first = artifact_identity(dir.path(), files).expect("identity");
        let reversed = artifact_identity(
            dir.path(),
            &[
                ArtifactRequirement {
                    path: "b.txt",
                    sha256: None,
                },
                ArtifactRequirement {
                    path: "a.txt",
                    sha256: None,
                },
            ],
        )
        .expect("identity");
        assert_eq!(first, reversed);
        assert_eq!(first.len(), 64);
    }

    #[test]
    fn identity_changes_when_file_content_changes() {
        let dir = TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join("model.bin"), b"version one").expect("write");
        let files = &[ArtifactRequirement {
            path: "model.bin",
            sha256: None,
        }];
        let first = artifact_identity(dir.path(), files).expect("identity");
        std::fs::write(dir.path().join("model.bin"), b"version two").expect("overwrite");
        let second = artifact_identity(dir.path(), files).expect("identity");
        assert_ne!(first, second);
    }

    #[test]
    fn missing_required_file_is_rejected() {
        let dir = TempDir::new().expect("temp dir");
        let files = &[ArtifactRequirement {
            path: "absent.bin",
            sha256: None,
        }];
        assert!(matches!(
            artifact_identity(dir.path(), files),
            Err(MemoryError::Storage(_))
        ));
    }

    #[test]
    fn zero_byte_required_file_is_rejected() {
        let dir = TempDir::new().expect("temp dir");
        std::fs::write(dir.path().join("empty.bin"), b"").expect("write empty");
        let files = &[ArtifactRequirement {
            path: "empty.bin",
            sha256: None,
        }];
        assert!(matches!(
            artifact_identity(dir.path(), files),
            Err(MemoryError::Validation(_))
        ));
    }
}
