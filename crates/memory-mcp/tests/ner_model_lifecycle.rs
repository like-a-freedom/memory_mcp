//! NER model lifecycle integration tests: acquisition, leases, activation,
//! and recovery. All collaborators are fakes; nothing here touches the
//! network.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use memory_mcp::config::{ModelBackedNerConfig, NativeGlinerConfig, NerConfig, NerExtractorConfig};
use memory_mcp::service::MemoryError;
use memory_mcp::service::model_artifacts::{
    ArtifactFetcher, ArtifactRequirement, CapturingSink, Clock, ModelProgressSink, NerArtifactSpec,
    NerArtifactStore, RevisionResolver, SystemClock,
};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

// Re-exported for test ergonomics; must compile to the public contract.
const _: fn() = || {
    let _ = NerConfig {
        extractor: NerExtractorConfig::ClassicGliner(NativeGlinerConfig {
            model: ModelBackedNerConfig {
                cache_dir: None,
                labels: vec![],
                threshold: None,
                max_concurrency: 1,
                idle_unload_secs: 0,
            },
            batch_size: 1,
            max_batch_tokens: 128,
            device: memory_mcp::config::GlinerDeviceKind::Cpu,
        }),
    };
};

fn test_spec() -> NerArtifactSpec {
    NerArtifactSpec {
        extractor_id: "test-extractor",
        repository: "org/test-model",
        runtime_version: "0.1.0",
        files: &[
            ArtifactRequirement {
                path: "model.bin",
                sha256: None,
            },
            ArtifactRequirement {
                path: "config.json",
                sha256: None,
            },
        ],
        companion_repository: None,
        companion_files: &[],
    }
}

/// Spec with a companion tokenizer source from a second repository.
fn companion_spec() -> NerArtifactSpec {
    let mut spec = test_spec();
    spec.companion_repository = Some("org/base-tokenizer");
    spec.companion_files = &[ArtifactRequirement {
        path: "tokenizer.json",
        sha256: None,
    }];
    spec
}

/// A fake resolver whose behavior is controlled per test.
struct FakeResolver {
    response: std::sync::Mutex<Result<String, String>>,
    calls: AtomicUsize,
}

impl Default for FakeResolver {
    fn default() -> Self {
        Self::ok("abc123")
    }
}

impl FakeResolver {
    fn ok(revision: &str) -> Self {
        Self {
            response: std::sync::Mutex::new(Ok(revision.to_string())),
            calls: AtomicUsize::new(0),
        }
    }

