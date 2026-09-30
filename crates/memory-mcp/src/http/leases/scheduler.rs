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

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::error::MemoryError;
use crate::http::registry::RegistryHandle;
use crate::logging::{LogLevel, StdoutLogger};
use crate::observability::record_job_metric;

pub type JobFuture = Pin<Box<dyn Future<Output = Result<(), MemoryError>> + Send>>;
pub type SchedulerJob = Arc<dyn Fn(RegistryHandle) -> JobFuture + Send + Sync>;

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
        let provisioning: SchedulerJob = Arc::new(move |registry| {
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
        jobs.spawn(async move {
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
                    return;
                }
                permit = semaphore.acquire_owned() => match permit {
                    Ok(permit) => permit,
                    Err(_) => {
                        // The semaphore is closed, which happens on shutdown.
                        log_scheduler("http.job.not_run", "reason", "runtime_shutdown", LogLevel::Debug);
                        return;
                    }
                },
            };
            let _permit = permit;
            // Measured from the moment the job actually starts running, not
            // from when it was scheduled: a job that waited on the semaphore
            // would otherwise report the queue's delay as its own work.
            let started = std::time::Instant::now();
            let outcome = match job(registry).await {
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
                        LogLevel::Error,
                    );
                    "error"
                }
            };
            record_job_metric("lease", outcome, started.elapsed().as_secs_f64());
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
    StdoutLogger::from_env().log(event, level);
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
            vec![Arc::new(|_registry| {
                Box::pin(async { Err(MemoryError::Storage("disk is gone".into())) })
            })],
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

    /// A failing background job is invisible to every other signal: it has no
    /// request, no status code, and never appears in the request metrics. The
    /// log line is how somebody finds it by reading; the counter is how they
    /// find it without reading — a rate that can be alerted on, which is the
    /// difference between noticing and not.
    #[tokio::test]
    async fn a_failing_job_is_counted_for_the_exporter() {
        let registry = test_registry().await;
        let hooks = SchedulerHooks::new(
            vec![Arc::new(|_registry| {
                Box::pin(async { Err(MemoryError::Storage("disk is gone".into())) })
            })],
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
                vec![Arc::new(move |_registry| {
                    let ran = ran_for_job.clone();
                    Box::pin(async move {
                        ran.fetch_add(1, Ordering::SeqCst);
                        Ok(())
                    })
                })],
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
            vec![Arc::new(move |_registry| {
                let runs = runs_for_job.clone();
                Box::pin(async move {
                    runs.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            })],
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
        let noop: SchedulerJob = Arc::new(|_registry| Box::pin(async { Ok(()) }));
        assert!(SchedulerHooks::new(vec![noop], 0).is_err());
    }
}
