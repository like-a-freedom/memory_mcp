//! Persisted artifact-store state (last-known-good and failure records).
//!
//! The state file is a JSON document written atomically via a sibling temp
//! file, `sync_all`, then rename. It records which revisions were activated,
//! which are incompatible, and what remains on disk so retention and
//! recovery are deterministic across processes.

use std::path::Path;

use serde::Deserialize;

use crate::service::MemoryError;

use super::manifest::{RevisionStatus, ValidationStatus};

/// Version of the persisted state schema.
pub const STATE_SCHEMA_VERSION: u8 = 2;

/// Durable role of one revision in the artifact lifecycle.
///
/// A revision begins as [`ArtifactRole::Candidate`] once static acquisition
/// succeeds; only successful next-start runtime validation promotes it to
/// [`ArtifactRole::KnownGood`]. A failed runtime validation moves it to
/// [`ArtifactRole::Incompatible`]. Known-good revisions are the only ones
/// returned by the ordinary selectors at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRole {
    /// Staged revision awaiting next-start runtime validation.
    Candidate,
    /// Runtime-validated, fully usable revision.
    KnownGood,
    /// Revision marked incompatible after a runtime failure; never reused.
    Incompatible,
}

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
    /// Durable lifecycle role.
    pub role: ArtifactRole,
    /// When `Some`, this revision is known-incompatible and must not be retried.
    #[serde(default)]
    pub incompatible: Option<IncompatibilityRecord>,
}

/// Schema-v1 revision record, used only for in-memory migration.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RevisionStateV1 {
    revision: String,
    artifact_identity: String,
    validation_status: ValidationStatus,
    revision_status: RevisionStatus,
    activated_at: i64,
    #[serde(default)]
    incompatible: Option<IncompatibilityRecord>,
}

impl RevisionStateV1 {
    /// Promotes a schema-v1 record to schema-v2 semantics.
    pub(crate) fn into_v2(self) -> RevisionState {
        let role = if self.incompatible.is_some() {
            ArtifactRole::Incompatible
        } else {
            ArtifactRole::KnownGood
        };
        RevisionState {
            revision: self.revision,
            artifact_identity: self.artifact_identity,
            validation_status: self.validation_status,
            revision_status: self.revision_status,
            activated_at: self.activated_at,
            role,
            incompatible: self.incompatible,
        }
    }
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
    /// Most recent first; contains candidate, known-good, and incompatibility entries.
    #[serde(default)]
    pub revisions: Vec<RevisionState>,
}

/// Schema-v1 envelope, used only to inspect the wire schema version and to
/// migrate legacy records. The struct mirrors the original on-disk shape.
#[derive(Debug, Deserialize)]
pub(crate) struct PersistedArtifactStateV1 {
    pub(crate) schema_version: u8,
    #[serde(default)]
    pub(crate) revisions: Vec<RevisionStateV1>,
}

impl PersistedArtifactState {
    /// Creates an empty state at the current schema version.
    pub fn new() -> Self {
        Self {
            schema_version: STATE_SCHEMA_VERSION,
            revisions: Vec::new(),
        }
    }

    /// Returns the single role-`Candidate` revision, if exactly one is
    /// persisted. Refresh always writes a single candidate, so multiples are
    /// treated as a state defect (returned through the typed local inspection
    /// rather than silently selecting a different one).
    pub fn candidate(&self) -> Option<&RevisionState> {
        self.revisions.iter().find(|record| record.role == ArtifactRole::Candidate)
    }

    /// Returns non-incompatible, non-candidate revisions ordered most-recent
    /// first. The ordinary known-good selector only considers records with
    /// role [`ArtifactRole::KnownGood`].
    pub fn known_goods(&self) -> impl Iterator<Item = &RevisionState> {
        self.revisions
            .iter()
            .filter(|record| record.role == ArtifactRole::KnownGood)
    }

