//! Persisted artifact-store state (last-known-good and failure records).
//!
//! The state file is a JSON document written atomically via a sibling temp
//! file, `sync_all`, then rename. It records which revisions were activated,
//! which are incompatible, and what remains on disk so retention and
//! recovery are deterministic across processes.

use std::path::Path;

use crate::service::MemoryError;

use super::manifest::{RevisionStatus, ValidationStatus};

/// Version of the persisted state schema.
pub const STATE_SCHEMA_VERSION: u8 = 1;

/// One revision's durable record.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RevisionState {
    /// Resolved upstream revision (commit hash or tag).
    pub revision: String,
    /// Stable content identity over sorted `path:size:sha256` entries.
    pub artifact_identity: String,
    /// Validation status when this revision was activated.
    pub validation_status: ValidationStatus,
    /// How this revision was resolved at activation time.
    pub revision_status: RevisionStatus,
    /// Unix epoch seconds when this revision was activated.
    pub activated_at: i64,
    /// When `Some`, this revision is known-incompatible and must not be retried.
    #[serde(default)]
    pub incompatible: Option<IncompatibilityRecord>,
}

/// Why a revision was rejected, keyed by commit so it is not retried
/// until upstream HEAD changes or the record is cleared.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IncompatibilityRecord {
    /// The rejected commit hash.
    pub commit: String,
    /// Reason recorded at rejection time.
    pub reason: String,
    /// Unix epoch seconds when the record was created.
    pub recorded_at: i64,
}

/// The full persisted state document.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PersistedArtifactState {
    pub schema_version: u8,
    /// Most recent first; contains active + retained previous known-good entries.
    #[serde(default)]
    pub revisions: Vec<RevisionState>,
}

impl PersistedArtifactState {
    /// Creates an empty state at the current schema version.
    pub fn new() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            revisions: Vec::new(),
        }
    }

    /// Returns the active (most recent non-incompatible) revision state.
    pub(crate) fn last_known_good(&self) -> Option<&RevisionState> {
        self.revisions
            .iter()
            .find(|state| state.incompatible.is_none())
    }

    /// Returns the incompatibility record for a commit, if present.
    pub(crate) fn incompatibility_for(&self, commit: &str) -> Option<&IncompatibilityRecord> {
        self.revisions
            .iter()
            .find_map(|state| match &state.incompatible {
                Some(record) if record.commit == commit => Some(record),
                _ => None,
            })
    }
}

/// Reads and validates the persisted state, returning an empty state when
/// the file is absent.
pub fn read_state(path: &Path) -> Result<PersistedArtifactState, MemoryError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(PersistedArtifactState::new());
        }
        Err(err) => {
            return Err(MemoryError::Storage(format!(
                "cannot read artifact state {}: {err}",
                path.display()
            )));
        }
    };
    let state: PersistedArtifactState = serde_json::from_slice(&bytes).map_err(|err| {
        MemoryError::Storage(format!("invalid artifact state {}: {err}", path.display()))
    })?;
    if state.schema_version != STATE_SCHEMA_VERSION {
        return Err(MemoryError::Storage(format!(
            "artifact state {} has unsupported schema version {} (expected {})",
            path.display(),
            state.schema_version,
            STATE_SCHEMA_VERSION
        )));
    }
    Ok(state)
}