    fn failing() -> Self {
        Self {
            response: std::sync::Mutex::new(Err("offline".to_string())),
            calls: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl RevisionResolver for FakeResolver {
    async fn latest(&self, repository: &str) -> Result<String, MemoryError> {
        let _ = repository;
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.response.lock().expect("resolver lock").clone() {
            Ok(revision) => Ok(revision),
            Err(message) => Err(MemoryError::Transient(message)),
        }
    }
}

/// A fake fetcher that writes the required artifacts locally.
struct FakeFetcher {
    fail_after: std::sync::Mutex<Option<usize>>,
    stall: AtomicBool,
    fetch_calls: AtomicUsize,
}

impl FakeFetcher {
    fn new() -> Self {
        Self {
            fail_after: std::sync::Mutex::new(None),
            stall: AtomicBool::new(false),
            fetch_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl ArtifactFetcher for FakeFetcher {
    async fn fetch(
        &self,
        _repository: &str,
        _revision: &str,
        requirement: &ArtifactRequirement,
        target: &std::path::Path,
        progress: &dyn ModelProgressSink,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<(), MemoryError> {
        self.fetch_calls.fetch_add(1, Ordering::SeqCst);
        if cancellation.is_cancelled() {
            return Err(MemoryError::Transient(
                "NER artifact refresh cancelled".to_string(),
            ));
        }
        {
            let mut remaining = self.fail_after.lock().expect("fail_after lock");
            if let Some(count) = remaining.as_mut().filter(|count| **count > 0) {
                *count -= 1;
                return Err(MemoryError::Storage("download failed".to_string()));
            }
        }
        if self.stall.load(Ordering::SeqCst) {
            return Err(MemoryError::Storage(
                "download stalled: no bytes for 60s".to_string(),
            ));
        }
        let bytes = format!("content-of-{}", requirement.path);
        std::fs::create_dir_all(target.parent().expect("target parent")).expect("create dirs");
        std::fs::write(target, bytes).expect("write artifact");
        progress.emit(
            &memory_mcp::service::model_artifacts::ModelProgressEvent::download(
                requirement.path,
                Some(_revision.to_string()),
                1,
                1,
                100,
            ),
        );
        Ok(())
    }
}

fn make_store(
    temp_dir: &TempDir,
    resolver: FakeResolver,
    fetcher: Arc<FakeFetcher>,
) -> (NerArtifactStore, Arc<CapturingSink>, Arc<FakeFetcher>) {
    let resolver = Arc::new(resolver);
    let (store, sink, _fetcher, _resolver) = make_store_with_resolver(temp_dir, resolver, fetcher);
    (store, sink, _fetcher)
}

fn make_store_with_resolver(
    temp_dir: &TempDir,
    resolver: Arc<FakeResolver>,
    fetcher: Arc<FakeFetcher>,
) -> (
    NerArtifactStore,
    Arc<CapturingSink>,
    Arc<FakeFetcher>,
    Arc<FakeResolver>,
) {
    let sink = Arc::new(CapturingSink::default());
    let store = NerArtifactStore::with_parts(
        temp_dir.path().join("models").join("ner"),
        resolver.clone(),
        fetcher.clone(),
        sink.clone(),
        Arc::new(SystemClock),
    );
    (store, sink, fetcher, resolver)
}

#[tokio::test]
async fn prepare_downloads_and_activates_artifacts() {
    let temp = TempDir::new().expect("temp dir");
    let (store, sink, _fetcher) = make_store(
        &temp,
        FakeResolver::ok("abc123"),
        Arc::new(FakeFetcher::new()),
    );
    let checkpoint = store.prepare(&test_spec()).await.expect("prepare");
    assert_eq!(checkpoint.repository, "org/test-model");
    assert_eq!(checkpoint.revision, "abc123");
    assert!(checkpoint.root.join("model.bin").is_file());
    assert!(checkpoint.root.join("config.json").is_file());
    assert_eq!(checkpoint.artifact_identity.len(), 64);
    assert!(!sink.events().is_empty());
}

#[tokio::test]
async fn prepare_reuses_complete_active_revision_without_fetching() {
    let temp = TempDir::new().expect("temp dir");
    let fetcher = Arc::new(FakeFetcher::new());
    let (store, _, fetcher_handle) = make_store(&temp, FakeResolver::ok("abc123"), fetcher);
    let first = store.prepare(&test_spec()).await.expect("first prepare");
    assert!(first.root.join("model.bin").is_file());

    // Second prepare with the same revision must not re-download.
    let calls_before = fetcher_handle.fetch_calls.load(Ordering::SeqCst);
    let second = store.prepare(&test_spec()).await.expect("second prepare");
    assert_eq!(second.revision, "abc123");
    assert_eq!(
        fetcher_handle.fetch_calls.load(Ordering::SeqCst),
        calls_before
    );
}

#[tokio::test]
async fn prepare_fetches_companion_files_from_second_repository() {
    let temp = TempDir::new().expect("temp dir");
    let fetcher = Arc::new(FakeFetcher::new());
    let (store, _, fetcher_handle) = make_store(&temp, FakeResolver::ok("abc123"), fetcher);
    let checkpoint = store.prepare(&companion_spec()).await.expect("prepare");
    // Primary files plus the companion tokenizer are staged and verified.
    assert!(checkpoint.root.join("model.bin").is_file());
    assert!(checkpoint.root.join("config.json").is_file());
    assert!(checkpoint.root.join("tokenizer.json").is_file());
    assert_eq!(checkpoint.artifact_identity.len(), 64);
    // Primary files (2) + companion file (1); the companion repository HEAD is
    // resolved and fetched separately.
    assert_eq!(fetcher_handle.fetch_calls.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn offline_resolve_falls_back_to_known_good() {
    let temp = TempDir::new().expect("temp dir");
    // First prepare succeeds online.
    {
        let (store, _, _) = make_store(
            &temp,
            FakeResolver::ok("abc123"),
            Arc::new(FakeFetcher::new()),
        );
        store.prepare(&test_spec()).await.expect("online prepare");
    }
    // Second store is offline; must fall back to known-good.
    let (store, sink, _) = make_store(&temp, FakeResolver::failing(), Arc::new(FakeFetcher::new()));
    let checkpoint = store.prepare(&test_spec()).await.expect("offline fallback");
    assert_eq!(checkpoint.revision, "abc123");
    assert_eq!(
        checkpoint.revision_status,
        memory_mcp::service::model_artifacts::RevisionStatus::UnverifiedLatest
    );
    assert!(
        sink.events().iter().any(|event| event.phase
            == memory_mcp::service::model_artifacts::ModelProgressPhase::Fallback)
    );
}

#[tokio::test]
async fn offline_without_cache_fails_startup() {
    let temp = TempDir::new().expect("temp dir");
    let (store, _, _) = make_store(&temp, FakeResolver::failing(), Arc::new(FakeFetcher::new()));
    let result = store.prepare(&test_spec()).await;
    assert!(matches!(result, Err(MemoryError::Storage(_))));
}

#[tokio::test]
async fn failed_download_removes_candidate_staging() {
    let temp = TempDir::new().expect("temp dir");
    let fetcher = Arc::new(FakeFetcher::new());
    *fetcher.fail_after.lock().expect("lock") = Some(1); // fail on first file
    let (store, _, _) = make_store(&temp, FakeResolver::ok("abc123"), fetcher);
    let result = store.prepare(&test_spec()).await;
    assert!(matches!(result, Err(MemoryError::Storage(_))));
    let staging_root = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("staging");
    let leftovers = std::fs::read_dir(&staging_root)
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(leftovers, 0, "staging must be cleaned after failure");
}

#[tokio::test]
async fn incompatible_revision_is_suppressed_until_head_changes() {
    let temp = TempDir::new().expect("temp dir");
    // First: activate known-good abc123.
    {
        let (store, _, _) = make_store(
            &temp,
            FakeResolver::ok("abc123"),
            Arc::new(FakeFetcher::new()),
        );
        store.prepare(&test_spec()).await.expect("good prepare");
    }
    // Second: HEAD is incompatible; record it and fall back to abc123.
    {
        let (store, _, _) = make_store(
            &temp,
            FakeResolver::ok("bad999"),
            Arc::new(FakeFetcher::new()),
        );
        let fallback = store
            .record_incompatible(&test_spec(), "bad999", "smoke probe failed")
            .await
            .expect("record incompatible");
        assert_eq!(fallback.revision, "abc123");
        assert_eq!(
            fallback.revision_status,
            memory_mcp::service::model_artifacts::RevisionStatus::LatestIncompatible
        );
    }
    // Third: HEAD is still bad999; prepare must not retry it and falls back.
    {
        let (store, sink, _) = make_store(
            &temp,
            FakeResolver::ok("bad999"),
            Arc::new(FakeFetcher::new()),
        );
        let checkpoint = store.prepare(&test_spec()).await.expect("suppressed retry");
        assert_eq!(checkpoint.revision, "abc123");
        assert_eq!(
            checkpoint.revision_status,
            memory_mcp::service::model_artifacts::RevisionStatus::LatestIncompatible
        );
        assert!(sink.events().iter().any(|event| event.phase
            == memory_mcp::service::model_artifacts::ModelProgressPhase::Fallback));
    }
}

#[tokio::test]
async fn concurrent_prepare_waiters_observe_activation() {
    let temp = TempDir::new().expect("temp dir");
    let (store, _, _) = make_store(
        &temp,
        FakeResolver::ok("abc123"),
        Arc::new(FakeFetcher::new()),
    );
    let store = Arc::new(store);
    let mut handles = Vec::new();
    for _ in 0..3 {
        let store = store.clone();
        let spec = test_spec();
        handles.push(tokio::spawn(async move { store.prepare(&spec).await }));
    }
    for handle in handles {
        let checkpoint = handle.await.expect("join").expect("prepare");
        assert_eq!(checkpoint.revision, "abc123");
        assert!(checkpoint.root.join("model.bin").is_file());
    }
}

#[tokio::test]
async fn active_revision_is_never_evicted() {
    let temp = TempDir::new().expect("temp dir");
    let (store, _, _) = make_store(
        &temp,
        FakeResolver::ok("rev-1"),
        Arc::new(FakeFetcher::new()),
    );
    store.prepare(&test_spec()).await.expect("prepare rev-1");

    // Simulate a newer revision; rev-1 becomes previous known-good.
    let (store2, _, _) = make_store(
        &temp,
        FakeResolver::ok("rev-2"),
        Arc::new(FakeFetcher::new()),
    );
    store2.prepare(&test_spec()).await.expect("prepare rev-2");

    let revisions_dir = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("revisions");
    let names: Vec<String> = std::fs::read_dir(&revisions_dir)
        .expect("revisions dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(names.contains(&"rev-1".to_string()));
    assert!(names.contains(&"rev-2".to_string()));
}

#[tokio::test]
async fn second_prepare_after_failure_retries_download() {
    let temp = TempDir::new().expect("temp dir");
    let fetcher = Arc::new(FakeFetcher::new());
    *fetcher.fail_after.lock().expect("lock") = Some(1); // fail first file
    let (store, _, _) = make_store(&temp, FakeResolver::ok("abc123"), fetcher);
    let first = store.prepare(&test_spec()).await;
    assert!(matches!(first, Err(MemoryError::Storage(_))));

    let (store2, _, _) = make_store(
        &temp,
        FakeResolver::ok("abc123"),
        Arc::new(FakeFetcher::new()),
    );
    let second = store2.prepare(&test_spec()).await.expect("retry prepare");
    assert_eq!(second.revision, "abc123");
}

#[test]
fn clock_trait_is_object_safe() {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    assert!(clock.now_secs() > 1_600_000_000);
}

#[test]
fn spec_contract_is_exact() {
    let spec = test_spec();
    assert_eq!(spec.extractor_id, "test-extractor");
    assert_eq!(spec.repository, "org/test-model");
    assert_eq!(spec.files.len(), 2);
}

// ── Typed local inspection (Task 2) ─────────────────────────────────────

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt")
        .block_on(future)
}

#[test]
fn inspect_local_empty_store_does_not_call_network_collaborators() {
    use memory_mcp::service::model_artifacts::LocalCheckpointSet;
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("abc123")),
        Arc::new(FakeFetcher::new()),
    );

    let inspected = store.inspect_local(&test_spec()).expect("inspect");
    assert!(inspected.candidate.is_none());
    assert!(inspected.known_good.is_none());
    assert!(inspected.issue.is_none());
    assert!(matches!(inspected, LocalCheckpointSet { .. }));
    assert_eq!(resolver.calls(), 0);
}

#[test]
fn inspect_local_returns_complete_known_good_without_calls() {
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("abc123")),
        Arc::new(FakeFetcher::new()),
    );
    // Seed a fully-validated known-good revision by running prepare() once.
    let checkpoint = block_on(store.prepare(&test_spec())).expect("seed prepare");
    assert!(checkpoint.root.join("model.bin").is_file());
    let resolver_calls = resolver.calls();

    let inspected = store.inspect_local(&test_spec()).expect("inspect");
    assert!(inspected.candidate.is_none());
    let known_good = inspected.known_good.expect("known-good");
    assert_eq!(known_good.revision, "abc123");
    assert!(inspected.issue.is_none());
    assert_eq!(resolver.calls(), resolver_calls);
}

#[test]
fn inspect_local_reports_incomplete_revision_without_evicting_state() {
    use memory_mcp::service::model_artifacts::LocalCheckpointIssue;
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("abc123")),
        Arc::new(FakeFetcher::new()),
    );
    let _ = block_on(store.prepare(&test_spec()));
    // Remove one of the required files.
    let revision_dir = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("revisions")
        .join("abc123");
    std::fs::remove_file(revision_dir.join("model.bin")).expect("remove file");

    let inspected = store.inspect_local(&test_spec()).expect("inspect");
    assert!(inspected.candidate.is_none());
    assert!(inspected.known_good.is_none());
    assert!(matches!(
        inspected.issue,
        Some(LocalCheckpointIssue::Incomplete { ref revision }) if revision == "abc123"
    ));
}

#[test]
fn inspect_local_reports_zero_byte_file_as_incomplete() {
    use memory_mcp::service::model_artifacts::LocalCheckpointIssue;
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("abc123")),
        Arc::new(FakeFetcher::new()),
    );
    let _ = block_on(store.prepare(&test_spec()));
    let revision_dir = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("revisions")
        .join("abc123");
    std::fs::write(revision_dir.join("model.bin"), b"").expect("zero-byte file");

    let inspected = store.inspect_local(&test_spec()).expect("inspect");
    assert!(inspected.known_good.is_none());
    assert!(matches!(
        inspected.issue,
        Some(LocalCheckpointIssue::Incomplete { ref revision }) if revision == "abc123"
    ));
}

#[test]
fn inspect_local_reports_identity_mismatch_when_bytes_change() {
    use memory_mcp::service::model_artifacts::LocalCheckpointIssue;
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("abc123")),
        Arc::new(FakeFetcher::new()),
    );
    let _ = block_on(store.prepare(&test_spec()));
    let revision_dir = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("revisions")
        .join("abc123");
    std::fs::write(revision_dir.join("model.bin"), b"tampered").expect("overwrite");

    let inspected = store.inspect_local(&test_spec()).expect("inspect");
    assert!(inspected.known_good.is_none());
    assert!(matches!(
        inspected.issue,
        Some(LocalCheckpointIssue::IdentityMismatch { ref revision }) if revision == "abc123"
    ));
}