    /// Returns the active (most recent role-`KnownGood`) revision state.
    pub(crate) fn last_known_good(&self) -> Option<&RevisionState> {
        self.known_goods().next()
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
/// the file is absent. Schema-v1 files migrate in memory to v2 semantics
/// without rewriting the file on disk; every successful write emits
/// [`STATE_SCHEMA_VERSION`].
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

    // Inspect the wire schema version first so an unsupported future schema
    // does not get silently re-deserialized into a structurally compatible v2.
    let envelope: SchemaVersionEnvelope = serde_json::from_slice(&bytes).map_err(|err| {
        MemoryError::Storage(format!("invalid artifact state {}: {err}", path.display()))
    })?;
    match envelope.schema_version {
        1 => {
            let legacy: PersistedArtifactStateV1 = serde_json::from_slice(&bytes).map_err(|err| {
                MemoryError::Storage(format!(
                    "invalid artifact state v1 {}: {err}",
                    path.display()
                ))
            })?;
            Ok(PersistedArtifactState {
                schema_version: STATE_SCHEMA_VERSION,
                revisions: legacy
                    .revisions
                    .into_iter()
                    .map(RevisionStateV1::into_v2)
                    .collect(),
            })
        }
        STATE_SCHEMA_VERSION => {
            let state: PersistedArtifactState =
                serde_json::from_slice(&bytes).map_err(|err| {
                    MemoryError::Storage(format!(
                        "invalid artifact state {}: {err}",
                        path.display()
                    ))
                })?;
            Ok(state)
        }
        other => Err(MemoryError::Storage(format!(
            "artifact state {} has unsupported schema version {} (expected {})",
            path.display(),
            other,
            STATE_SCHEMA_VERSION
        ))),
    }
}

#[derive(Deserialize)]
pub(crate) struct SchemaVersionEnvelope {
    pub(crate) schema_version: u8,
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

    fn sample_revision(revision: &str, role: ArtifactRole, activated_at: i64) -> RevisionState {
        RevisionState {
            revision: revision.to_string(),
            artifact_identity: format!("identity-{revision}"),
            validation_status: match role {
                ArtifactRole::KnownGood => ValidationStatus::RuntimeRegressionVerified,
                _ => ValidationStatus::ReleaseParityVerified,
            },
            revision_status: RevisionStatus::Latest,
            activated_at,
            role,
            incompatible: None,
        }
    }

    fn sample_state() -> PersistedArtifactState {
        let mut state = PersistedArtifactState::new();
        state.revisions.push(sample_revision(
            "abc123",
            ArtifactRole::KnownGood,
            1_700_000_000,
        ));
        state
    }

    #[test]
    fn state_round_trips_through_persist_and_read() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("state.json");
        let state = sample_state();
        persist_state(&path, &state).expect("persist");
        let loaded = read_state(&path).expect("read");
        assert_eq!(loaded.schema_version, STATE_SCHEMA_VERSION);
        assert_eq!(loaded.revisions.len(), 1);
        assert_eq!(loaded.revisions[0].revision, "abc123");
        assert_eq!(
            loaded.revisions[0].validation_status,
            ValidationStatus::RuntimeRegressionVerified
        );
        assert_eq!(loaded.revisions[0].role, ArtifactRole::KnownGood);
    }

    #[test]
    fn missing_state_reads_as_empty() {
        let dir = TempDir::new().expect("temp dir");
        let loaded = read_state(&dir.path().join("absent.json")).expect("read missing");
        assert_eq!(loaded.revisions.len(), 0);
        assert!(loaded.last_known_good().is_none());
    }

