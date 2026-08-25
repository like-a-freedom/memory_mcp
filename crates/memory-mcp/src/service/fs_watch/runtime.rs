//! Tracked filesystem-ingestion runtime owned by the stdio MCP server.
//!
//! Startup validates and attaches the OS watcher before launching a
//! non-blocking background scan; both funnel into the durable inbox revision
//! store. A sequential processor drains the store. The watcher is recreated
//! with bounded backoff and then enters a logged degraded state. Shutdown is
//! bounded to 30 seconds for the in-flight revision.
//!
//! The runtime is started by `run_stdio_server` in a later task; until then its
//! types are exercised only by tests, so dead-code analysis is relaxed.
#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::fs_watch::FsWatchConfig;
use crate::error::MemoryError;
use crate::models::inbox_revision::{InboxRevisionLease, InboxRevisionRecord};
use crate::service::fs_watch::candidate::{
    CandidateOutcome, CandidateSkipReason, PreparedInboxRevision, prepare_candidate,
};
use crate::service::fs_watch::processor::InboxRevisionProcessor;
use crate::service::fs_watch::telemetry::FsWatchTelemetry;
use crate::service::{MemoryService, deterministic_episode_id_v2};
use crate::storage::InboxRevisionStoreClient;
use crate::storage::inbox_revision_store::new_revision_record;

/// Startup generation marker for requeueing failed revisions once per start.
fn startup_generation() -> String {
    format!("startup-{}", std::process::id())
}

/// Result of bounded shutdown.
#[derive(Debug)]
pub struct FsWatchShutdownOutcome {
    /// Seconds waited for the in-flight revision before aborting its task.
    pub waited_secs: u64,
    /// Whether the in-flight lease was released via `release_interrupted`.
    pub lease_released: bool,
}

/// Tracks the processor's current lease and its stop-dequeue token.
pub struct ProcessorRuntime {
    stop_dequeue: CancellationToken,
    current_lease: Arc<tokio::sync::Mutex<Option<InboxRevisionLease>>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

/// Owns the watcher, scanner, and processor tasks.
pub struct FsWatchRuntime {
    stop_discovery: CancellationToken,
    watcher_handle: tokio::task::JoinHandle<()>,
    scanner_handle: tokio::task::JoinHandle<()>,
    processor: ProcessorRuntime,
    db_client: Arc<dyn crate::storage::DbClient>,
    namespace: String,
}

/// Event bridge: sends `notify::Result<Event>` into a Tokio channel, recreates
/// the watcher with bounded backoff on failure, then degrades.
fn spawn_event_bridge(
    inbox: std::path::PathBuf,
    store: InboxRevisionStoreClient,
    service: MemoryService,
    telemetry: FsWatchTelemetry,
    stop_discovery: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut backoff_attempts = 0u32;
        loop {
            if stop_discovery.is_cancelled() {
                return;
            }
            match run_watcher_cycle(&inbox, &store, &stop_discovery).await {
                Ok(()) => return,
                Err(_) => {
                    backoff_attempts += 1;
                    const MAX_BACKOFF_ATTEMPTS: u32 = 5;
                    if backoff_attempts >= MAX_BACKOFF_ATTEMPTS {
                        telemetry.set_degraded(true);
                        service.logger.log(
                            crate::service::log_event(
                                "fs_watch.degraded",
                                serde_json::json!({}),
                                serde_json::json!({"status": "watcher_backend_exhausted"}),
                                None,
                                None,
                                None,
                            ),
                            crate::logging::LogLevel::Warn,
                        );
                        return;
                    }
                    let delay_ms = 500u64 << backoff_attempts.min(4);
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
            }
        }
    })
}

/// One watcher backend lifetime: attach, forward events, exit on channel close.
async fn run_watcher_cycle(
    inbox: &Path,
    store: &InboxRevisionStoreClient,
    stop_discovery: &CancellationToken,
) -> Result<(), MemoryError> {
    let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(256);
    let mut watcher = RecommendedWatcher::new(
        move |result| {
            let _ = tx.try_send(result);
        },
        Config::default().with_follow_symlinks(false),
    )
    .map_err(|err| {
        MemoryError::Storage(format!("failed to initialize filesystem watcher: {err}"))
    })?;
    watcher
        .watch(inbox, RecursiveMode::Recursive)
        .map_err(|err| {
            MemoryError::Storage(format!(
                "failed to watch inbox directory `{}`: {err}",
                inbox.display()
            ))
        })?;

    loop {
        tokio::select! {
            _ = stop_discovery.cancelled() => return Ok(()),
            result = rx.recv() => {
                match result {
                    Some(Ok(event)) => handle_watch_event(store, inbox, &event).await,
                    Some(Err(_)) => {
                        return Err(MemoryError::Transient("watcher backend event error".to_string()));
                    }
                    None => return Ok(()),
                }
            }
        }
    }
}