#[test]
fn inspect_local_reports_malformed_state() {
    use memory_mcp::service::model_artifacts::LocalCheckpointIssue;
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("abc123")),
        Arc::new(FakeFetcher::new()),
    );
    std::fs::create_dir_all(
        temp.path()
            .join("models")
            .join("ner")
            .join("test-extractor"),
    )
    .expect("create extractor dir");
    std::fs::write(
        temp.path()
            .join("models")
            .join("ner")
            .join("test-extractor")
            .join("state.json"),
        "not json",
    )
    .expect("write malformed state");

    let inspected = store.inspect_local(&test_spec()).expect("inspect");
    assert!(inspected.known_good.is_none());
    assert!(matches!(
        inspected.issue,
        Some(LocalCheckpointIssue::MalformedState { .. })
    ));
}

#[test]
fn inspect_local_reports_unsupported_state_version() {
    use memory_mcp::service::model_artifacts::LocalCheckpointIssue;
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("abc123")),
        Arc::new(FakeFetcher::new()),
    );
    let dir = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor");
    std::fs::create_dir_all(&dir).expect("create dir");
    std::fs::write(
        dir.join("state.json"),
        r#"{"schema_version": 99, "revisions": []}"#,
    )
    .expect("write state");

    let inspected = store.inspect_local(&test_spec()).expect("inspect");
    assert!(inspected.known_good.is_none());
    assert!(matches!(
        inspected.issue,
        Some(LocalCheckpointIssue::UnsupportedStateVersion { found: 99 })
    ));
}

