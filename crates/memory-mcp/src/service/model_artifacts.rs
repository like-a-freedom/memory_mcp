//! Shared NER artifact lifecycle domain.
//!
//! Pure manifest, state, and progress contracts with no
//! network or filesystem side effects beyond the sinks themselves. Acquisition,
//! leases, activation, and recovery orchestration are added in a later step.
//! [`NerArtifactStore`].

pub(crate) mod download;
pub(crate) mod lease;
pub(crate) mod manifest;
pub(crate) mod progress;
pub(crate) mod state;

pub use download::{
    ArtifactFetcher, Clock, HfArtifactFetcher, HfRevisionResolver, RevisionResolver, SystemClock,
};
pub use lease::{Lease, LeaseRecord};
pub use manifest::{
    ArtifactRequirement, CandidateRefreshOutcome, LocalCheckpointIssue, LocalCheckpointSet,
    NerArtifactSpec, PreparedCheckpoint, RevisionStatus, ValidationStatus, artifact_identity,
};
pub use progress::{
    CapturingSink, CliProgressSink, JsonLineProgressSink, ModelProgressEvent, ModelProgressPhase,
    ModelProgressSink, ThrottledProgressSink,
};
pub use state::{
    ArtifactRole, IncompatibilityRecord, PersistedArtifactState, RevisionState, persist_state,
    read_state,
};

/// Embedded RU/EN/mixed runtime-regression corpus for unseen upstream
/// revisions: extraction must succeed structurally on every case. Reference
/// scores are absent by design — unseen commits are never claimed as
/// Python-parity verified (see `evals/corpora/ner/vago_release_parity.json`).
pub mod runtime {
    /// Compiled from `evals/corpora/ner/vago_runtime_regression.json` so the
    /// gate ships inside the binary and cannot drift from the checked-in
    /// corpus.
    pub const RUNTIME_REGRESSION_CORPUS: &str =
        include_str!("../../../../evals/corpora/ner/vago_runtime_regression.json");

    #[derive(Debug, serde::Deserialize)]
    pub struct RuntimeCorpusFile {
        #[serde(default)]
        pub cases: Vec<RuntimeCorpusCase>,
    }

    #[derive(Debug, serde::Deserialize)]
    pub struct RuntimeCorpusCase {
        pub id: String,
        #[serde(default)]
        pub labels: Vec<String>,
        pub text: String,
    }
}

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::service::MemoryError;

use download::{REVISION_RESOLVE_ATTEMPTS, REVISION_RESOLVE_BACKOFF, REVISION_RESOLVE_DEADLINE};

/// Directory layout inside the store root, per extractor:
/// `<root>/<extractor_id>/revisions/<revision>/`, staging under
/// `<root>/<extractor_id>/staging/`, leases under
/// `<root>/<extractor_id>/leases/`, state at
/// `<root>/<extractor_id>/state.json`.
struct ExtractorLayout {
    revisions: PathBuf,
    staging: PathBuf,
    leases: PathBuf,
    state_path: PathBuf,
}

impl ExtractorLayout {
    fn new(store_root: &Path, extractor_id: &str) -> Self {
        let root = store_root.join(extractor_id);
        Self {
            revisions: root.join("revisions"),
            staging: root.join("staging"),
            leases: root.join("leases"),
            state_path: root.join("state.json"),
        }
    }

    fn revision_dir(&self, revision: &str) -> PathBuf {
        self.revisions.join(revision)
    }
}

/// Owns revision resolution, staged acquisition, atomic activation, retention,
/// and last-known-good recovery for model-backed NER extractors.
pub struct NerArtifactStore {
    root: PathBuf,
    resolver: Arc<dyn RevisionResolver>,
    fetcher: Arc<dyn ArtifactFetcher>,
    progress: Arc<dyn ModelProgressSink>,
    clock: Arc<dyn Clock>,
}

/// Outcome of reading the local artifact state.
enum ReadStateOutcome {
    /// State was read and (if necessary) migrated to v2 semantics.
    Ok(crate::service::model_artifacts::state::PersistedArtifactState),
    /// Recoverable defect that the caller should report through
    /// [`crate::service::model_artifacts::manifest::LocalCheckpointSet::issue`].
    Recoverable(crate::service::model_artifacts::manifest::LocalCheckpointIssue),
    /// Unrecoverable I/O error (permission, unreadable directory) that the
    /// caller must surface as a startup-fatal error.
    Fatal(MemoryError),
}

impl NerArtifactStore {
    /// Creates a store with default Hugging Face resolver/fetcher and the
    /// system clock.
    pub fn new(root: PathBuf, progress: Arc<dyn ModelProgressSink>) -> Result<Self, MemoryError> {
        Ok(Self {
            resolver: Arc::new(HfRevisionResolver::new()?),
            fetcher: Arc::new(HfArtifactFetcher::new()?),
            progress,
            clock: Arc::new(SystemClock),
            root,
        })
    }

