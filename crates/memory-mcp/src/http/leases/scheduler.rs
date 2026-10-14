//! Process scheduler.
//!
//! Each cycle discovers due work through the registry. Each
//! job is responsible for acquiring a datastore-time lease,
//! heartbeating while its bounded pass runs, and releasing
//! only its own lease. App Session cleanup, retry, and
//! subscription/outbox jobs are registered alongside the
//! provisioning job through `with_additional_job`.
//!
//! Constructing hooks with an empty job list returns a
//! configuration error: there is no implicit "do nothing"
//! scheduler.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::error::MemoryError;
use crate::http::registry::RegistryHandle;
use crate::logging::LogLevel;
use crate::observability::record_job_metric;

pub type JobFuture = Pin<Box<dyn Future<Output = Result<(), MemoryError>> + Send>>;

/// Fixed maintenance dimensions and their cadence. Parameterized lifecycle
/// windows remain bounded because only the interval differs, never the label.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MaintenanceJob {
    Provisioning,
    DeletionRecovery,
    TaskDispatch,
    RuntimeEviction,
    AppSessionCleanup,
    PhysicalCleanup,
    SubscriptionOutbox,
    UsageReconcile,
    RegistryNamespaceAudit,
    RegistryRateBucketCleanup,
    EmbeddingBackfill,
    LifecycleDecay(u64),
    LifecycleArchival(u64),
}

impl MaintenanceJob {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Provisioning => "provisioning",
            Self::DeletionRecovery => "deletion_recovery",
            Self::TaskDispatch => "task_dispatch",
            Self::RuntimeEviction => "runtime_eviction",
            Self::AppSessionCleanup => "app_session_cleanup",
            Self::PhysicalCleanup => "physical_cleanup",
            Self::SubscriptionOutbox => "subscription_outbox",
            Self::UsageReconcile => "usage_reconcile",
            Self::RegistryNamespaceAudit => "registry_namespace_audit",
            Self::RegistryRateBucketCleanup => "registry_rate_bucket_cleanup",
            Self::EmbeddingBackfill => "embedding_backfill",
            Self::LifecycleDecay(_) => "lifecycle_decay",
            Self::LifecycleArchival(_) => "lifecycle_archival",
        }
    }

    pub const fn cadence(self) -> Duration {
        match self {
            Self::Provisioning
            | Self::DeletionRecovery
            | Self::TaskDispatch
            | Self::RuntimeEviction
            | Self::SubscriptionOutbox => Duration::from_secs(1),
            Self::LifecycleDecay(seconds) | Self::LifecycleArchival(seconds) => {
                Duration::from_secs(seconds)
            }
            Self::AppSessionCleanup
            | Self::PhysicalCleanup
            | Self::UsageReconcile
            | Self::RegistryNamespaceAudit
            | Self::EmbeddingBackfill => Duration::from_secs(60),
            Self::RegistryRateBucketCleanup => Duration::from_secs(300),
        }
    }

    /// These multi-tenant passes report their own duration and degraded
    /// tenant outcome. The scheduler still records dispatch lifecycle events,
    /// but must not add a second background-pass sample for the same work.
    const fn records_background_pass(self) -> bool {
        matches!(
            self,
            Self::TaskDispatch
                | Self::PhysicalCleanup
                | Self::EmbeddingBackfill
                | Self::LifecycleDecay(_)
                | Self::LifecycleArchival(_)
        )
    }
}

/// Instance-owned cadence and fair tenant-page cursors. A process restart
/// resets both: the first pass is due and tenant discovery starts at page one.
#[derive(Default)]
pub struct MaintenanceSchedule {
    completed_at: Mutex<HashMap<MaintenanceJob, std::time::Instant>>,
    tenant_cursors: Mutex<HashMap<MaintenanceJob, Option<String>>>,
}

impl MaintenanceSchedule {
    pub fn due(&self, job: MaintenanceJob, now: std::time::Instant) -> bool {
        let completed_at = self
            .completed_at
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        completed_at.get(&job).is_none_or(|previous| {
            now.checked_duration_since(*previous)
                .is_none_or(|elapsed| elapsed >= job.cadence())
        })
    }

    pub fn record_completed(&self, job: MaintenanceJob, now: std::time::Instant) {
        self.completed_at
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(job, now);
    }