/// Persists state atomically: write a sibling temp file, `sync_all`, rename.
pub fn persist_state(path: &Path, state: &PersistedArtifactState) -> Result<(), MemoryError> {
    let parent = path.parent().ok_or_else(|| {
        MemoryError::Storage(format!(
            "artifact state path {} has no parent",
            path.display()
        ))
    })?;
    std::fs::create_dir_all(parent).map_err(|err| {
        MemoryError::Storage(format!(
            "cannot create artifact state directory {}: {err}",
            parent.display()
        ))
    })?;
    let temp = parent.join(format!(
        ".{}.tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("state"),
        std::process::id()
    ));
    let json = serde_json::to_vec_pretty(state)
        .map_err(|err| MemoryError::Storage(format!("cannot serialize artifact state: {err}")))?;
    {
        let mut file = std::fs::File::create(&temp).map_err(|err| {
            MemoryError::Storage(format!(
                "cannot create artifact state temp {}: {err}",
                temp.display()
            ))
        })?;
        use std::io::Write;
        file.write_all(&json).map_err(|err| {
            MemoryError::Storage(format!(
                "cannot write artifact state temp {}: {err}",
                temp.display()
            ))
        })?;
        file.sync_all().map_err(|err| {
            MemoryError::Storage(format!(
                "cannot sync artifact state temp {}: {err}",
                temp.display()
            ))
        })?;
    }
    std::fs::rename(&temp, path).map_err(|err| {
        MemoryError::Storage(format!(
            "cannot activate artifact state {}: {err}",
            path.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_state() -> PersistedArtifactState {
        let mut state = PersistedArtifactState::new();
        state.revisions.push(RevisionState {
            revision: "abc123".to_string(),
            artifact_identity: "identity-a".to_string(),
            validation_status: ValidationStatus::ReleaseParityVerified,
            revision_status: RevisionStatus::Latest,
            activated_at: 1_700_000_000,
            incompatible: None,
        });
        state
    }

    #[test]
    fn state_round_trips_through_persist_and_read() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("state.json");
        let state = sample_state();
        persist_state(&path, &state).expect("persist");
        let loaded = read_state(&path).expect("read");
        assert_eq!(loaded.schema_version, 1);
        assert_eq!(loaded.revisions.len(), 1);
        assert_eq!(loaded.revisions[0].revision, "abc123");
        assert_eq!(
            loaded.revisions[0].validation_status,
            ValidationStatus::ReleaseParityVerified
        );
    }

    #[test]
    fn missing_state_reads_as_empty() {
        let dir = TempDir::new().expect("temp dir");
        let loaded = read_state(&dir.path().join("absent.json")).expect("read missing");
        assert_eq!(loaded.revisions.len(), 0);
        assert!(loaded.last_known_good().is_none());
    }

    #[test]
    fn last_known_good_skips_incompatible_revisions() {
        let mut state = PersistedArtifactState::new();
        state.revisions.push(RevisionState {
            revision: "bad".to_string(),
            artifact_identity: "id".to_string(),
            validation_status: ValidationStatus::RuntimeRegressionVerified,
            revision_status: RevisionStatus::LatestIncompatible,
            activated_at: 0,
            incompatible: Some(IncompatibilityRecord {
                commit: "bad".to_string(),
                reason: "smoke probe failed".to_string(),
                recorded_at: 0,
            }),
        });
        state.revisions.push(RevisionState {
            revision: "good".to_string(),
            artifact_identity: "id".to_string(),
            validation_status: ValidationStatus::ReleaseParityVerified,
            revision_status: RevisionStatus::Latest,
            activated_at: 1,
            incompatible: None,
        });
        assert_eq!(
            state.last_known_good().map(|r| r.revision.as_str()),
            Some("good")
        );
        assert!(state.incompatibility_for("bad").is_some());
        assert!(state.incompatibility_for("good").is_none());
    }

    #[test]
    fn state_with_wrong_schema_version_is_rejected() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("state.json");
        std::fs::write(&path, r#"{"schema_version": 99, "revisions": []}"#).expect("write state");
        assert!(matches!(read_state(&path), Err(MemoryError::Storage(_))));
    }

    #[test]
    fn malformed_state_is_rejected() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("state.json");
        std::fs::write(&path, "not json").expect("write state");
        assert!(matches!(read_state(&path), Err(MemoryError::Storage(_))));
    }

    #[test]
    fn persist_is_atomic_and_leaves_no_temp_file() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("state.json");
        persist_state(&path, &sample_state()).expect("persist");
        let entries = std::fs::read_dir(dir.path())
            .expect("read dir")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        assert_eq!(entries.len(), 1);
    }
}