#[test]
fn inspect_local_keeps_known_good_when_candidate_record_is_unreadable() {
    use memory_mcp::service::model_artifacts::RevisionStatus;
    use memory_mcp::service::model_artifacts::ValidationStatus;
    use memory_mcp::service::model_artifacts::{
        ArtifactRole, PersistedArtifactState, RevisionState, persist_state,
    };
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("abc123")),
        Arc::new(FakeFetcher::new()),
    );
    let _ = block_on(store.prepare(&test_spec()));
    let good_dir = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("revisions")
        .join("abc123");

    // Persist a state with one known-good ("abc123") and one candidate
    // ("bad-cand") whose files are missing.
    let mut state = PersistedArtifactState::new();
    state.revisions.push(RevisionState {
        revision: "abc123".to_string(),
        artifact_identity: "identity-good".to_string(),
        validation_status: ValidationStatus::RuntimeRegressionVerified,
        revision_status: RevisionStatus::Latest,
        activated_at: 1_700_000_000,
        role: ArtifactRole::KnownGood,
        incompatible: None,
    });
    state.revisions.push(RevisionState {
        revision: "bad-cand".to_string(),
        artifact_identity: "identity-bad-cand".to_string(),
        validation_status: ValidationStatus::ReleaseParityVerified,
        revision_status: RevisionStatus::Latest,
        activated_at: 1_700_000_001,
        role: ArtifactRole::Candidate,
        incompatible: None,
    });
    let state_path = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("state.json");
    persist_state(&state_path, &state).expect("persist state");

    // Patch the known-good identity to match the actual on-disk bytes.
    let new_identity = memory_mcp::service::model_artifacts::artifact_identity(
        &good_dir,
        &[
            ArtifactRequirement {
                path: "model.bin",
                sha256: None,
            },
            ArtifactRequirement {
                path: "config.json",
                sha256: None,
            },
        ],
    )
    .expect("identity");
    let mut state = memory_mcp::service::model_artifacts::read_state(&state_path).expect("read");
    state.revisions[0].artifact_identity = new_identity;
    persist_state(&state_path, &state).expect("repersist");

    let inspected = store.inspect_local(&test_spec()).expect("inspect");
    // Bad-cand is incomplete; it should NOT be exposed as candidate.
    assert!(inspected.candidate.is_none());
    // Independently verified known-good is still returned.
    let known_good = inspected.known_good.expect("known-good");
    assert_eq!(known_good.revision, "abc123");
    assert!(matches!(
        inspected.issue,
        Some(memory_mcp::service::model_artifacts::LocalCheckpointIssue::Incomplete { ref revision })
            if revision == "bad-cand"
    ));
}

