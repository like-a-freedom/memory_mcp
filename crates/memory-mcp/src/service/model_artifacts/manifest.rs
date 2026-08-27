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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArtifactRequirement {
    /// Path relative to the checkpoint root, e.g. `pytorch_model.bin`.
    pub path: &'static str,
    /// Optional expected SHA-256; verified when present.
    pub sha256: Option<&'static str>,
}

/// Everything needed to fetch and validate one extractor's artifacts.
#[derive(Debug, Clone)]
pub struct NerArtifactSpec {
    /// Stable extractor identity, e.g. `vago-lfm2.5-gliner`.
    pub extractor_id: &'static str,
    /// Hugging Face repository, e.g. `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`.
    pub repository: &'static str,
    /// Required files and their optional pinned checksums.
    pub files: &'static [ArtifactRequirement],
    /// Optional companion repository for files that do not live in
    /// `repository` (e.g. a base-model `tokenizer.json` referenced by the
    /// model config). Companion files are fetched from the companion
    /// repository's own HEAD, staged with the primary files, and included in
    /// completeness and identity checks. The tokenizer is stable in practice,
    /// so companion drift does not re-key the primary revision.
    pub companion_repository: Option<&'static str>,
    /// Files to fetch from `companion_repository` when it is set.
    pub companion_files: &'static [ArtifactRequirement],
    /// Runtime/model-family version recorded in fingerprints.
    pub runtime_version: &'static str,
}

impl NerArtifactSpec {
    /// All required files: primary plus companion. Used for completeness and
    /// identity checks so a staged checkpoint cannot activate missing pieces.
    pub fn all_requirements(&self) -> impl Iterator<Item = &ArtifactRequirement> {
        self.files.iter().chain(self.companion_files.iter())
    }
}

/// How trustworthy the resolved upstream revision is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RevisionStatus {
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
pub enum ValidationStatus {
    /// Matches a release-known Python reference on RU/EN/mixed fixtures.
    ReleaseParityVerified,
    /// Passed the embedded RU/EN runtime regression corpus only.
    RuntimeRegressionVerified,
}

/// A fully prepared, validated local checkpoint ready for a backend loader.
#[derive(Debug, Clone)]
pub struct PreparedCheckpoint {
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

/// Recoverable defect observed while inspecting a persisted local checkpoint.
///
/// Permission errors and unreadable directory I/O are not represented here:
/// they are propagated as `MemoryError::Storage` because background refresh
/// cannot safely repair an inaccessible store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LocalCheckpointIssue {
    /// Required files are missing or zero-byte on disk.
    Incomplete { revision: String },
    /// Recomputed artifact identity differs from the persisted identity.
    IdentityMismatch { revision: String },
    /// State JSON could not be parsed.
    MalformedState { summary: String },
    /// State JSON carried a schema version this build cannot interpret.
    UnsupportedStateVersion { found: u8 },
}

/// Result of inspecting a local extractor store without network access.
///
/// At most one candidate and one known-good checkpoint are returned, even if
/// multiple records of the same role exist. A non-`None` [`Self::issue`]
/// reports a recoverably bad record so the caller can decide whether the
/// independently verified role is still usable.
#[derive(Debug, Clone, Default)]
pub struct LocalCheckpointSet {
    /// Staged revision awaiting next-start runtime validation.
    pub candidate: Option<PreparedCheckpoint>,
    /// Latest runtime-verified revision.
    pub known_good: Option<PreparedCheckpoint>,
    /// Sanitized defect description, when at least one record was bad.
    pub issue: Option<LocalCheckpointIssue>,
}

/// Outcome of one background `refresh_candidate` attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateRefreshOutcome {
    /// Resolved HEAD already matches the on-disk known-good or candidate.
    UpToDate { revision: String },
    /// A new candidate was staged and persisted.
    CandidateReady { revision: String },
    /// The resolved HEAD is already known-incompatible; no work performed.
    SuppressedIncompatible { revision: String },
}

/// Computes a stable SHA-256 identity from actual on-disk artifacts.
///
/// Entries are sorted by relative path; each is rendered as
/// `path:size:sha256`. A missing or zero-byte required file is an error.
pub fn artifact_identity(
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