    /// Creates a store with injected collaborators for tests.
    pub fn with_parts(
        root: PathBuf,
        resolver: Arc<dyn RevisionResolver>,
        fetcher: Arc<dyn ArtifactFetcher>,
        progress: Arc<dyn ModelProgressSink>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            root,
            resolver,
            fetcher,
            progress,
            clock,
        }
    }

    /// Returns the currently activated (non-incompatible) revision for `spec`,
    /// if any, from the persisted state.
    pub fn active_revision(&self, spec: &NerArtifactSpec) -> Option<String> {
        let layout = ExtractorLayout::new(&self.root, spec.extractor_id);
        read_state(&layout.state_path).ok().and_then(|state| {
            state
                .last_known_good()
                .map(|record| record.revision.clone())
        })
    }

    /// Inspects local checkpoints for `spec` without invoking the resolver,
    /// fetcher, lease, or any remote operation.
    ///
    /// Recoverable defects (missing/zero-byte files, identity mismatch,
    /// malformed/unsupported state) are returned in [`LocalCheckpointSet::issue`]
    /// while the independently verified role is still returned if usable.
    /// Permission errors and unreadable directory I/O are propagated as
    /// [`MemoryError::Storage`].
    pub fn inspect_local(
        &self,
        spec: &NerArtifactSpec,
    ) -> Result<crate::service::model_artifacts::manifest::LocalCheckpointSet, MemoryError> {
        use crate::service::model_artifacts::manifest::LocalCheckpointSet;
        let layout = ExtractorLayout::new(&self.root, spec.extractor_id);

        // Read state through the typed envelope so unsupported future
        // schemas surface as `LocalCheckpointIssue::UnsupportedStateVersion`
        // instead of pretending the local store is empty. Permission and other
        // I/O errors are NOT recoverable, so they propagate as `Storage`.
        let state = match self.read_state_typed(&layout.state_path) {
            ReadStateOutcome::Ok(state) => state,
            ReadStateOutcome::Recoverable(issue) => {
                return Ok(LocalCheckpointSet {
                    candidate: None,
                    known_good: None,
                    issue: Some(issue),
                });
            }
            ReadStateOutcome::Fatal(err) => return Err(err),
        };

        let mut result = LocalCheckpointSet::default();

        // Candidate first; refresh writes a single candidate so the newest
        // persisted candidate is the only one we honor.
        if let Some(record) = state.candidate() {
            match self.verify_local_record(spec, &layout, record) {
                Ok(checkpoint) => result.candidate = Some(checkpoint),
                Err(issue) => result.issue = Some(issue),
            }
        }

        // Known-good selector excludes candidates by construction.
        for record in state.known_goods() {
            if result.known_good.is_some() {
                break;
            }
            match self.verify_local_record(spec, &layout, record) {
                Ok(checkpoint) => result.known_good = Some(checkpoint),
                Err(issue) => result.issue = Some(issue),
            }
        }

        Ok(result)
    }

    /// Reads state while preserving the typed defect categories. Operational
    /// I/O errors (permission denied, unreadable directory) bubble up so the
    /// caller can fail startup; recoverable state defects are reported.
    fn read_state_typed(&self, path: &std::path::Path) -> ReadStateOutcome {
        use crate::service::model_artifacts::manifest::LocalCheckpointIssue;
        use crate::service::model_artifacts::state::{
            PersistedArtifactState, PersistedArtifactStateV1, RevisionStateV1,
            STATE_SCHEMA_VERSION, SchemaVersionEnvelope,
        };
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return ReadStateOutcome::Ok(PersistedArtifactState::new());
            }
            Err(err) => {
                return ReadStateOutcome::Fatal(MemoryError::Storage(format!(
                    "cannot read artifact state {}: {err}",
                    path.display()
                )));
            }
        };
        let envelope: SchemaVersionEnvelope = match serde_json::from_slice(&bytes) {
            Ok(envelope) => envelope,
            Err(err) => {
                return ReadStateOutcome::Recoverable(LocalCheckpointIssue::MalformedState {
                    summary: err.to_string(),
                });
            }
        };
        if envelope.schema_version > STATE_SCHEMA_VERSION && envelope.schema_version != 1 {
            return ReadStateOutcome::Recoverable(LocalCheckpointIssue::UnsupportedStateVersion {
                found: envelope.schema_version,
            });
        }
        if envelope.schema_version == 1 {
            let legacy: PersistedArtifactStateV1 = match serde_json::from_slice(&bytes) {
                Ok(legacy) => legacy,
                Err(err) => {
                    return ReadStateOutcome::Recoverable(LocalCheckpointIssue::MalformedState {
                        summary: err.to_string(),
                    });
                }
            };
            return ReadStateOutcome::Ok(PersistedArtifactState {
                schema_version: STATE_SCHEMA_VERSION,
                revisions: legacy
                    .revisions
                    .into_iter()
                    .map(RevisionStateV1::into_v2)
                    .collect(),
            });
        }
        match serde_json::from_slice::<PersistedArtifactState>(&bytes) {
            Ok(state) => ReadStateOutcome::Ok(state),
            Err(err) => ReadStateOutcome::Recoverable(LocalCheckpointIssue::MalformedState {
                summary: err.to_string(),
            }),
        }
    }

    /// Verifies a single persisted record's on-disk state without touching
    /// the network. Recomputes the artifact identity, checks file presence
    /// and non-zero size, and returns a typed defect on mismatch.
    fn verify_local_record(
        &self,
        spec: &NerArtifactSpec,
        layout: &ExtractorLayout,
        record: &crate::service::model_artifacts::state::RevisionState,
    ) -> Result<
        crate::service::model_artifacts::manifest::PreparedCheckpoint,
        crate::service::model_artifacts::manifest::LocalCheckpointIssue,
    > {
        use crate::service::model_artifacts::manifest::{LocalCheckpointIssue, PreparedCheckpoint};
        let revision_dir = layout.revision_dir(&record.revision);
        if !is_complete(&revision_dir, spec) {
            return Err(LocalCheckpointIssue::Incomplete {
                revision: record.revision.clone(),
            });
        }
        let identity = match crate::service::model_artifacts::artifact_identity(
            &revision_dir,
            &spec.all_requirements().copied().collect::<Vec<_>>(),
        ) {
            Ok(identity) => identity,
            Err(_) => {
                return Err(LocalCheckpointIssue::Incomplete {
                    revision: record.revision.clone(),
                });
            }
        };
        if identity != record.artifact_identity {
            return Err(LocalCheckpointIssue::IdentityMismatch {
                revision: record.revision.clone(),
            });
        }
        Ok(PreparedCheckpoint {
            root: revision_dir,
            repository: spec.repository.to_string(),
            revision: record.revision.clone(),
            artifact_identity: identity,
            revision_status: record.revision_status,
            validation_status: record.validation_status,
        })
    }

    /// Prepares a fully validated local checkpoint for `spec`.
    ///
    /// Resolution makes [`REVISION_RESOLVE_ATTEMPTS`] attempts within
    /// [`REVISION_RESOLVE_DEADLINE`]. Downloads have no total wall-clock
    /// deadline while bytes advance. A previously activated revision that is
    /// still complete on disk is reused without downloading. When upstream is
    /// unreachable and no revision is known-good, startup fails.
    pub async fn prepare(&self, spec: &NerArtifactSpec) -> Result<PreparedCheckpoint, MemoryError> {
        let layout = ExtractorLayout::new(&self.root, spec.extractor_id);
        let state = read_state(&layout.state_path)?;

        self.emit(&ModelProgressEvent::started(
            spec.extractor_id,
            ModelProgressPhase::Resolve,
        ));

        let latest = match self.resolve_latest(spec.repository).await {
            Ok(revision) => {
                if let Some(record) = state.incompatibility_for(&revision) {
                    self.emit(&ModelProgressEvent::started(
                        spec.extractor_id,
                        ModelProgressPhase::Fallback,
                    ));
                    self.emit(&ModelProgressEvent::completed(
                        spec.extractor_id,
                        ModelProgressPhase::Fallback,
                        format!("revision {revision} previously failed: {}", record.reason),
                    ));
                    return self.prepare_known_good(
                        spec,
                        &state,
                        RevisionStatus::LatestIncompatible,
                    );
                }
                self.emit(&ModelProgressEvent::completed(
                    spec.extractor_id,
                    ModelProgressPhase::Resolve,
                    format!("latest revision {revision}"),
                ));
                revision
            }
            Err(err) => {
                // Offline fallback: use the last known-good revision.
                if state.last_known_good().is_some() {
                    self.emit(&ModelProgressEvent::started(
                        spec.extractor_id,
                        ModelProgressPhase::Fallback,
                    ));
                    self.emit(&ModelProgressEvent::completed(
                        spec.extractor_id,
                        ModelProgressPhase::Fallback,
                        format!("upstream unreachable: {err}"),
                    ));
                    return self.prepare_known_good(spec, &state, RevisionStatus::UnverifiedLatest);
                }
                return Err(MemoryError::Storage(format!(
                    "cannot resolve latest revision for {} and no known-good checkpoint exists: {err}",
                    spec.repository
                )));
            }
        };

        // Reuse the active revision when it is still complete on disk.
        let revision_dir = layout.revision_dir(&latest);
        if state
            .last_known_good()
            .is_some_and(|record| record.revision == latest)
            && is_complete(&revision_dir, spec)
        {
            return self.build_prepared(spec, &revision_dir, &latest, RevisionStatus::Latest);
        }

        // Acquire a per-revision lease; waiters observe activation.
        let lease_path = layout.leases.join(format!("{latest}.json"));
        let staging = self.stage_path(&layout, &latest);
        let lease = loop {
            // A concurrent process may have completed this revision while we
            // waited for the lease; reuse it instead of downloading again.
            if is_complete(&revision_dir, spec) {
                return self.build_prepared(spec, &revision_dir, &latest, RevisionStatus::Latest);
            }
            let record = LeaseRecord {
                extractor: spec.extractor_id.to_string(),
                revision: latest.clone(),
                pid: std::process::id(),
                created_at: self.clock.now_secs(),
                heartbeat_at: self.clock.now_secs(),
                staging: staging.clone(),
            };
            match Lease::acquire(&lease_path, &record) {
                Ok(Some(lease)) => break lease,
                Ok(None) => {
                    match Lease::read(&lease_path) {
                        Ok(Some(held)) if lease::can_reclaim(&held, self.clock.now_secs()) => {
                            // Conservative reclaim: expired heartbeat + dead process.
                            let _ = std::fs::remove_file(&lease_path);
                            continue;
                        }
                        _ => {}
                    }
                    self.emit(&ModelProgressEvent::started(
                        spec.extractor_id,
                        ModelProgressPhase::WaitForLease,
                    ));
                    tokio::time::timeout(Duration::from_secs(5), async {
                        loop {
                            if !lease_path.exists() {
                                break;
                            }
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                    })
                    .await
                    .ok();
                }
                Err(err) => return Err(err),
            }
        };

        self.emit(&ModelProgressEvent::started(
            spec.extractor_id,
            ModelProgressPhase::Download,
        ));
        std::fs::create_dir_all(&staging).map_err(|err| {
            MemoryError::Storage(format!(
                "cannot create staging {}: {err}",
                staging.display()
            ))
        })?;
        // Legacy `prepare()` path is uncancellable: a fresh never-cancelled
        // token keeps callers' behavior unchanged while exercising the same
        // cancellation-aware fetcher.
        let never_cancelled = tokio_util::sync::CancellationToken::new();
        for requirement in spec.files {
            let target = staging.join(requirement.path);
            if let Err(err) = self
                .fetcher
                .fetch(
                    spec.repository,
                    &latest,
                    requirement,
                    &target,
                    self.progress.as_ref(),
                    &never_cancelled,
                )
                .await
            {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(err);
            }
        }
        if let Some(companion) = spec.companion_repository {
            let companion_revision = self.resolve_latest(companion).await?;
            for requirement in spec.companion_files {
                let target = staging.join(requirement.path);
                if let Err(err) = self
                    .fetcher
                    .fetch(
                        companion,
                        &companion_revision,
                        requirement,
                        &target,
                        self.progress.as_ref(),
                        &never_cancelled,
                    )
                    .await
                {
                    let _ = std::fs::remove_dir_all(&staging);
                    return Err(err);
                }
            }
        }
        drop(lease); // release before verification; verification is local
        self.emit(&ModelProgressEvent::completed(
            spec.extractor_id,
            ModelProgressPhase::Download,
            "artifacts downloaded",
        ));

        self.emit(&ModelProgressEvent::started(
            spec.extractor_id,
            ModelProgressPhase::Verify,
        ));
        let identity = artifact_identity(
            &staging,
            &spec.all_requirements().copied().collect::<Vec<_>>(),
        )?;
        // Pinned per-file SHA-256 checksums (when present on a requirement) are
        // verified inside `HfArtifactFetcher::fetch` while streaming; the
        // identity below additionally covers content for the whole checkpoint.

        // Atomic activation: rename the staged dir into the revisions layout.
        std::fs::create_dir_all(&layout.revisions).map_err(|err| {
            MemoryError::Storage(format!(
                "cannot create revisions dir {}: {err}",
                layout.revisions.display()
            ))
        })?;
        if let Err(err) = std::fs::rename(&staging, &revision_dir) {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(MemoryError::Storage(format!(
                "cannot activate revision {latest}: {err}"
            )));
        }
        self.emit(&ModelProgressEvent::completed(
            spec.extractor_id,
            ModelProgressPhase::Verify,
            format!("verified identity {identity}"),
        ));

        self.activate(spec, &layout, &latest, &identity, RevisionStatus::Latest)?;
        self.retain(spec, &layout, &latest)?;

        Ok(PreparedCheckpoint {
            root: revision_dir,
            repository: spec.repository.to_string(),
            revision: latest,
            artifact_identity: identity,
            revision_status: RevisionStatus::Latest,
            validation_status: ValidationStatus::RuntimeRegressionVerified,
        })
    }

    /// Marks a revision incompatible and returns the previous known-good
    /// checkpoint, or an error when none exists.
    pub async fn record_incompatible(
        &self,
        spec: &NerArtifactSpec,
        commit: &str,
        reason: &str,
    ) -> Result<PreparedCheckpoint, MemoryError> {
        let layout = ExtractorLayout::new(&self.root, spec.extractor_id);
        let mut state = read_state(&layout.state_path)?;
        let now = self.clock.now_secs();

        // Persist commit-keyed failure metadata.
        state.revisions.retain(|record| record.revision != commit);
        state.revisions.push(RevisionState {
            revision: commit.to_string(),
            artifact_identity: String::new(),
            validation_status: ValidationStatus::RuntimeRegressionVerified,
            revision_status: RevisionStatus::LatestIncompatible,
            activated_at: now,
            role: state::ArtifactRole::Incompatible,
            incompatible: Some(state::IncompatibilityRecord {
                commit: commit.to_string(),
                reason: reason.to_string(),
                recorded_at: now,
            }),
        });
        persist_state(&layout.state_path, &state)?;

        // Remove the failed candidate artifacts.
        let revision_dir = layout.revision_dir(commit);
        let _ = std::fs::remove_dir_all(&revision_dir);

        self.emit(&ModelProgressEvent::failed(
            spec.extractor_id,
            ModelProgressPhase::SmokeTest,
            format!("revision {commit} incompatible: {reason}"),
        ));

        if state.last_known_good().is_none() {
            return Err(MemoryError::Storage(format!(
                "revision {commit} is incompatible and no known-good checkpoint exists"
            )));
        }
        let prepared = self.prepare_known_good(spec, &state, RevisionStatus::LatestIncompatible)?;
        Ok(prepared)
    }

    /// Stages a new candidate revision without promoting it.
    ///
    /// Reuses the existing resolve/lease/staging/checksum mechanics, but
    /// persists role [`state::ArtifactRole::Candidate`] and never assigns
    /// [`ValidationStatus::RuntimeRegressionVerified`]. Promotion to
    /// [`state::ArtifactRole::KnownGood`] happens only on next-start via
    /// [`Self::promote_candidate`].
    pub async fn refresh_candidate(
        &self,
        spec: &NerArtifactSpec,
        cancellation: tokio_util::sync::CancellationToken,
    ) -> Result<crate::service::model_artifacts::CandidateRefreshOutcome, MemoryError> {
        let layout = ExtractorLayout::new(&self.root, spec.extractor_id);
        let state = read_state(&layout.state_path)?;

        // If HEAD is already known-incompatible, return without retrying.
        let resolved = self
            .resolve_latest(spec.repository)
            .await
            .inspect_err(|err| {
                self.emit(&ModelProgressEvent::failed(
                    spec.extractor_id,
                    ModelProgressPhase::Resolve,
                    err.to_string(),
                ));
            })?;

        if state.incompatibility_for(&resolved).is_some() {
            self.emit(&ModelProgressEvent::completed(
                spec.extractor_id,
                ModelProgressPhase::Fallback,
                format!("suppressed already-incompatible revision {resolved}"),
            ));
            return Ok(
                crate::service::model_artifacts::CandidateRefreshOutcome::SuppressedIncompatible {
                    revision: resolved,
                },
            );
        }

        // Reuse a known-good if it already matches HEAD.
        if let Some(record) = state
            .known_goods()
            .find(|record| record.revision == resolved)
        {
            let revision_dir = layout.revision_dir(&record.revision);
            if is_complete(&revision_dir, spec) {
                self.emit(&ModelProgressEvent::completed(
                    spec.extractor_id,
                    ModelProgressPhase::Verify,
                    format!("up to date at revision {}", record.revision),
                ));
                return Ok(
                    crate::service::model_artifacts::CandidateRefreshOutcome::UpToDate {
                        revision: resolved,
                    },
                );
            }
        }

        // Skip work if a complete candidate already exists for the resolved
        // revision; refresh is idempotent for the same HEAD.
        if let Some(record) = state.candidate()
            && record.revision == resolved
        {
            let revision_dir = layout.revision_dir(&resolved);
            if is_complete(&revision_dir, spec) {
                self.emit(&ModelProgressEvent::completed(
                    spec.extractor_id,
                    ModelProgressPhase::Verify,
                    format!("up to date at candidate revision {resolved}"),
                ));
                return Ok(
                    crate::service::model_artifacts::CandidateRefreshOutcome::UpToDate {
                        revision: resolved,
                    },
                );
            }
        }

        // Acquire lease and download to a staging directory.
        let lease_path = layout.leases.join(format!("{resolved}.json"));
        let staging = self.stage_path(&layout, &resolved);
        let _lease = self
            .acquire_refresh_lease(spec, &lease_path, &resolved, &staging, &cancellation)
            .await?;

        self.emit(&ModelProgressEvent::started(
            spec.extractor_id,
            ModelProgressPhase::Download,
        ));
        std::fs::create_dir_all(&staging).map_err(|err| {
            MemoryError::Storage(format!(
                "cannot create staging {}: {err}",
                staging.display()
            ))
        })?;
        let staging_guard = StagingDirGuard::new(&staging);
        self.fetch_all_files(spec, &resolved, &staging, &cancellation)
            .await?;

        let identity = artifact_identity(
            &staging,
            &spec.all_requirements().copied().collect::<Vec<_>>(),
        )?;

        // Atomic commit into the revisions layout.
        let revision_dir = layout.revision_dir(&resolved);
        std::fs::create_dir_all(&layout.revisions).map_err(|err| {
            MemoryError::Storage(format!(
                "cannot create revisions dir {}: {err}",
                layout.revisions.display()
            ))
        })?;
        std::fs::rename(&staging, &revision_dir).map_err(|err| {
            MemoryError::Storage(format!("cannot activate revision {resolved}: {err}"))
        })?;
        // The staged directory has been atomically renamed; disarm the guard
        // so the revisions directory is not removed on drop.
        staging_guard.commit();
        drop(_lease);

        // Persist as Candidate; never set RuntimeRegressionVerified here.
        let now = self.clock.now_secs();
        let mut state = read_state(&layout.state_path)?;
        state.revisions.retain(|record| record.revision != resolved);
        state.revisions.push(RevisionState {
            revision: resolved.clone(),
            artifact_identity: identity,
            validation_status: ValidationStatus::ReleaseParityVerified,
            revision_status: RevisionStatus::Latest,
            activated_at: now,
            role: state::ArtifactRole::Candidate,
            incompatible: None,
        });
        // Keep at most one candidate.
        let mut kept = Vec::new();
        let mut candidate_kept = false;
        let mut known_kept = 0usize;
        let mut ordered: Vec<&RevisionState> = state.revisions.iter().collect();
        ordered.sort_by_key(|record| std::cmp::Reverse(record.activated_at));
        for record in ordered {
            match record.role {
                state::ArtifactRole::Candidate if !candidate_kept => {
                    kept.push(record.clone());
                    candidate_kept = true;
                }
                state::ArtifactRole::KnownGood if known_kept < 2 => {
                    kept.push(record.clone());
                    known_kept += 1;
                }
                state::ArtifactRole::Incompatible => {
                    kept.push(record.clone());
                }
                _ => {}
            }
        }
        state.revisions = kept;
        persist_state(&layout.state_path, &state)?;

        self.emit(&ModelProgressEvent::completed(
            spec.extractor_id,
            ModelProgressPhase::Activate,
            format!("staged candidate revision {resolved}"),
        ));

        Ok(
            crate::service::model_artifacts::CandidateRefreshOutcome::CandidateReady {
                revision: resolved,
            },
        )
    }

    /// Acquires a per-revision lease, blocking while another live process
    /// holds it. Cancellation is observed before each retry sleep.
    async fn acquire_refresh_lease(
        &self,
        spec: &NerArtifactSpec,
        lease_path: &std::path::Path,
        revision: &str,
        staging: &std::path::Path,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<Lease, MemoryError> {
        loop {
            if cancellation.is_cancelled() {
                return Err(MemoryError::Transient(
                    "NER artifact refresh cancelled".to_string(),
                ));
            }
            let record = LeaseRecord {
                extractor: spec.extractor_id.to_string(),
                revision: revision.to_string(),
                pid: std::process::id(),
                created_at: self.clock.now_secs(),
                heartbeat_at: self.clock.now_secs(),
                staging: staging.to_path_buf(),
            };
            match Lease::acquire(lease_path, &record) {
                Ok(Some(lease)) => return Ok(lease),
                Ok(None) => {
                    // Check whether we can reclaim an expired heartbeat.
                    if let Ok(Some(held)) = Lease::read(lease_path)
                        && lease::can_reclaim(&held, self.clock.now_secs())
                    {
                        let _ = std::fs::remove_file(lease_path);
                        continue;
                    }
                    self.emit(&ModelProgressEvent::started(
                        spec.extractor_id,
                        ModelProgressPhase::WaitForLease,
                    ));
                    tokio::select! {
                        _ = cancellation.cancelled() => {
                            return Err(MemoryError::Transient(
                                "NER artifact refresh cancelled".to_string(),
                            ));
                        }
                        _ = async {
                            loop {
                                if !lease_path.exists() {
                                    break;
                                }
                                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                            }
                        } => {}
                    }
                }
                Err(err) => return Err(err),
            }
        }
    }

    /// Fetches all primary and companion files into `staging`, observing
    /// cancellation between each requirement.
    async fn fetch_all_files(
        &self,
        spec: &NerArtifactSpec,
        revision: &str,
        staging: &std::path::Path,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<(), MemoryError> {
        for requirement in spec.files {
            if cancellation.is_cancelled() {
                return Err(MemoryError::Transient(
                    "NER artifact refresh cancelled".to_string(),
                ));
            }
            let target = staging.join(requirement.path);
            self.fetcher
                .fetch(
                    spec.repository,
                    revision,
                    requirement,
                    &target,
                    self.progress.as_ref(),
                    cancellation,
                )
                .await?;
        }
        if let Some(companion) = spec.companion_repository {
            let companion_revision = self.resolve_latest(companion).await?;
            for requirement in spec.companion_files {
                if cancellation.is_cancelled() {
                    return Err(MemoryError::Transient(
                        "NER artifact refresh cancelled".to_string(),
                    ));
                }
                let target = staging.join(requirement.path);
                self.fetcher
                    .fetch(
                        companion,
                        &companion_revision,
                        requirement,
                        &target,
                        self.progress.as_ref(),
                        cancellation,
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Promotes the named candidate to role [`state::ArtifactRole::KnownGood`]
    /// with [`ValidationStatus::RuntimeRegressionVerified`]. Fails when the
    /// record is not a candidate, the revision does not match, or the
    /// on-disk artifact identity diverges.
    pub fn promote_candidate(
        &self,
        spec: &NerArtifactSpec,
        revision: &str,
    ) -> Result<crate::service::model_artifacts::PreparedCheckpoint, MemoryError> {
        let layout = ExtractorLayout::new(&self.root, spec.extractor_id);
        let mut state = read_state(&layout.state_path)?;
        let record = state
            .candidate()
            .filter(|record| record.revision == revision)
            .ok_or_else(|| {
                MemoryError::Storage(format!(
                    "no Candidate revision {revision} for {}",
                    spec.repository
                ))
            })?
            .clone();
        let revision_dir = layout.revision_dir(&record.revision);
        if !is_complete(&revision_dir, spec) {
            return Err(MemoryError::Storage(format!(
                "candidate revision {revision} files are incomplete"
            )));
        }
        let identity = artifact_identity(
            &revision_dir,
            &spec.all_requirements().copied().collect::<Vec<_>>(),
        )?;
        if identity != record.artifact_identity {
            return Err(MemoryError::Storage(format!(
                "candidate revision {revision} identity mismatch"
            )));
        }
        let now = self.clock.now_secs();
        // Update the existing record's role; do not duplicate it.
        for entry in state.revisions.iter_mut() {
            if entry.revision == revision {
                entry.role = state::ArtifactRole::KnownGood;
                entry.validation_status = ValidationStatus::RuntimeRegressionVerified;
                entry.activated_at = now;
                entry.artifact_identity = identity.clone();
            }
        }
        // Retain: at most one Candidate plus the two most recent KnownGood
        // entries; always keep all Incompatibility records.
        state
            .revisions
            .sort_by_key(|record| std::cmp::Reverse(record.activated_at));
        let mut kept: Vec<RevisionState> = Vec::new();
        let mut candidate_kept = false;
        let mut known_kept = 0usize;
        for entry in state.revisions.drain(..) {
            match entry.role {
                state::ArtifactRole::Candidate if !candidate_kept => {
                    kept.push(entry);
                    candidate_kept = true;
                }
                state::ArtifactRole::KnownGood if known_kept < 2 => {
                    kept.push(entry);
                    known_kept += 1;
                }
                state::ArtifactRole::Incompatible => {
                    kept.push(entry);
                }
                _ => {}
            }
        }
        state.revisions = kept;
        persist_state(&layout.state_path, &state)?;
        self.retain(spec, &layout, &record.revision)?;
        Ok(crate::service::model_artifacts::PreparedCheckpoint {
            root: revision_dir,
            repository: spec.repository.to_string(),
            revision: record.revision,
            artifact_identity: identity,
            revision_status: record.revision_status,
            validation_status: ValidationStatus::RuntimeRegressionVerified,
        })
    }

    /// Marks the named candidate as incompatible, removes its artifact
    /// directory, and returns the newest identity-verified known-good
    /// checkpoint when present.
    pub fn reject_candidate(
        &self,
        spec: &NerArtifactSpec,
        revision: &str,
        reason: &str,
    ) -> Result<Option<crate::service::model_artifacts::PreparedCheckpoint>, MemoryError> {
        let layout = ExtractorLayout::new(&self.root, spec.extractor_id);
        let mut state = read_state(&layout.state_path)?;
        let now = self.clock.now_secs();
        let mut found_candidate = false;
        for entry in state.revisions.iter_mut() {
            if entry.revision == revision && entry.role == state::ArtifactRole::Candidate {
                entry.role = state::ArtifactRole::Incompatible;
                // The candidate's static validation status is preserved:
                // a runtime probe failure does not manufacture
                // `RuntimeRegressionVerified` (Global Constraints). The
                // incompatibility record and the role change already
                // document the rejection.
                entry.activated_at = now;
                entry.artifact_identity = String::new();
                entry.incompatible = Some(state::IncompatibilityRecord {
                    commit: revision.to_string(),
                    reason: reason.to_string(),
                    recorded_at: now,
                });
                found_candidate = true;
            }
        }
        if !found_candidate {
            return Err(MemoryError::Storage(format!(
                "no Candidate revision {revision} to reject"
            )));
        }
        persist_state(&layout.state_path, &state)?;
        let revision_dir = layout.revision_dir(revision);
        let _ = std::fs::remove_dir_all(&revision_dir);
        // Find the newest known-good record and verify it on disk.
        let known_good_record = state
            .known_goods()
            .next()
            .filter(|record| is_complete(&layout.revision_dir(&record.revision), spec))
            .cloned();
        if let Some(record) = known_good_record {
            let revision_dir = layout.revision_dir(&record.revision);
            let identity = artifact_identity(
                &revision_dir,
                &spec.all_requirements().copied().collect::<Vec<_>>(),
            )?;
            return Ok(Some(crate::service::model_artifacts::PreparedCheckpoint {
                root: revision_dir,
                repository: spec.repository.to_string(),
                revision: record.revision,
                artifact_identity: identity,
                revision_status: record.revision_status,
                validation_status: record.validation_status,
            }));
        }
        Ok(None)
    }

    /// Prepares the persisted last known-good revision.
    fn prepare_known_good(
        &self,
        spec: &NerArtifactSpec,
        state: &PersistedArtifactState,
        revision_status: RevisionStatus,
    ) -> Result<PreparedCheckpoint, MemoryError> {
        let record = state.last_known_good().ok_or_else(|| {
            MemoryError::Storage(format!("no known-good revision for {}", spec.repository))
        })?;
        let layout = ExtractorLayout::new(&self.root, spec.extractor_id);
        let revision_dir = layout.revision_dir(&record.revision);
        if !is_complete(&revision_dir, spec) {
            return Err(MemoryError::Storage(format!(
                "known-good revision {} artifacts are incomplete",
                record.revision
            )));
        }
        let identity = artifact_identity(
            &revision_dir,
            &spec.all_requirements().copied().collect::<Vec<_>>(),
        )?;
        self.emit(&ModelProgressEvent::completed(
            spec.extractor_id,
            ModelProgressPhase::Fallback,
            format!("using known-good revision {}", record.revision),
        ));
        Ok(PreparedCheckpoint {
            root: revision_dir,
            repository: spec.repository.to_string(),
            revision: record.revision.clone(),
            artifact_identity: identity,
            revision_status,
            validation_status: record.validation_status,
        })
    }

    /// Resolves the latest revision with bounded retries and a total deadline.
    async fn resolve_latest(&self, repository: &str) -> Result<String, MemoryError> {
        let deadline = tokio::time::Instant::now() + REVISION_RESOLVE_DEADLINE;
        let mut last_err: Option<MemoryError> = None;
        for attempt in 0..REVISION_RESOLVE_ATTEMPTS {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, self.resolver.latest(repository)).await {
                Ok(Ok(revision)) => return Ok(revision),
                Ok(Err(err)) => last_err = Some(err),
                Err(_) => {
                    last_err = Some(MemoryError::Transient(
                        "revision lookup exceeded the total deadline".to_string(),
                    ))
                }
            }
            if attempt + 1 < REVISION_RESOLVE_ATTEMPTS {
                tokio::time::sleep(REVISION_RESOLVE_BACKOFF).await;
            }
        }
        Err(last_err
            .unwrap_or_else(|| MemoryError::Transient("revision lookup failed".to_string())))
    }

    /// Persists activation and refreshes the retention window.
    fn activate(
        &self,
        spec: &NerArtifactSpec,
        layout: &ExtractorLayout,
        revision: &str,
        identity: &str,
        revision_status: RevisionStatus,
    ) -> Result<(), MemoryError> {
        let mut state = read_state(&layout.state_path)?;
        let now = self.clock.now_secs();
        state.revisions.retain(|record| record.revision != revision);
        state.revisions.push(RevisionState {
            revision: revision.to_string(),
            artifact_identity: identity.to_string(),
            validation_status: ValidationStatus::RuntimeRegressionVerified,
            revision_status,
            activated_at: now,
            role: state::ArtifactRole::KnownGood,
            incompatible: None,
        });
        state
            .revisions
            .sort_by_key(|record| std::cmp::Reverse(record.activated_at));
        // Never evict active; keep active + one previous known-good.
        let mut kept = Vec::new();
        for record in state.revisions {
            if record.revision == revision || !record.incompatible.is_some() {
                kept.push(record);
            }
            if kept.len() >= 2 {
                break;
            }
        }
        state.revisions = kept;
        persist_state(&layout.state_path, &state)?;
        self.emit(&ModelProgressEvent::completed(
            spec.extractor_id,
            ModelProgressPhase::Activate,
            format!("activated revision {revision}"),
        ));
        Ok(())
    }

    /// Removes revisions that are neither active nor the previous known-good.
    fn retain(
        &self,
        spec: &NerArtifactSpec,
        layout: &ExtractorLayout,
        active: &str,
    ) -> Result<(), MemoryError> {
        let state = read_state(&layout.state_path)?;
        let retained: std::collections::HashSet<String> = state
            .revisions
            .iter()
            .map(|record| record.revision.clone())
            .collect();
        if let Ok(entries) = std::fs::read_dir(&layout.revisions) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name != active && !retained.contains(&name) {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        let _ = spec;
        Ok(())
    }

    fn stage_path(&self, layout: &ExtractorLayout, revision: &str) -> PathBuf {
        layout.staging.join(format!(
            "{revision}-{}-{}",
            std::process::id(),
            self.clock.now_secs()
        ))
    }

    fn emit(&self, event: &ModelProgressEvent) {
        self.progress.emit(event);
    }

    fn build_prepared(
        &self,
        spec: &NerArtifactSpec,
        revision_dir: &Path,
        revision: &str,
        revision_status: RevisionStatus,
    ) -> Result<PreparedCheckpoint, MemoryError> {
        let identity = artifact_identity(
            revision_dir,
            &spec.all_requirements().copied().collect::<Vec<_>>(),
        )?;
        Ok(PreparedCheckpoint {
            root: revision_dir.to_path_buf(),
            repository: spec.repository.to_string(),
            revision: revision.to_string(),
            artifact_identity: identity,
            revision_status,
            validation_status: ValidationStatus::RuntimeRegressionVerified,
        })
    }
}

fn is_complete(root: &Path, spec: &NerArtifactSpec) -> bool {
    // Primary AND companion files must be present: a staged checkpoint missing
    // its companion tokenizer is not reusable and must not bypass re-fetch.
    spec.all_requirements()
        .all(|requirement| root.join(requirement.path).is_file())
}

/// RAII guard that removes a staged artifact directory on drop unless
/// [`StagingDirGuard::commit`] is called. A future call that would have
/// renamed the staged directory into `revisions/` must disarm the guard
/// only after the rename succeeds.
pub(crate) struct StagingDirGuard {
    path: PathBuf,
    committed: bool,
}

impl StagingDirGuard {
    pub(crate) fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            committed: false,
        }
    }

    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for StagingDirGuard {
    fn drop(&mut self) {
        if self.committed {
            return;
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