    pub async fn next_ready_tenants(
        &self,
        job: MaintenanceJob,
        store: &dyn crate::http::registry::storage::TenantStore,
        page_size: usize,
    ) -> Result<Vec<crate::http::registry::models::Tenant>, MemoryError> {
        let limit = page_size.clamp(1, 100);
        let cursor = self
            .tenant_cursors
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&job)
            .cloned()
            .flatten();
        let mut page = store.list_ready_tenants(cursor.as_deref(), limit).await?;
        if page.is_empty() && cursor.is_some() {
            page = store.list_ready_tenants(None, limit).await?;
        }
        let next_cursor = page.last().map(|tenant| tenant.id.clone());
        self.tenant_cursors
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(job, next_cursor);
        Ok(page)
    }
}

type SchedulerAction =
    Arc<dyn Fn(RegistryHandle, Arc<MaintenanceSchedule>) -> JobFuture + Send + Sync>;

#[derive(Clone)]
pub struct SchedulerJob {
    category: MaintenanceJob,
    action: SchedulerAction,
}

impl SchedulerJob {
    pub fn new<F>(category: MaintenanceJob, action: F) -> Self
    where
        F: Fn(RegistryHandle, Arc<MaintenanceSchedule>) -> JobFuture + Send + Sync + 'static,
    {
        Self {
            category,
            action: Arc::new(action),
        }
    }

    pub async fn invoke(
        &self,
        registry: RegistryHandle,
        schedule: Arc<MaintenanceSchedule>,
    ) -> Result<(), MemoryError> {
        (self.action)(registry, schedule).await
    }
}

/// Stable owner identity shared by all fenced workers in one process. Deployments
/// should set `MEMORY_MCP_HTTP_REPLICA_ID` to a durable replica identity; the PID
/// fallback is unique for the process lifetime and remains safe after restart.
pub fn replica_id() -> String {
    std::env::var("MEMORY_MCP_HTTP_REPLICA_ID")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("memory-mcp-{}", std::process::id()))
}

#[derive(Clone)]
pub struct SchedulerHooks {
    jobs: Arc<Vec<SchedulerJob>>,
    maintenance_parallelism: usize,
    schedule: Arc<MaintenanceSchedule>,
}

impl SchedulerHooks {
    pub fn new(
        jobs: Vec<SchedulerJob>,
        maintenance_parallelism: usize,
    ) -> Result<Self, MemoryError> {
        if jobs.is_empty() || maintenance_parallelism == 0 {
            return Err(MemoryError::ConfigInvalid(
                "scheduler requires at least one job and positive parallelism".into(),
            ));
        }
        Ok(Self {
            jobs: Arc::new(jobs),
            maintenance_parallelism,
            schedule: Arc::new(MaintenanceSchedule::default()),
        })
    }

    /// The only production provisioning-hook construction path. The
    /// migration adapter and fault injector come from the startup
    /// composition (ADR-0053); the scheduler never selects either
    /// itself.
    pub fn with_provisioning_only(
        migrations: Arc<dyn crate::http::leases::migration::ApplyMigrations>,
        fault_injector: Arc<dyn crate::platform::fault_injection::FaultInjector>,
    ) -> Result<Self, MemoryError> {
        let provisioning_injector = Arc::clone(&fault_injector);
        let provisioning = SchedulerJob::new(MaintenanceJob::Provisioning, move |registry, _| {
            let migrations = Arc::clone(&migrations);
            let injector = Arc::clone(&provisioning_injector);
            Box::pin(crate::http::leases::migration::run_due_provisioning(
                registry, migrations, injector,
            ))
        });
        Self::new(vec![provisioning], 4)
    }

    /// Tasks 7–9 call this before the binary starts serving
    /// to add their cleanup/retry/outbox jobs. The returned
    /// value is immutable thereafter (the inner Vec is in
    /// `Arc`, so `with_additional_job` rebuilds a new
    /// `SchedulerHooks` rather than mutating the old one).
    pub fn with_maintenance_parallelism(
        mut self,
        maintenance_parallelism: usize,
    ) -> Result<Self, MemoryError> {
        if maintenance_parallelism == 0 {
            return Err(MemoryError::ConfigInvalid(
                "scheduler maintenance parallelism must be positive".into(),
            ));
        }
        self.maintenance_parallelism = maintenance_parallelism;
        Ok(self)
    }