/// Dispatches one watcher event through the shared discover→prepare→persist
/// path.
async fn handle_watch_event(store: &InboxRevisionStoreClient, inbox: &Path, event: &Event) {
    if !(event.kind.is_create() || event.kind.is_modify()) {
        return;
    }
    for path in &event.paths {
        discover_path(store, inbox, path).await;
    }
}

/// Shared discovery pipeline for one candidate path.
async fn discover_path(store: &InboxRevisionStoreClient, inbox: &Path, path: &Path) {
    let config = FsWatchConfig {
        inbox: inbox.to_path_buf(),
    };
    let cancel = CancellationToken::new();
    let Ok(CandidateOutcome::Ready(prepared)) =
        prepare_candidate(&config, path, None, &cancel).await
    else {
        return;
    };
    let record = build_record(&prepared);
    let _ = store.discover_prepared(&record).await;
}

/// Recursive startup scan of existing supported files.
async fn run_startup_scan(
    inbox: &Path,
    store: InboxRevisionStoreClient,
    telemetry: FsWatchTelemetry,
    stop_discovery: CancellationToken,
) {
    let mut pending = vec![inbox.to_path_buf()];
    while let Some(dir) = pending.pop() {
        if stop_discovery.is_cancelled() {
            return;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(file_type) => file_type,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                if !file_type.is_symlink() {
                    pending.push(path);
                }
                continue;
            }
            if file_type.is_symlink() {
                telemetry.record_scan_file("skipped_symlink");
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let config = FsWatchConfig {
                inbox: inbox.to_path_buf(),
            };
            let cancel = CancellationToken::new();
            match prepare_candidate(&config, &path, None, &cancel).await {
                Ok(CandidateOutcome::Ready(prepared)) => {
                    telemetry.record_scan_file("enqueued");
                    let record = build_record(&prepared);
                    let _ = store.discover_prepared(&record).await;
                }
                Ok(CandidateOutcome::Skipped(CandidateSkipReason::Symlink)) => {
                    telemetry.record_scan_file("skipped_symlink");
                }
                Ok(CandidateOutcome::Skipped(CandidateSkipReason::UnsupportedFormat)) => {
                    telemetry.record_scan_file("skipped_unsupported");
                }
                Ok(CandidateOutcome::Skipped(CandidateSkipReason::NotRegularFile)) => {
                    telemetry.record_scan_file("skipped_not_regular");
                }
                Ok(CandidateOutcome::Skipped(CandidateSkipReason::Interrupted)) => {
                    telemetry.record_scan_file("interrupted");
                }
                Ok(CandidateOutcome::Skipped(CandidateSkipReason::OutsideRoot)) => {}
                Err(_) => telemetry.record_scan_file("failed_read"),
            }
        }
    }
}

/// Builds a durable record from a prepared revision.
fn build_record(prepared: &PreparedInboxRevision) -> InboxRevisionRecord {
    new_revision_record(
        prepared.lineage.clone(),
        prepared.relative_path.clone(),
        prepared.content_sha256.clone(),
        prepared.source_type.clone(),
        prepared.t_ref,
        prepared.prepared_content.clone(),
        deterministic_episode_id_v2(&prepared.source_type, &prepared.source_id, prepared.t_ref),
        chrono::Utc::now(),
    )
}