#[cfg(unix)]
#[test]
fn inspect_local_propagates_permission_error_as_storage_failure() {
    use std::os::unix::fs::PermissionsExt;
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("abc123")),
        Arc::new(FakeFetcher::new()),
    );
    // Restrict the extractor directory to be unreadable.
    let dir = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor");
    std::fs::create_dir_all(&dir).expect("create dir");
    std::fs::write(dir.join("state.json"), "{}").expect("write state");
    let original = std::fs::metadata(&dir).expect("meta").permissions();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).expect("set perms");
    let err = store
        .inspect_local(&test_spec())
        .expect_err("permission failure must be fatal");
    // Restore perms before asserting to allow cleanup.
    let _ = std::fs::set_permissions(&dir, original);
    assert!(matches!(err, MemoryError::Storage(_)));
}

// ── Candidate lifecycle (Task 3) ────────────────────────────────────────

#[test]
fn refresh_persists_candidate_without_runtime_verified_status() {
    use memory_mcp::service::model_artifacts::ArtifactRole;
    use memory_mcp::service::model_artifacts::ValidationStatus;
    use memory_mcp::service::model_artifacts::read_state;
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("candidate-1")),
        Arc::new(FakeFetcher::new()),
    );
    let outcome =
        block_on(store.refresh_candidate(&test_spec(), CancellationToken::new())).expect("refresh");
    assert!(matches!(
        outcome,
        memory_mcp::service::model_artifacts::CandidateRefreshOutcome::CandidateReady { ref revision }
            if revision == "candidate-1"
    ));

    let state_path = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("state.json");
    let state = read_state(&state_path).expect("read state");
    let candidate = state.candidate().expect("candidate present");
    assert_eq!(candidate.revision, "candidate-1");
    assert_ne!(
        candidate.validation_status,
        ValidationStatus::RuntimeRegressionVerified
    );
    assert_eq!(candidate.role, ArtifactRole::Candidate);
}

#[test]
fn refresh_returns_up_to_date_when_persisted_candidate_matches_resolved_revision() {
    use memory_mcp::service::model_artifacts::CandidateRefreshOutcome;
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("candidate-1")),
        Arc::new(FakeFetcher::new()),
    );
    let _ = block_on(store.refresh_candidate(&test_spec(), CancellationToken::new()))
        .expect("first refresh");
    let outcome = block_on(store.refresh_candidate(&test_spec(), CancellationToken::new()))
        .expect("second refresh");
    assert!(matches!(
        outcome,
        CandidateRefreshOutcome::UpToDate { ref revision } if revision == "candidate-1"
    ));
}

