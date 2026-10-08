use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Process-wide limits for detached embedding retries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackgroundEmbeddingLimits {
    pub max_admitted_tasks: usize,
    pub max_running_tasks: usize,
    pub max_retained_bytes: usize,
    pub total_timeout: Duration,
}

impl Default for BackgroundEmbeddingLimits {
    fn default() -> Self {
        Self {
            max_admitted_tasks: 8,
            max_running_tasks: 1,
            max_retained_bytes: 262_144,
            total_timeout: Duration::from_secs(60),
        }
    }
}

/// Why a detached embedding retry could not be admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundAdmissionError {
    Duplicate,
    TaskCapacity,
    ByteCapacity,
    Oversized,
    ShuttingDown,
    RegistryPoisoned,
    DeadlineOverflow,
}

/// Safe process-wide resource counters for background embedding work.
#[cfg(any(test, feature = "streamable-http"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackgroundTaskSnapshot {
    pub admitted_tasks: usize,
    pub running_tasks: usize,
    pub retained_bytes: usize,
}

#[derive(Default)]
struct RegistryState {
    tasks: HashMap<Arc<str>, usize>,
    retained_bytes: usize,
    running_tasks: usize,
    shutting_down: bool,
}

struct RunnerInner {
    state: Mutex<RegistryState>,
    limits: BackgroundEmbeddingLimits,
    running_permits: Arc<tokio::sync::Semaphore>,
    shutdown: tokio_util::sync::CancellationToken,
    jobs: tokio::sync::Mutex<tokio::task::JoinSet<()>>,
}

/// Non-cloneable admission token. Dropping it releases its key and retained-byte
/// reservation even when the task exits through cancellation or panic.
pub struct BackgroundTaskReservation {
    inner: Arc<RunnerInner>,
    task_key: Arc<str>,
    deadline: Instant,
    running: bool,
    active: bool,
}

impl BackgroundTaskReservation {
    fn mark_running(&mut self) -> Result<(), BackgroundAdmissionError> {
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BackgroundAdmissionError::RegistryPoisoned)?;
        if state.shutting_down {
            return Err(BackgroundAdmissionError::ShuttingDown);
        }
        state.running_tasks = state
            .running_tasks
            .checked_add(1)
            .ok_or(BackgroundAdmissionError::TaskCapacity)?;
        self.running = true;
        Ok(())
    }
}

impl Drop for BackgroundTaskReservation {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = match self.inner.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(retained_bytes) = state.tasks.remove(self.task_key.as_ref()) {
            state.retained_bytes = state.retained_bytes.saturating_sub(retained_bytes);
        }
        if self.running {
            state.running_tasks = state.running_tasks.saturating_sub(1);
        }
        self.active = false;
    }
}

/// Nonblocking admission ledger for detached embedding tasks.
pub struct BackgroundTaskRunner {
    inner: Arc<RunnerInner>,
}

impl BackgroundTaskRunner {
    pub fn new() -> Self {
        Self::with_limits(BackgroundEmbeddingLimits::default())
    }

    pub fn with_limits(limits: BackgroundEmbeddingLimits) -> Self {
        Self {
            inner: Arc::new(RunnerInner {
                state: Mutex::new(RegistryState::default()),
                running_permits: Arc::new(tokio::sync::Semaphore::new(limits.max_running_tasks)),
                shutdown: tokio_util::sync::CancellationToken::new(),
                jobs: tokio::sync::Mutex::new(tokio::task::JoinSet::new()),
                limits,
            }),
        }
    }