impl FsWatchRuntime {
    /// Validates backend attachment synchronously, then spawns the event
    /// bridge, startup scan, and sequential processor.
    pub async fn start(service: MemoryService, config: FsWatchConfig) -> Result<Self, MemoryError> {
        let telemetry = FsWatchTelemetry::new();
        let store = InboxRevisionStoreClient::new(
            service.db_client.clone(),
            service.active_namespace.clone(),
        );

        // Synchronously validate that the watcher can attach before the MCP
        // transport is ready.
        let mut probe =
            RecommendedWatcher::new(|_| {}, Config::default().with_follow_symlinks(false))
                .map_err(|err| {
                    MemoryError::ConfigInvalid(format!(
                        "failed to initialize filesystem watcher for `{}`: {err}",
                        config.inbox.display()
                    ))
                })?;
        probe
            .watch(&config.inbox, RecursiveMode::Recursive)
            .map_err(|err| {
                MemoryError::ConfigInvalid(format!(
                    "failed to watch inbox `{}`: {err}",
                    config.inbox.display()
                ))
            })?;
        drop(probe);

        // Recovery: requeue failed revisions once per startup generation and
        // requeue expired leases from a previous crash.
        let generation = startup_generation();
        let _ = store.requeue_failed_for_startup(&generation).await;
        let _ = store.requeue_expired_leases().await;

        service.logger.log(
            crate::service::log_event(
                "fs_watch.ready",
                serde_json::json!({"inbox": config.inbox.display().to_string()}),
                serde_json::json!({"status": "listening"}),
                None,
                None,
                None,
            ),
            crate::logging::LogLevel::Info,
        );

        let stop_discovery = CancellationToken::new();
        let stop_dequeue = CancellationToken::new();
        let current_lease = Arc::new(tokio::sync::Mutex::new(None));

        // Watcher-first: the event bridge attaches before the scan starts.
        let watcher_handle = spawn_event_bridge(
            config.inbox.clone(),
            store.clone(),
            service.clone(),
            telemetry.clone(),
            stop_discovery.clone(),
        );

        let scan_store = store.clone();
        let scan_telemetry = telemetry.clone();
        let scan_stop = stop_discovery.clone();
        let scan_inbox = config.inbox.clone();
        let scanner_handle = tokio::spawn(async move {
            run_startup_scan(&scan_inbox, scan_store, scan_telemetry, scan_stop).await;
        });

        // Sequential processor with a separate stop-dequeue token.
        let processor_handle = tokio::spawn({
            let processor_store = store.clone();
            let processor_service = service.clone();
            let processor_telemetry = telemetry.clone();
            let processor_stop = stop_dequeue.clone();
            let processor_lease = current_lease.clone();
            async move {
                let processor = InboxRevisionProcessor::new(
                    processor_store,
                    processor_service,
                    processor_telemetry,
                    processor_stop,
                    processor_lease,
                );
                processor.run().await;
            }
        });

        Ok(Self {
            stop_discovery,
            watcher_handle,
            scanner_handle,
            processor: ProcessorRuntime {
                stop_dequeue,
                current_lease,
                handle: Some(processor_handle),
            },
            db_client: service.db_client.clone(),
            namespace: service.active_namespace.clone(),
        })
    }

    /// Bounded shutdown: stop discovery immediately, stop dequeue, grant the
    /// in-flight revision up to 30 seconds, then abort and release.
    pub async fn shutdown(self) -> FsWatchShutdownOutcome {
        self.stop_discovery.cancel();
        self.processor.stop_dequeue.cancel();

        let watcher_handle = self.watcher_handle;
        let scanner_handle = self.scanner_handle;
        let mut processor = self.processor;
        let processor_handle = match processor.handle.take() {
            Some(handle) => handle,
            // The processor task is always spawned by `start`; a missing handle
            // means shutdown raced construction and there is nothing to await.
            None => {
                return FsWatchShutdownOutcome {
                    waited_secs: 0,
                    lease_released: true,
                };
            }
        };
        let db_client = self.db_client;
        let namespace = self.namespace;

        let wait_result = tokio::time::timeout(Duration::from_secs(30), async {
            let _ = watcher_handle.await;
            let _ = scanner_handle.await;
            let _ = processor_handle.await;
        })
        .await;

        match wait_result {
            Ok(()) => FsWatchShutdownOutcome {
                waited_secs: 0,
                lease_released: true,
            },
            Err(_elapsed) => {
                // Timeout: abort only the outer processor task and try to
                // release its current lease via compare-and-set. If ownership
                // cannot be proved, the row stays leased for expiry.
                let lease = processor.current_lease.lock().await.clone();
                if let Some(handle) = processor.handle.take() {
                    handle.abort();
                }
                let lease_released = if let Some(lease) = lease {
                    let store = InboxRevisionStoreClient::new(db_client, namespace);
                    store
                        .release_interrupted(&lease.revision_id, &lease.owner)
                        .await
                        .is_ok()
                } else {
                    false
                };
                FsWatchShutdownOutcome {
                    waited_secs: 30,
                    lease_released,
                }
            }
        }
    }
}