    pub fn with_additional_job(self, job: SchedulerJob) -> Self {
        let mut jobs = (*self.jobs).clone();
        jobs.push(job);
        Self {
            jobs: Arc::new(jobs),
            maintenance_parallelism: self.maintenance_parallelism,
            schedule: self.schedule,
        }
    }
}

pub struct SchedulerHandle {
    join: tokio::task::JoinHandle<()>,
}

pub fn start(
    registry: RegistryHandle,
    hooks: SchedulerHooks,
    shutdown: CancellationToken,
) -> SchedulerHandle {
    let join = tokio::spawn(run_scheduler(registry, hooks, shutdown));
    SchedulerHandle { join }
}

impl SchedulerHandle {
    pub async fn join(self) {
        if let Err(error) = self.join.await {
            // The scheduler task itself failed — not one of its jobs, the loop
            // that runs them. Nothing else will report it: there is no request,
            // and a job that never ran is indistinguishable from one that was
            // never scheduled.
            log_scheduler(
                "http.scheduler.failed",
                "error",
                &error.to_string(),
                LogLevel::Error,
            );
        }
    }
}

async fn run_scheduler(
    registry: RegistryHandle,
    hooks: SchedulerHooks,
    shutdown: CancellationToken,
) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = ticker.tick() => run_cycle(registry.clone(), &hooks, shutdown.clone()).await,
        }
    }
}

async fn run_cycle(registry: RegistryHandle, hooks: &SchedulerHooks, shutdown: CancellationToken) {
    let semaphore = Arc::new(Semaphore::new(hooks.maintenance_parallelism));
    let mut jobs: JoinSet<()> = JoinSet::new();
    for job in hooks.jobs.iter().cloned() {
        let semaphore = semaphore.clone();
        let registry = registry.clone();
        let shutdown = shutdown.clone();
        let schedule = Arc::clone(&hooks.schedule);
        jobs.spawn(async move {
            let category = job.category;
            crate::observability::record_maintenance_scheduled(category.label());
            let permit = tokio::select! {
                // Shutdown wins, deterministically. Without `biased` this
                // picks a ready branch at random, and after the token is
                // cancelled *both* branches are ready — so a job would
                // sometimes acquire a permit and run during shutdown while
                // the log said it had not. The line has to state a fact the
                // code establishes, not one it merely allows.
                biased;
                _ = shutdown.cancelled() => {
                    // Both ways a job can be scheduled and then not run were
                    // silent. "It ran" and "it did not" are different facts,
                    // and only the second one was missing: a job that was
                    // scheduled, then skipped on shutdown, looked exactly like
                    // a job that was never scheduled.
                    log_scheduler("http.job.not_run", "reason", "runtime_shutdown", LogLevel::Debug);
                    crate::observability::record_maintenance_skipped(category.label(), "runtime_shutdown");
                    return;
                }
                permit = semaphore.acquire_owned() => match permit {
                    Ok(permit) => permit,
                    Err(_) => {
                        // The semaphore is closed, which happens on shutdown.
                        log_scheduler("http.job.not_run", "reason", "runtime_shutdown", LogLevel::Debug);
                        crate::observability::record_maintenance_skipped(category.label(), "runtime_shutdown");
                        return;
                    }
                },
            };
            let _permit = permit;
            let now = std::time::Instant::now();
            if !schedule.due(category, now) {
                crate::observability::record_maintenance_skipped(category.label(), "cadence");
                return;
            }
            // Measured from the moment the job actually starts running, not
            // from when it was scheduled: a job that waited on the semaphore
            // would otherwise report the queue's delay as its own work.
            let started = std::time::Instant::now();
            let outcome = match (job.action)(registry, Arc::clone(&schedule)).await {
                Ok(()) => "ok",
                // A scheduled job that fails is the one failure class with
                // nobody watching: no request, no status, no response to point
                // at. Logged, it is a line somebody has to be watching for;
                // counted, it is an alert on a rate.
                Err(error) => {
                    log_scheduler(
                        "http.job.failed",
                        "error",
                        &error.to_string(),
                        error.log_level(),
                    );
                    "error"
                }
            };
            // Cadence limits attempts, not only successful work. Otherwise a
            // failed slow job (for example a 60-second audit) is retried on
            // every one-second scheduler tick and can amplify a database
            // outage. Recording after the attempt preserves the full interval
            // between both successful runs and retries.
            schedule.record_completed(category, std::time::Instant::now());
            crate::observability::record_maintenance_executed(category.label(), outcome);
            if !category.records_background_pass() {
                record_job_metric(category.label(), outcome, started.elapsed().as_secs_f64());
            }
        });
    }
    while let Some(result) = jobs.join_next().await {
        if let Err(error) = result {
            log_scheduler(
                "http.job.panicked",
                "error",
                &error.to_string(),
                LogLevel::Error,
            );
        }
    }
}