    /// Atomically reserves one key and its retained input bytes without waiting.
    pub fn try_admit(
        &self,
        task_key: &str,
        retained_bytes: usize,
    ) -> Result<BackgroundTaskReservation, BackgroundAdmissionError> {
        let deadline = Instant::now()
            .checked_add(self.inner.limits.total_timeout)
            .ok_or(BackgroundAdmissionError::DeadlineOverflow)?;
        let mut state = self
            .inner
            .state
            .lock()
            .map_err(|_| BackgroundAdmissionError::RegistryPoisoned)?;
        if state.shutting_down {
            return Err(BackgroundAdmissionError::ShuttingDown);
        }
        if state.tasks.contains_key(task_key) {
            return Err(BackgroundAdmissionError::Duplicate);
        }
        if retained_bytes > self.inner.limits.max_retained_bytes {
            return Err(BackgroundAdmissionError::Oversized);
        }
        if state.tasks.len() >= self.inner.limits.max_admitted_tasks {
            return Err(BackgroundAdmissionError::TaskCapacity);
        }
        let total_bytes = state
            .retained_bytes
            .checked_add(retained_bytes)
            .ok_or(BackgroundAdmissionError::ByteCapacity)?;
        if total_bytes > self.inner.limits.max_retained_bytes {
            return Err(BackgroundAdmissionError::ByteCapacity);
        }
        let task_key: Arc<str> = Arc::from(task_key);
        state.tasks.insert(task_key.clone(), retained_bytes);
        state.retained_bytes = total_bytes;
        Ok(BackgroundTaskReservation {
            inner: self.inner.clone(),
            task_key,
            deadline,
            running: false,
            active: true,
        })
    }