    #[test]
    fn last_known_good_skips_incompatible_and_candidate_revisions() {
        let mut state = PersistedArtifactState::new();
        state.revisions.push(sample_revision(
            "bad",
            ArtifactRole::Incompatible,
            0,
        ));
        state.revisions.push(RevisionState {
            revision: "bad".to_string(),
            artifact_identity: "id".to_string(),
            validation_status: ValidationStatus::RuntimeRegressionVerified,
            revision_status: RevisionStatus::LatestIncompatible,
            activated_at: 0,
            role: ArtifactRole::Incompatible,
            incompatible: Some(IncompatibilityRecord {
                commit: "bad".to_string(),
                reason: "smoke probe failed".to_string(),
                recorded_at: 0,
            }),
        });
        state.revisions.push(sample_revision(
            "candidate",
            ArtifactRole::Candidate,
            1,
        ));
        state.revisions.push(sample_revision(
            "good",
            ArtifactRole::KnownGood,
            2,
        ));
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

    // ── Schema-v2 role migration (Task 1) ──────────────────────────────

    #[test]
    fn schema_v1_non_incompatible_records_migrate_to_known_good() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{
              "schema_version": 1,
              "revisions": [{
                "revision": "old-good",
                "artifact_identity": "abc",
                "validation_status": "runtime_regression_verified",
                "revision_status": "latest",
                "activated_at": 10,
                "incompatible": null
              }]
            }"#,
        )
        .expect("write state");

        let state = read_state(&path).expect("read v1 state");
        assert_eq!(state.schema_version, STATE_SCHEMA_VERSION);
        assert_eq!(
            state
                .known_goods()
                .map(|r| r.revision.as_str())
                .collect::<Vec<_>>(),
            vec!["old-good"]
        );
        assert!(state.candidate().is_none());
    }

    #[test]
    fn schema_v1_incompatible_records_migrate_to_incompatible_role() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{
              "schema_version": 1,
              "revisions": [{
                "revision": "bad",
                "artifact_identity": "abc",
                "validation_status": "runtime_regression_verified",
                "revision_status": "latest_incompatible",
                "activated_at": 10,
                "incompatible": {
                  "commit": "bad",
                  "reason": "smoke probe failed",
                  "recorded_at": 10
                }
              }]
            }"#,
        )
        .expect("write state");

        let state = read_state(&path).expect("read v1 state");
        assert_eq!(state.known_goods().count(), 0);
        let record = state
            .revisions
            .iter()
            .find(|record| record.revision == "bad")
            .expect("bad revision migrated");
        assert_eq!(record.role, ArtifactRole::Incompatible);
        assert!(state.incompatibility_for("bad").is_some());
    }

    #[test]
    fn schema_v2_candidate_is_never_returned_as_known_good() {
        let mut state = PersistedArtifactState::new();
        state.revisions.push(sample_revision("candidate", ArtifactRole::Candidate, 20));
        state.revisions.push(sample_revision("known-good", ArtifactRole::KnownGood, 10));

        assert_eq!(state.candidate().map(|r| r.revision.as_str()), Some("candidate"));
        assert_eq!(
            state.known_goods().next().map(|r| r.revision.as_str()),
            Some("known-good")
        );
    }

    #[test]
    fn schema_v2_round_trip_persists_role_and_schema_version() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("state.json");
        let mut state = PersistedArtifactState::new();
        state.revisions.push(sample_revision("candidate", ArtifactRole::Candidate, 20));
        state.revisions.push(sample_revision("known-good", ArtifactRole::KnownGood, 10));
        persist_state(&path, &state).expect("persist");

        let raw = std::fs::read_to_string(&path).expect("read raw");
        assert!(raw.contains("\"schema_version\": 2"));
        assert!(raw.contains("\"role\": \"candidate\""));
        assert!(raw.contains("\"role\": \"known_good\""));

        let loaded = read_state(&path).expect("read");
        assert_eq!(loaded.schema_version, 2);
        assert_eq!(loaded.revisions.len(), 2);
        assert_eq!(loaded.candidate().map(|r| r.revision.as_str()), Some("candidate"));
        assert_eq!(
            loaded.known_goods().next().map(|r| r.revision.as_str()),
            Some("known-good")
        );
    }

    #[test]
    fn unsupported_state_version_is_rejected_as_typed_error() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            r#"{
              "schema_version": 7,
              "revisions": []
            }"#,
        )
        .expect("write state");
        let err = read_state(&path).expect_err("unsupported schema must be rejected");
        assert!(matches!(err, MemoryError::Storage(_)));
    }
}