#[test]
fn refresh_returns_suppressed_incompatible_when_revision_is_already_incompatible() {
    use memory_mcp::service::model_artifacts::CandidateRefreshOutcome;
    let temp = TempDir::new().expect("temp dir");
    // First, seed a known-good "good-1" so record_incompatible can succeed.
    {
        let resolver = Arc::new(FakeResolver::ok("good-1"));
        let store = NerArtifactStore::with_parts(
            temp.path().join("models").join("ner"),
            resolver,
            Arc::new(FakeFetcher::new()),
            Arc::new(CapturingSink::default()),
            Arc::new(SystemClock),
        );
        let _ = block_on(store.prepare(&test_spec())).expect("seed known-good");
    }
    // Then mark "bad-1" as incompatible (with "good-1" still present as fallback).
    let resolver = Arc::new(FakeResolver::ok("bad-1"));
    let store = NerArtifactStore::with_parts(
        temp.path().join("models").join("ner"),
        resolver,
        Arc::new(FakeFetcher::new()),
        Arc::new(CapturingSink::default()),
        Arc::new(SystemClock),
    );
    let _ = block_on(store.record_incompatible(&test_spec(), "bad-1", "smoke failed"))
        .expect("record incompatible");
    let outcome = block_on(store.refresh_candidate(&test_spec(), CancellationToken::new()))
        .expect("suppressed refresh");
    assert!(matches!(
        outcome,
        CandidateRefreshOutcome::SuppressedIncompatible { ref revision } if revision == "bad-1"
    ));
}

#[test]
fn promote_candidate_changes_role_to_known_good_and_preserves_previous_known_good() {
    use memory_mcp::service::model_artifacts::{
        ArtifactRole, PreparedCheckpoint, ValidationStatus, artifact_identity, read_state,
    };
    let temp = TempDir::new().expect("temp dir");
    // Seed a known-good "old-good" via the standard prepare path.
    let resolver_a = Arc::new(FakeResolver::ok("old-good"));
    let store = NerArtifactStore::with_parts(
        temp.path().join("models").join("ner"),
        resolver_a,
        Arc::new(FakeFetcher::new()),
        Arc::new(CapturingSink::default()),
        Arc::new(SystemClock),
    );
    let _ = block_on(store.prepare(&test_spec())).expect("seed known-good");

    // New store refreshes a candidate "candidate-2".
    let resolver_b = Arc::new(FakeResolver::ok("candidate-2"));
    let store = NerArtifactStore::with_parts(
        temp.path().join("models").join("ner"),
        resolver_b,
        Arc::new(FakeFetcher::new()),
        Arc::new(CapturingSink::default()),
        Arc::new(SystemClock),
    );
    let _ =
        block_on(store.refresh_candidate(&test_spec(), CancellationToken::new())).expect("refresh");

    // Promote the candidate.
    let promoted: PreparedCheckpoint = store
        .promote_candidate(&test_spec(), "candidate-2")
        .expect("promote");
    assert_eq!(promoted.revision, "candidate-2");
    assert_eq!(
        promoted.validation_status,
        ValidationStatus::RuntimeRegressionVerified
    );
    let state_path = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("state.json");
    let state = read_state(&state_path).expect("read");
    let promoted_record = state
        .revisions
        .iter()
        .find(|r| r.revision == "candidate-2")
        .expect("promoted record");
    assert_eq!(promoted_record.role, ArtifactRole::KnownGood);
    let previous = state
        .revisions
        .iter()
        .find(|r| r.revision == "old-good")
        .expect("previous known-good kept");
    assert_eq!(previous.role, ArtifactRole::KnownGood);
    let revision_dir = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("revisions")
        .join("candidate-2");
    let recomputed = artifact_identity(
        &revision_dir,
        &test_spec().all_requirements().copied().collect::<Vec<_>>(),
    )
    .expect("identity");
    assert_eq!(promoted.artifact_identity, recomputed);
}

#[test]
fn reject_candidate_returns_previous_known_good() {
    use memory_mcp::service::model_artifacts::ArtifactRole;
    use memory_mcp::service::model_artifacts::read_state;
    let temp = TempDir::new().expect("temp dir");
    let resolver_a = Arc::new(FakeResolver::ok("good-1"));
    let store = NerArtifactStore::with_parts(
        temp.path().join("models").join("ner"),
        resolver_a,
        Arc::new(FakeFetcher::new()),
        Arc::new(CapturingSink::default()),
        Arc::new(SystemClock),
    );
    let _ = block_on(store.prepare(&test_spec())).expect("seed good-1");

    let resolver_b = Arc::new(FakeResolver::ok("bad-1"));
    let store = NerArtifactStore::with_parts(
        temp.path().join("models").join("ner"),
        resolver_b,
        Arc::new(FakeFetcher::new()),
        Arc::new(CapturingSink::default()),
        Arc::new(SystemClock),
    );
    let _ =
        block_on(store.refresh_candidate(&test_spec(), CancellationToken::new())).expect("refresh");
    let fallback = store
        .reject_candidate(&test_spec(), "bad-1", "smoke failed")
        .expect("reject")
        .expect("previous known-good present");
    assert_eq!(fallback.revision, "good-1");
    let state_path = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("state.json");
    let state = read_state(&state_path).expect("read");
    let bad_record = state
        .revisions
        .iter()
        .find(|r| r.revision == "bad-1")
        .expect("bad-1 record");
    assert_eq!(bad_record.role, ArtifactRole::Incompatible);
    let bad_dir = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("revisions")
        .join("bad-1");
    assert!(!bad_dir.exists(), "bad-1 directory must be removed");
}

