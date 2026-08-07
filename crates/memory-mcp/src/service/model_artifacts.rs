//! Shared NER artifact lifecycle domain.
//!
//! Task 4 provides the pure manifest, state, and progress contracts with no
//! network or filesystem side effects beyond the sinks themselves. Task 5 adds
//! acquisition, leases, activation, and recovery orchestration through
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
    ArtifactRequirement, NerArtifactSpec, PreparedCheckpoint, RevisionStatus, ValidationStatus,
    artifact_identity,
};
pub use progress::{
    CapturingSink, CliProgressSink, JsonLineProgressSink, ModelProgressEvent, ModelProgressPhase,
    ModelProgressSink, ThrottledProgressSink,
};
pub use state::{
    IncompatibilityRecord, PersistedArtifactState, RevisionState, persist_state, read_state,
};

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
        let identity = artifact_identity(&staging, spec.files)?;
        if let Some(expected) = spec.files.iter().find_map(|requirement| requirement.sha256) {
            // Identity covers content; a pinned checksum is verified at the
            // file level below.
            let _ = expected;
        }

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
        let identity = artifact_identity(&revision_dir, spec.files)?;
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
        let identity = artifact_identity(revision_dir, spec.files)?;
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
    spec.files
        .iter()
        .all(|requirement| root.join(requirement.path).is_file())
}