    /// Spawn work under the reservation's admitted deadline and the process-wide
    /// running permit. Completed handles are reaped before another one is stored.
    pub async fn spawn<F>(
        &self,
        reservation: BackgroundTaskReservation,
        work: F,
    ) -> Result<(), BackgroundAdmissionError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let inner = self.inner.clone();
        let deadline = tokio::time::Instant::from_std(reservation.deadline);
        let mut jobs = self.inner.jobs.lock().await;
        while jobs.try_join_next().is_some() {}
        let state = self
            .inner
            .state
            .lock()
            .map_err(|_| BackgroundAdmissionError::RegistryPoisoned)?;
        if state.shutting_down {
            return Err(BackgroundAdmissionError::ShuttingDown);
        }
        jobs.spawn(async move {
            let permit = tokio::select! {
                _ = inner.shutdown.cancelled() => return,
                result = tokio::time::timeout_at(
                    deadline,
                    inner.running_permits.clone().acquire_owned(),
                ) => match result {
                    Ok(Ok(permit)) => permit,
                    _ => return,
                },
            };
            let mut reservation = reservation;
            if reservation.mark_running().is_err() {
                return;
            }
            tokio::select! {
                _ = inner.shutdown.cancelled() => {}
                _ = tokio::time::timeout_at(deadline, work) => {}
            }
            drop(permit);
        });
        drop(state);
        Ok(())
    }

    /// Returns an identity-free snapshot of the current resource ledger.
    #[cfg(any(test, feature = "streamable-http"))]
    pub fn resource_snapshot(&self) -> BackgroundTaskSnapshot {
        let state = match self.inner.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        BackgroundTaskSnapshot {
            admitted_tasks: state.tasks.len(),
            running_tasks: state.running_tasks,
            retained_bytes: state.retained_bytes,
        }
    }

    /// Closes admission and cancels queued/running local job futures.
    #[cfg(any(test, feature = "streamable-http"))]
    pub fn shutdown(&self) {
        let mut state = match self.inner.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.shutting_down = true;
        drop(state);
        self.inner.shutdown.cancel();
    }

    /// Joins tracked tasks until an absolute deadline, aborting the remainder
    /// if it expires. Admission must be closed separately with [`Self::shutdown`]
    /// when the caller is performing shutdown.
    #[cfg(any(test, feature = "streamable-http"))]
    pub async fn join_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Result<(), crate::error::MemoryError> {
        let mut jobs = self.inner.jobs.lock().await;
        let mut first_failure = None;
        while !jobs.is_empty() {
            match tokio::time::timeout_at(deadline, jobs.join_next()).await {
                Ok(Some(Ok(()))) => {}
                Ok(Some(Err(error))) => {
                    first_failure.get_or_insert_with(|| error.to_string());
                }
                Ok(None) => break,
                Err(_) => {
                    jobs.abort_all();
                    while jobs.join_next().await.is_some() {}
                    return Err(crate::error::MemoryError::Storage(
                        "background embedding jobs exceeded shutdown deadline; remaining jobs aborted"
                            .to_string(),
                    ));
                }
            }
        }
        if let Some(error) = first_failure {
            return Err(crate::error::MemoryError::Storage(format!(
                "background embedding job failed while joining: {error}"
            )));
        }
        Ok(())
    }

    /// Returns whether an admitted key is currently present.
    pub async fn is_inflight(&self, task_key: &str) -> bool {
        self.inner
            .state
            .lock()
            .map(|state| state.tasks.contains_key(task_key))
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn one_running_job_keeps_pending_admitted_work_bounded() {
        let runner = Arc::new(BackgroundTaskRunner::with_limits(
            BackgroundEmbeddingLimits {
                max_admitted_tasks: 2,
                max_running_tasks: 1,
                max_retained_bytes: 100,
                ..BackgroundEmbeddingLimits::default()
            },
        ));
        let first = runner.try_admit("task:first", 1).expect("first admitted");
        let (first_started_tx, first_started_rx) = tokio::sync::oneshot::channel();
        let (release_first_tx, release_first_rx) = tokio::sync::oneshot::channel();
        runner
            .spawn(first, async move {
                let _ = first_started_tx.send(());
                let _ = release_first_rx.await;
            })
            .await
            .expect("first job spawn succeeds");
        first_started_rx
            .await
            .expect("first job reaches running state");

        let second = runner.try_admit("task:second", 1).expect("second admitted");
        let (second_started_tx, second_started_rx) = tokio::sync::oneshot::channel();
        runner
            .spawn(second, async move {
                let _ = second_started_tx.send(());
            })
            .await
            .expect("second job spawn succeeds");
        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot {
                admitted_tasks: 2,
                running_tasks: 1,
                retained_bytes: 2,
            }
        );

        let _ = release_first_tx.send(());
        tokio::time::timeout(std::time::Duration::from_secs(5), second_started_rx)
            .await
            .expect("second job starts after the running permit is released")
            .expect("second start signal is sent");
        runner
            .join_until(tokio::time::Instant::now() + std::time::Duration::from_secs(5))
            .await
            .expect("jobs join before the deadline");
        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot::default()
        );
    }

    #[tokio::test]
    async fn admitted_deadline_includes_time_waiting_for_running_capacity() {
        let total_timeout = Duration::from_millis(400);
        let runner = BackgroundTaskRunner::with_limits(BackgroundEmbeddingLimits {
            max_admitted_tasks: 2,
            max_running_tasks: 1,
            total_timeout,
            ..BackgroundEmbeddingLimits::default()
        });
        let first = runner
            .try_admit("task:first-deadline", 1)
            .expect("first task is admitted");
        let (first_started_tx, first_started_rx) = tokio::sync::oneshot::channel();
        runner
            .spawn(first, async move {
                let _ = first_started_tx.send(());
                std::future::pending::<()>().await;
            })
            .await
            .expect("first task is spawned");
        first_started_rx
            .await
            .expect("first task occupies running capacity");

        assert!(
            tokio::time::timeout(Duration::from_millis(100), std::future::pending::<()>())
                .await
                .is_err()
        );
        assert_eq!(runner.resource_snapshot().running_tasks, 1);
        let second_admitted_at = tokio::time::Instant::now();
        let second_deadline = second_admitted_at + total_timeout;
        let second = runner
            .try_admit("task:second-deadline", 1)
            .expect("second task is admitted while the first runs");
        let (second_started_tx, second_started_rx) = tokio::sync::oneshot::channel();
        runner
            .spawn(second, async move {
                let _ = second_started_tx.send(());
                std::future::pending::<()>().await;
            })
            .await
            .expect("second task is spawned");
        tokio::time::timeout_at(
            second_deadline + Duration::from_millis(150),
            second_started_rx,
        )
        .await
        .expect("second task starts after the first deadline")
        .expect("second start signal is sent");

        runner
            .join_until(second_deadline + Duration::from_millis(150))
            .await
            .expect("second task expires at its admission deadline, not a new run deadline");
        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot::default()
        );
    }

    #[tokio::test]
    async fn admitted_deadline_covers_provider_backoff_and_persistence() {
        let total_timeout = Duration::from_millis(500);
        let runner = BackgroundTaskRunner::with_limits(BackgroundEmbeddingLimits {
            total_timeout,
            ..BackgroundEmbeddingLimits::default()
        });
        let admission_time = tokio::time::Instant::now();
        let reservation = runner
            .try_admit("task:full-deadline", 1)
            .expect("task is admitted");
        let (provider_started_tx, provider_started_rx) = tokio::sync::oneshot::channel();
        let (provider_release_tx, provider_release_rx) = tokio::sync::oneshot::channel::<()>();
        let (backoff_started_tx, backoff_started_rx) = tokio::sync::oneshot::channel();
        let (backoff_release_tx, backoff_release_rx) = tokio::sync::oneshot::channel::<()>();
        let (persistence_started_tx, persistence_started_rx) = tokio::sync::oneshot::channel();
        let (_persistence_release_tx, persistence_release_rx) =
            tokio::sync::oneshot::channel::<()>();
        runner
            .spawn(reservation, async move {
                let _ = provider_started_tx.send(());
                let _ = provider_release_rx.await;
                let _ = backoff_started_tx.send(());
                let _ = backoff_release_rx.await;
                let _ = persistence_started_tx.send(());
                let _ = persistence_release_rx.await;
            })
            .await
            .expect("task is spawned");

        provider_started_rx.await.expect("provider phase starts");
        let _ = provider_release_tx.send(());
        backoff_started_rx.await.expect("backoff phase starts");
        let _ = backoff_release_tx.send(());
        persistence_started_rx
            .await
            .expect("persistence phase starts before the total deadline");

        runner
            .join_until(admission_time + total_timeout + Duration::from_secs(1))
            .await
            .expect("the total admission deadline cancels pending persistence work");
        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot::default()
        );
    }

    #[test]
    fn default_limits_match_the_bounded_retry_contract() {
        assert_eq!(
            BackgroundEmbeddingLimits::default(),
            BackgroundEmbeddingLimits {
                max_admitted_tasks: 8,
                max_running_tasks: 1,
                max_retained_bytes: 262_144,
                total_timeout: Duration::from_secs(60),
            }
        );
    }

    #[test]
    fn duplicate_key_is_refused() {
        let runner = BackgroundTaskRunner::with_limits(BackgroundEmbeddingLimits::default());
        let _reservation = runner
            .try_admit("query:signature:identity", 64)
            .expect("first reservation should be admitted");

        assert!(matches!(
            runner.try_admit("query:signature:identity", 64),
            Err(BackgroundAdmissionError::Duplicate)
        ));
    }

    #[test]
    fn task_capacity_is_nonblocking() {
        let runner = BackgroundTaskRunner::with_limits(BackgroundEmbeddingLimits {
            max_admitted_tasks: 1,
            ..BackgroundEmbeddingLimits::default()
        });
        let _first = runner
            .try_admit("task:first", 1)
            .expect("first task should be admitted");

        assert!(matches!(
            runner.try_admit("task:second", 1),
            Err(BackgroundAdmissionError::TaskCapacity)
        ));
    }

    #[test]
    fn retained_byte_capacity_is_nonblocking() {
        let runner = BackgroundTaskRunner::with_limits(BackgroundEmbeddingLimits {
            max_retained_bytes: 10,
            ..BackgroundEmbeddingLimits::default()
        });
        let first = runner
            .try_admit("task:first", 6)
            .expect("first retained input fits");

        assert!(matches!(
            runner.try_admit("task:second", 5),
            Err(BackgroundAdmissionError::ByteCapacity)
        ));
        drop(first);
        assert!(runner.try_admit("task:second", 5).is_ok());
    }

    #[test]
    fn oversized_input_is_refused_without_retaining_bytes() {
        let runner = BackgroundTaskRunner::with_limits(BackgroundEmbeddingLimits {
            max_retained_bytes: 8,
            ..BackgroundEmbeddingLimits::default()
        });

        assert!(matches!(
            runner.try_admit("task:oversized", 9),
            Err(BackgroundAdmissionError::Oversized)
        ));
        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot::default()
        );
    }

    #[test]
    fn reservation_drop_releases_key_and_bytes() {
        let runner = BackgroundTaskRunner::with_limits(BackgroundEmbeddingLimits {
            max_admitted_tasks: 1,
            max_retained_bytes: 100,
            ..BackgroundEmbeddingLimits::default()
        });
        let reservation = runner
            .try_admit("fact:namespace:fact-id", 60)
            .expect("reservation fits both budgets");
        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot {
                admitted_tasks: 1,
                running_tasks: 0,
                retained_bytes: 60,
            }
        );

        drop(reservation);

        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot::default()
        );
        assert!(runner.try_admit("fact:namespace:fact-id", 60).is_ok());
    }

    #[tokio::test]
    async fn task_panic_releases_admission_and_is_reported_by_join() {
        let runner = BackgroundTaskRunner::new();
        let reservation = runner
            .try_admit("task:panics", 12)
            .expect("task reservation succeeds");
        runner
            .spawn(reservation, async {
                panic!("synthetic task panic");
            })
            .await
            .expect("task is spawned");

        let result = runner
            .join_until(tokio::time::Instant::now() + Duration::from_secs(5))
            .await;

        assert!(result.is_err(), "the join reports the task panic");
        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot::default()
        );
    }

    #[tokio::test]
    async fn shutdown_rejects_admission_and_joins_running_work() {
        let runner = BackgroundTaskRunner::new();
        let reservation = runner
            .try_admit("task:shutdown", 12)
            .expect("task reservation succeeds");
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel::<()>();
        runner
            .spawn(reservation, async move {
                let _ = started_tx.send(());
                let _ = release_rx.await;
            })
            .await
            .expect("task is spawned");
        started_rx.await.expect("task reaches its running state");

        runner.shutdown();
        assert!(matches!(
            runner.try_admit("task:after-shutdown", 1),
            Err(BackgroundAdmissionError::ShuttingDown)
        ));
        runner
            .join_until(tokio::time::Instant::now() + Duration::from_secs(5))
            .await
            .expect("shutdown cancels and joins running work");

        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot::default()
        );
        let _ = release_tx.send(());
    }

    #[tokio::test]
    async fn expired_join_deadline_aborts_and_releases_all_admission() {
        let runner = BackgroundTaskRunner::new();
        let reservation = runner
            .try_admit("task:abort", 12)
            .expect("task reservation succeeds");
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        runner
            .spawn(reservation, async move {
                let _ = started_tx.send(());
                std::future::pending::<()>().await;
            })
            .await
            .expect("task is spawned");
        started_rx.await.expect("task reaches its running state");
        let result = runner.join_until(tokio::time::Instant::now()).await;

        assert!(result.is_err(), "an expired shutdown deadline is reported");
        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot::default()
        );
    }

    #[tokio::test]
    async fn shutdown_racing_spawn_never_registers_unjoined_work() {
        let runner = Arc::new(BackgroundTaskRunner::new());
        let reservation = runner
            .try_admit("task:spawn-race", 12)
            .expect("reservation is admitted before shutdown");
        let jobs_guard = runner.inner.jobs.lock().await;
        let spawn_runner = runner.clone();
        let (spawn_result_tx, spawn_result_rx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let result = spawn_runner.spawn(reservation, async {}).await;
            let _ = spawn_result_tx.send(result);
        });
        tokio::task::yield_now().await;

        runner.shutdown();
        drop(jobs_guard);
        let result = spawn_result_rx
            .await
            .expect("spawn reports its admission outcome");

        assert!(matches!(
            result,
            Err(BackgroundAdmissionError::ShuttingDown)
        ));
        assert_eq!(
            runner.resource_snapshot(),
            BackgroundTaskSnapshot::default()
        );
    }

    #[tokio::test]
    async fn is_inflight_reports_correctly() {
        let runner = BackgroundTaskRunner::new();
        assert!(!runner.is_inflight("missing").await);
        let reservation = runner
            .try_admit("task:2", 0)
            .expect("task reservation succeeds");
        assert!(runner.is_inflight("task:2").await);
        drop(reservation);
        assert!(!runner.is_inflight("task:2").await);
    }
}