#[test]
fn reject_candidate_returns_none_when_no_previous_known_good_exists() {
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("candidate-x")),
        Arc::new(FakeFetcher::new()),
    );
    let _ =
        block_on(store.refresh_candidate(&test_spec(), CancellationToken::new())).expect("refresh");
    let fallback = store
        .reject_candidate(&test_spec(), "candidate-x", "smoke failed")
        .expect("reject");
    assert!(fallback.is_none());
}

#[test]
fn candidate_is_never_returned_by_known_good_selector() {
    use memory_mcp::service::model_artifacts::read_state;
    let temp = TempDir::new().expect("temp dir");
    let (store, _sink, _fetcher, _resolver) = make_store_with_resolver(
        &temp,
        Arc::new(FakeResolver::ok("candidate-only")),
        Arc::new(FakeFetcher::new()),
    );
    let _ =
        block_on(store.refresh_candidate(&test_spec(), CancellationToken::new())).expect("refresh");
    let state_path = temp
        .path()
        .join("models")
        .join("ner")
        .join("test-extractor")
        .join("state.json");
    let state = read_state(&state_path).expect("read");
    assert!(state.known_goods().next().is_none());
    assert!(state.candidate().is_some());
}

// ── Cancellation debris (Task 4) ───────────────────────────────────────

fn store_root_for(temp: &TempDir) -> std::path::PathBuf {
    temp.path().join("models").join("ner")
}

fn assert_no_part_or_staging(temp: &TempDir) {
    let store_root = store_root_for(temp);
    let gliner_root = store_root.join("test-extractor");
    let staging = gliner_root.join("staging");
    let leases = gliner_root.join("leases");
    if staging.exists() {
        let entries: Vec<_> = std::fs::read_dir(&staging)
            .expect("read staging")
            .map_while(Result::ok)
            .collect();
        assert!(entries.is_empty(), "staging not empty: {entries:?}");
    }
    if leases.exists() {
        let entries: Vec<_> = std::fs::read_dir(&leases)
            .expect("read leases")
            .map_while(Result::ok)
            .collect();
        assert!(entries.is_empty(), "leases not empty: {entries:?}");
    }
    // Walk revisions to confirm no `.part` files survived.
    let revisions = gliner_root.join("revisions");
    if revisions.exists() {
        for entry in std::fs::read_dir(&revisions).expect("read revisions") {
            let entry = entry.expect("entry");
            let path = entry.path();
            if path.is_dir() {
                for inner in std::fs::read_dir(&path).expect("read rev") {
                    let inner = inner.expect("inner");
                    let name = inner.file_name();
                    assert!(
                        !name.to_string_lossy().ends_with(".part"),
                        ".part debris at {}",
                        inner.path().display()
                    );
                }
            }
        }
    }
}

