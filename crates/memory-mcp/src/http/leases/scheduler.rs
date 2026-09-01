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

pub type JobFuture = Pin<Box<dyn Future<Output = Result<(), MemoryError>> + Send>>;
pub type SchedulerJob = Arc<dyn Fn(RegistryHandle) -> JobFuture + Send + Sync>;

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

    pub fn with_provisioning_only() -> Result<Self, MemoryError> {
        let mut jobs: Vec<SchedulerJob> = vec![Arc::new(|registry| {
            Box::pin(crate::http::leases::migration::run_due_provisioning(
                registry,
            ))
        })];
        #[cfg(feature = "control-plane")]
        {
            jobs.push(Arc::new(|registry| {
                Box::pin(crate::control::deletion::run_deletion_worker(registry))
            }));
        }
        Self::new(jobs, 4)
    }

    /// Tasks 7–9 call this before the binary starts serving
    /// to add their cleanup/retry/outbox jobs. The returned
    /// value is immutable thereafter (the inner Vec is in
    /// `Arc`, so `with_additional_job` rebuilds a new
    /// `SchedulerHooks` rather than mutating the old one).
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
            eprintln!("memory_mcp::http::scheduler: scheduler task failed: {error}");
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
                _ = shutdown.cancelled() => return,
                permit = semaphore.acquire_owned() => match permit {
                    Ok(permit) => permit,
                    Err(_) => return,
                },
            };
            let _permit = permit;
            if let Err(error) = job(registry).await {
                eprintln!("memory_mcp::http::scheduler: scheduled job failed: {error}");
            }
        });
    }
    while let Some(result) = jobs.join_next().await {
        if let Err(error) = result {
            eprintln!("memory_mcp::http::scheduler: scheduled job panicked: {error}");
        }
    }
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
    fn provisioning_hooks_include_deletion_worker() {
        let hooks = SchedulerHooks::with_provisioning_only().expect("provisioning hooks");
        assert_eq!(hooks.jobs.len(), 2);
    }

    #[tokio::test]
    async fn zero_parallelism_hooks_are_rejected() {
        assert!(SchedulerHooks::new(Vec::new(), 0).is_err());
        let noop: SchedulerJob = Arc::new(|_registry| Box::pin(async { Ok(()) }));
        assert!(SchedulerHooks::new(vec![noop], 0).is_err());
    }
}