/// Record a scheduler event through the deployment's logger.
///
/// `detail` goes in as one value rather than being split into fields: a
/// `MemoryError` is prose, and splitting it on spaces would report a fragment
/// as though it were the whole failure. The field is named `error` for a
/// failure and `reason` for a job that never ran, which is not one — so the
/// caller says which it is reporting.
fn log_scheduler(op: &'static str, field: &'static str, detail: &str, level: LogLevel) {
    let mut event = std::collections::HashMap::new();
    event.insert("op".into(), op.into());
    event.insert(field.into(), detail.to_string().into());
    crate::logging::emit(event, level);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    async fn test_registry() -> RegistryHandle {
        use surrealdb::Surreal;
        use surrealdb::engine::local::Mem;
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        RegistryHandle::in_memory_with_mem_engine(Arc::new(db))
    }

    /// A scheduled job that fails is the one failure nobody is watching: no
    /// request, no status, no response to point at. It went to stderr as free
    /// text, so it carried no level, no timestamp, and could not be joined to
    /// the run it belonged to — and `RUST_LOG` could not raise or lower it.
    ///
    /// The rendered line is asserted, not the event: the level is what an
    /// operator filters on, and reading it back out of a structure would not
    /// prove the line carries it.
    #[tokio::test]
    async fn a_failing_job_is_logged_with_its_level() {
        let registry = test_registry().await;
        let sink = crate::logging::capture::install();
        let hooks = SchedulerHooks::new(
            vec![SchedulerJob::new(
                MaintenanceJob::Provisioning,
                |_registry, _schedule| {
                    Box::pin(async { Err(MemoryError::Storage("disk is gone".into())) })
                },
            )],
            1,
        )
        .expect("non-empty hooks");
        let shutdown = CancellationToken::new();

        run_cycle(registry, &hooks, shutdown).await;

        let recorded = sink.lines();
        assert!(
            recorded.iter().any(|line| {
                line.contains("op=http.job.failed")
                    // Quoted as one value: a `MemoryError` is prose, and the
                    // line must not break it into a first word and noise.
                    && line.contains(r#"error="storage error: disk is gone""#)
                    && line.contains("ERROR")
            }),
            "a failed job must be logged at error level: {recorded:?}"
        );
    }

    /// The level of a failed job is the error's, not the call site's: a job
    /// that failed on a client-class error must not be reported as if the
    /// deployment were broken, or a storage outage and a bad input would be
    /// indistinguishable to `RUST_LOG`.
    ///
    /// This is the regression pin for asking the error rather than
    /// hardcoding `error` here: it fails the moment this line is hardcoded
    /// back to one level.
    #[tokio::test]
    async fn a_job_failed_on_a_client_error_is_logged_at_its_level() {
        let registry = test_registry().await;
        let sink = crate::logging::capture::install();
        let hooks = SchedulerHooks::new(
            vec![SchedulerJob::new(
                MaintenanceJob::Provisioning,
                |_registry, _schedule| {
                    Box::pin(async { Err(MemoryError::Validation("bad dimension".into())) })
                },
            )],
            1,
        )
        .expect("non-empty hooks");
        let shutdown = CancellationToken::new();

        let _ = crate::logging::capture::with_level("http=info", || {
            run_cycle(registry, &hooks, shutdown)
        })
        .await;

        let recorded = sink.lines();
        assert!(
            recorded.iter().any(|line| {
                line.contains("op=http.job.failed")
                    && line.contains(r#"error="validation error: bad dimension""#)
                    && line.contains("WARN")
            }),
            "a failed job must be logged at the level its error asks for: {recorded:?}"
        );
    }

    /// A failing background job is invisible to every other signal: it has no
    /// request, no status code, and never appears in the request metrics. The
    /// log line is how somebody finds it by reading; the counter is how they
    /// find it without reading — a rate that can be alerted on, which is the
    /// difference between noticing and not.
    #[tokio::test]
    async fn a_failing_job_is_counted_for_the_exporter() {
        let registry = test_registry().await;
        let hooks = SchedulerHooks::new(
            vec![SchedulerJob::new(
                MaintenanceJob::Provisioning,
                |_registry, _schedule| {
                    Box::pin(async { Err(MemoryError::Storage("disk is gone".into())) })
                },
            )],
            1,
        )
        .expect("non-empty hooks");
        let shutdown = CancellationToken::new();

        let exposition = crate::observability::tests::exposed(|| async {
            run_cycle(registry, &hooks, shutdown).await;
            crate::observability::tests::render()
        })
        .await;

        assert!(
            exposition.contains(crate::observability::METRIC_BACKGROUND_JOBS_TOTAL),
            "a background failure must be countable: {exposition}"
        );
        assert!(
            exposition.contains(r#"outcome="error""#),
            "and labelled as the failure it is: {exposition}"
        );
    }

    #[tokio::test]
    async fn failed_slow_job_respects_its_maintenance_cadence() {
        let registry = test_registry().await;
        let runs = Arc::new(AtomicUsize::new(0));
        let runs_for_job = Arc::clone(&runs);
        let hooks = SchedulerHooks::new(
            vec![SchedulerJob::new(
                MaintenanceJob::RegistryNamespaceAudit,
                move |_registry, _schedule| {
                    let runs = Arc::clone(&runs_for_job);
                    Box::pin(async move {
                        runs.fetch_add(1, Ordering::SeqCst);
                        Err(MemoryError::Storage("temporary audit failure".into()))
                    })
                },
            )],
            1,
        )
        .expect("non-empty hooks");
        let shutdown = CancellationToken::new();

        run_cycle(registry.clone(), &hooks, shutdown.clone()).await;
        run_cycle(registry, &hooks, shutdown).await;

        assert_eq!(
            runs.load(Ordering::SeqCst),
            1,
            "a failed maintenance attempt must wait for the category cadence before retrying"
        );
    }

    #[cfg(feature = "prometheus")]
    #[tokio::test]
    async fn cadence_events_are_wired_and_owner_passes_are_not_double_counted() {
        let registry = test_registry().await;
        let provisioning_runs = Arc::new(AtomicUsize::new(0));
        let runs_for_provisioning = Arc::clone(&provisioning_runs);
        let hooks = SchedulerHooks::new(
            vec![
                SchedulerJob::new(MaintenanceJob::Provisioning, move |_registry, _schedule| {
                    let runs = Arc::clone(&runs_for_provisioning);
                    Box::pin(async move {
                        runs.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                }),
                SchedulerJob::new(MaintenanceJob::TaskDispatch, |_registry, _schedule| {
                    Box::pin(async {
                        // The task owner records its whole multi-tenant
                        // pass; the scheduler must not add another sample.
                        crate::observability::record_job_metric("task", "ok", 0.01);
                        Ok(())
                    })
                }),
            ],
            2,
        )
        .expect("non-empty hooks");
        let shutdown = CancellationToken::new();

        let (before, after) = crate::observability::tests::exposed(|| async {
            let before = crate::observability::tests::render();
            run_cycle(registry.clone(), &hooks, shutdown.clone()).await;
            // Both jobs completed successfully, so the same cycle is a
            // cadence skip and must not look like a second execution.
            run_cycle(registry, &hooks, shutdown).await;
            (before, crate::observability::tests::render())
        })
        .await;

        let delta = |metric, labels| {
            let before =
                crate::observability::tests::sample_series(before.as_str(), metric, labels)
                    .unwrap_or(0.0);
            let after = crate::observability::tests::sample_series(after.as_str(), metric, labels)
                .unwrap_or(0.0);
            after - before
        };
        let events = crate::shared::observability::METRIC_HTTP_MAINTENANCE_EVENTS_TOTAL;
        assert_eq!(
            delta(
                events,
                "category=\"provisioning\",event=\"scheduled\",outcome=\"none\""
            ),
            2.0
        );
        assert_eq!(
            delta(
                events,
                "category=\"provisioning\",event=\"skipped\",outcome=\"cadence\""
            ),
            1.0
        );
        assert_eq!(
            delta(
                events,
                "category=\"provisioning\",event=\"executed\",outcome=\"ok\""
            ),
            1.0
        );
        assert_eq!(
            delta(
                crate::observability::METRIC_BACKGROUND_JOBS_TOTAL,
                "pass=\"provisioning\",outcome=\"ok\""
            ),
            1.0
        );
        assert_eq!(
            delta(
                crate::observability::METRIC_BACKGROUND_JOBS_TOTAL,
                "pass=\"task\",outcome=\"ok\""
            ),
            1.0,
            "the task pass has one owner-level duration sample"
        );
        assert_eq!(provisioning_runs.load(Ordering::SeqCst), 1);
    }

    /// A job that does not run because the runtime is shutting down is not a
    /// failure, so it stays at `debug` — but it is recorded, because "the job
    /// was scheduled and then nothing" is otherwise indistinguishable from a
    /// job that was never scheduled at all.
    ///
    /// Run repeatedly: the two branches of that `select!` are both ready once
    /// the token is cancelled, and only `biased` makes shutdown win every time.
    /// A single run passes about half the time without it, so the count is
    /// what proves the guarantee rather than one lucky outcome.
    #[tokio::test]
    async fn a_job_that_did_not_run_says_so() {
        const ROUNDS: usize = 32;
        for _ in 0..ROUNDS {
            let registry = test_registry().await;
            let sink = crate::logging::capture::install();
            let ran = Arc::new(AtomicUsize::new(0));
            let ran_for_job = ran.clone();
            let hooks = SchedulerHooks::new(
                vec![SchedulerJob::new(
                    MaintenanceJob::Provisioning,
                    move |_registry, _schedule| {
                        let ran = ran_for_job.clone();
                        Box::pin(async move {
                            ran.fetch_add(1, Ordering::SeqCst);
                            Ok(())
                        })
                    },
                )],
                1,
            )
            .expect("non-empty hooks");
            // Already cancelled: the job is scheduled and then never acquires a
            // permit, which is the path with no output at all.
            let shutdown = CancellationToken::new();
            shutdown.cancel();

            crate::logging::capture::with_level("http=debug", || async {
                run_cycle(registry, &hooks, shutdown).await;
            })
            .await;

            assert_eq!(
                ran.load(Ordering::SeqCst),
                0,
                "a job must not run once the runtime is shutting down"
            );
            assert!(
                sink.lines()
                    .iter()
                    .any(|line| line.contains("op=http.job.not_run")),
                "a job that did not run must say so: {:?}",
                sink.lines()
            );
        }
    }

    #[tokio::test]
    async fn scheduler_advances_due_work_and_skips_idle() {
        let registry = test_registry().await;
        let runs = Arc::new(AtomicUsize::new(0));
        let runs_for_job = runs.clone();
        let hooks = SchedulerHooks::new(
            vec![SchedulerJob::new(
                MaintenanceJob::Provisioning,
                move |_registry, _schedule| {
                    let runs = runs_for_job.clone();
                    Box::pin(async move {
                        runs.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                },
            )],
            1,
        )
        .expect("non-empty hooks");
        let shutdown = CancellationToken::new();
        let handle = start(registry, hooks, shutdown.clone());
        // The scheduler ticks at 1Hz; sleep past one tick
        // to observe at least one cycle.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        shutdown.cancel();
        handle.join().await;
        assert!(runs.load(Ordering::SeqCst) >= 1);
    }

    #[tokio::test]
    async fn empty_scheduler_hooks_are_rejected() {
        assert!(SchedulerHooks::new(Vec::new(), 1).is_err());
    }

    #[cfg(feature = "control-plane")]
    #[test]
    fn bootstrap_scheduler_hooks_include_deletion_worker() {
        let hooks = crate::bootstrap::provisioning_scheduler_hooks(
            Arc::new(crate::http::leases::migration::NoopMigrations),
            Arc::new(crate::platform::fault_injection::NoFaults),
        )
        .expect("provisioning hooks");
        assert_eq!(hooks.jobs.len(), 2);

        let platform_hooks = SchedulerHooks::with_provisioning_only(
            Arc::new(crate::http::leases::migration::NoopMigrations),
            Arc::new(crate::platform::fault_injection::NoFaults),
        )
        .expect("platform provisioning hooks");
        assert_eq!(platform_hooks.jobs.len(), 1);
    }

    #[tokio::test]
    async fn zero_parallelism_hooks_are_rejected() {
        assert!(SchedulerHooks::new(Vec::new(), 0).is_err());
        let noop = SchedulerJob::new(MaintenanceJob::TaskDispatch, |_registry, _schedule| {
            Box::pin(async { Ok(()) })
        });
        assert!(SchedulerHooks::new(vec![noop], 0).is_err());
    }
}