struct BlockingFetcher {
    phase: Arc<std::sync::Mutex<BlockingPhase>>,
    started: std::sync::Mutex<bool>,
    started_notify: std::sync::Condvar,
    resume: Arc<std::sync::Mutex<bool>>,
    #[allow(dead_code)]
    resume_notify: std::sync::Condvar,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
enum BlockingPhase {
    /// Block before acquiring the lease.
    Lease,
    /// Block after lease is acquired but before any fetch call.
    PreFetch,
    /// Block after the first chunk is delivered to disk.
    MidStream,
    /// Block between the first and second file of multi-file acquisition.
    BetweenFiles,
}

impl BlockingFetcher {
    fn new(phase: BlockingPhase) -> Self {
        Self {
            phase: Arc::new(std::sync::Mutex::new(phase)),
            started: std::sync::Mutex::new(false),
            started_notify: std::sync::Condvar::new(),
            resume: Arc::new(std::sync::Mutex::new(false)),
            resume_notify: std::sync::Condvar::new(),
        }
    }
}

#[async_trait::async_trait]
impl ArtifactFetcher for BlockingFetcher {
    async fn fetch(
        &self,
        _repository: &str,
        _revision: &str,
        requirement: &ArtifactRequirement,
        target: &std::path::Path,
        _progress: &dyn ModelProgressSink,
        cancellation: &tokio_util::sync::CancellationToken,
    ) -> Result<(), MemoryError> {
        let phase = *self.phase.lock().expect("phase");
        match phase {
            BlockingPhase::Lease | BlockingPhase::PreFetch => {
                {
                    let mut started = self.started.lock().expect("started");
                    *started = true;
                }
                self.started_notify.notify_all();
                loop {
                    {
                        let resume = *self.resume.lock().expect("resume");
                        if resume {
                            break;
                        }
                    }
                    if cancellation.is_cancelled() {
                        return Err(MemoryError::Transient(
                            "NER artifact refresh cancelled".to_string(),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
            BlockingPhase::MidStream => {
                std::fs::create_dir_all(target.parent().expect("parent")).expect("create parent");
                std::fs::write(target, b"partial").expect("write partial");
                {
                    let mut started = self.started.lock().expect("started");
                    *started = true;
                }
                self.started_notify.notify_all();
                loop {
                    {
                        let resume = *self.resume.lock().expect("resume");
                        if resume {
                            break;
                        }
                    }
                    if cancellation.is_cancelled() {
                        let _ = std::fs::remove_file(target);
                        return Err(MemoryError::Transient(
                            "NER artifact refresh cancelled".to_string(),
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                let _ = std::fs::remove_file(target);
            }
            BlockingPhase::BetweenFiles => {
                std::fs::create_dir_all(target.parent().expect("parent")).expect("create parent");
                std::fs::write(target, format!("content-of-{}", requirement.path))
                    .expect("write content");
                if requirement.path == "model.bin" {
                    {
                        let mut started = self.started.lock().expect("started");
                        *started = true;
                    }
                    self.started_notify.notify_all();
                    loop {
                        {
                            let resume = *self.resume.lock().expect("resume");
                            if resume {
                                break;
                            }
                        }
                        if cancellation.is_cancelled() {
                            return Err(MemoryError::Transient(
                                "NER artifact refresh cancelled".to_string(),
                            ));
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                }
            }
        }
        Ok(())
    }
}

fn await_signal(fetcher: &BlockingFetcher) {
    let mut started = fetcher.started.lock().expect("started");
    let mut spins = 0;
    while !*started && spins < 200 {
        started = fetcher
            .started_notify
            .wait_timeout(started, std::time::Duration::from_millis(25))
            .expect("wait started")
            .0;
        spins += 1;
    }
}

#[test]
fn cancellation_during_response_wait_leaves_no_debris() {
    let temp = TempDir::new().expect("temp dir");
    let fetcher = Arc::new(BlockingFetcher::new(BlockingPhase::PreFetch));
    let store = NerArtifactStore::with_parts(
        store_root_for(&temp),
        Arc::new(FakeResolver::ok("abc123")),
        fetcher.clone(),
        Arc::new(CapturingSink::default()),
        Arc::new(SystemClock),
    );
    let cancellation = CancellationToken::new();
    let cancel_token = cancellation.clone();
    let fetcher_clone = fetcher.clone();
    std::thread::spawn(move || {
        await_signal(&fetcher_clone);
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel_token.cancel();
    });
    let result = block_on(store.refresh_candidate(&test_spec(), cancellation));
    assert!(matches!(
        result,
        Err(MemoryError::Transient(_)) | Err(MemoryError::Storage(_))
    ));
    assert_no_part_or_staging(&temp);
}

#[test]
fn cancellation_mid_stream_leaves_no_part_file() {
    let temp = TempDir::new().expect("temp dir");
    let fetcher = Arc::new(BlockingFetcher::new(BlockingPhase::MidStream));
    let store = NerArtifactStore::with_parts(
        store_root_for(&temp),
        Arc::new(FakeResolver::ok("abc123")),
        fetcher.clone(),
        Arc::new(CapturingSink::default()),
        Arc::new(SystemClock),
    );
    let cancellation = CancellationToken::new();
    let cancel_token = cancellation.clone();
    let fetcher_clone = fetcher.clone();
    std::thread::spawn(move || {
        await_signal(&fetcher_clone);
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel_token.cancel();
    });
    let result = block_on(store.refresh_candidate(&test_spec(), cancellation));
    assert!(matches!(
        result,
        Err(MemoryError::Transient(_)) | Err(MemoryError::Storage(_))
    ));
    assert_no_part_or_staging(&temp);
}

#[test]
fn cancellation_between_files_cleans_first_file_and_staging() {
    let temp = TempDir::new().expect("temp dir");
    let fetcher = Arc::new(BlockingFetcher::new(BlockingPhase::BetweenFiles));
    let store = NerArtifactStore::with_parts(
        store_root_for(&temp),
        Arc::new(FakeResolver::ok("abc123")),
        fetcher.clone(),
        Arc::new(CapturingSink::default()),
        Arc::new(SystemClock),
    );
    let cancellation = CancellationToken::new();
    let cancel_token = cancellation.clone();
    let fetcher_clone = fetcher.clone();
    std::thread::spawn(move || {
        await_signal(&fetcher_clone);
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel_token.cancel();
    });
    let result = block_on(store.refresh_candidate(&test_spec(), cancellation));
    assert!(matches!(
        result,
        Err(MemoryError::Transient(_)) | Err(MemoryError::Storage(_))
    ));
    assert_no_part_or_staging(&temp);
}

#[test]
fn refresh_cancellation_releases_lease() {
    let temp = TempDir::new().expect("temp dir");
    let fetcher = Arc::new(BlockingFetcher::new(BlockingPhase::MidStream));
    let store = NerArtifactStore::with_parts(
        store_root_for(&temp),
        Arc::new(FakeResolver::ok("abc123")),
        fetcher.clone(),
        Arc::new(CapturingSink::default()),
        Arc::new(SystemClock),
    );
    let cancellation = CancellationToken::new();
    let cancel_token = cancellation.clone();
    let fetcher_clone = fetcher.clone();
    std::thread::spawn(move || {
        await_signal(&fetcher_clone);
        std::thread::sleep(std::time::Duration::from_millis(50));
        cancel_token.cancel();
    });
    let _ = block_on(store.refresh_candidate(&test_spec(), cancellation));
    let lease_path = store_root_for(&temp)
        .join("test-extractor")
        .join("leases")
        .join("abc123.json");
    assert!(
        !lease_path.exists(),
        "lease must be released after cancellation: {}",
        lease_path.display()
    );
}
