//! Shared loaded-model lifecycle for heavyweight model instances.
//!
//! `LoadedModel<T>` defers construction of `T` until first use and unloads it
//! after a configurable idle period. Unload is armed at USE COMPLETION (never
//! on load), so a long-running inference cannot be interrupted mid-flight.
//! `InferenceGate` bounds concurrent inference permits.
//!
//! This module owns only in-memory retention. Artifact acquisition, model
//! construction, device policy, and validation are backend responsibilities.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::error::MemoryError;

type ScheduledTask = Pin<Box<dyn Future<Output = ()> + Send + 'static>>;

trait MonotonicRuntime: Send + Sync {
    fn now(&self) -> Duration;

    fn spawn_at(&self, deadline: Duration, task: ScheduledTask) -> tokio::task::JoinHandle<()>;
}

struct TokioRuntime {
    origin: Instant,
}

impl MonotonicRuntime for TokioRuntime {
    fn now(&self) -> Duration {
        self.origin.elapsed()
    }

    fn spawn_at(&self, deadline: Duration, task: ScheduledTask) -> tokio::task::JoinHandle<()> {
        let deadline = self.origin + deadline;
        tokio::spawn(async move {
            tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
            task.await;
        })
    }
}

/// State of a loaded model instance.
pub(crate) struct LoadedModelState<T> {
    loaded: Option<Arc<T>>,
    last_used: Duration,
    unload_handle: Option<tokio::task::JoinHandle<()>>,
}

/// A model that is constructed on first use and dropped after `idle_unload`
/// of inactivity. `None` disables unloading (model stays loaded forever).
pub(crate) struct LoadedModel<T> {
    state: Arc<Mutex<LoadedModelState<T>>>,
    idle_unload: Option<Duration>,
    runtime: Arc<dyn MonotonicRuntime>,
}

impl<T: Send + Sync + 'static> LoadedModel<T> {
    pub(crate) fn new(idle_unload: Option<Duration>) -> Self {
        Self::with_runtime(
            idle_unload,
            Arc::new(TokioRuntime {
                origin: Instant::now(),
            }),
        )
    }

    fn with_runtime(idle_unload: Option<Duration>, runtime: Arc<dyn MonotonicRuntime>) -> Self {
        let last_used = runtime.now();
        Self {
            state: Arc::new(Mutex::new(LoadedModelState {
                loaded: None,
                last_used,
                unload_handle: None,
            })),
            idle_unload,
            runtime,
        }
    }

    /// Returns the cached model, or constructs it exactly once under the
    /// state lock. The `load` closure runs on the blocking pool.
    /// Does NOT schedule an unload — call `arm_unload` after use.
    pub(crate) async fn get_or_load<F>(&self, load: F) -> Result<Arc<T>, MemoryError>
    where
        F: FnOnce() -> Result<Arc<T>, MemoryError> + Send + 'static,
    {
        let mut guard = self.state.lock().await;
        if guard.loaded.is_some() {
            guard.last_used = self.runtime.now();
            if let Some(handle) = guard.unload_handle.take() {
                handle.abort();
            }
            let Some(loaded) = guard.loaded.as_ref() else {
                return Err(MemoryError::Storage(
                    "loaded model disappeared while accessing the cache".to_string(),
                ));
            };
            return Ok(Arc::clone(loaded));
        }
        let loaded = tokio::task::spawn_blocking(load)
            .await
            .map_err(|err| MemoryError::Storage(format!("model load task panicked: {err}")))??;
        guard.last_used = self.runtime.now();
        guard.loaded = Some(Arc::clone(&loaded));
        Ok(loaded)
    }

    /// Installs an already-validated model without invoking the loader.
    ///
    /// Used to hand a successfully probed candidate to the runtime so the
    /// first real extraction reuses it instead of constructing a second copy.
    /// Aborts any pending unload task, resets the idle clock, and replaces the
    /// cached instance.
    pub(crate) async fn install_loaded(&self, loaded: Arc<T>) {
        let mut guard = self.state.lock().await;
        if let Some(handle) = guard.unload_handle.take() {
            handle.abort();
        }
        guard.last_used = self.runtime.now();
        guard.loaded = Some(loaded);
    }

    /// Records that the model was used and (re)arms the idle-unload timer.
    /// The idle clock starts at USE COMPLETION, so an unload can never fire
    /// while an extract is still running.
    pub(crate) async fn arm_unload(&self) {
        let mut guard = self.state.lock().await;
        let last_used = self.runtime.now();
        guard.last_used = last_used;
        if let Some(handle) = guard.unload_handle.take() {
            handle.abort();
        }
        guard.unload_handle = self.idle_unload.map(|timeout| {
            Self::spawn_unload_task(
                Arc::clone(&self.state),
                Arc::clone(&self.runtime),
                last_used.saturating_add(timeout),
                timeout,
            )
        });
    }

    fn spawn_unload_task(
        state: Arc<Mutex<LoadedModelState<T>>>,
        runtime: Arc<dyn MonotonicRuntime>,
        deadline: Duration,
        timeout: Duration,
    ) -> tokio::task::JoinHandle<()> {
        let task_runtime = Arc::clone(&runtime);
        runtime.spawn_at(
            deadline,
            Box::pin(async move {
                let mut guard = state.lock().await;
                if task_runtime.now().saturating_sub(guard.last_used) >= timeout {
                    guard.loaded = None;
                    guard.unload_handle = None;
                }
            }),
        )
    }
}

/// Bounds concurrent inference to the configured permit pool.
#[derive(Debug, Clone)]
pub(crate) struct InferenceGate {
    permits: Arc<tokio::sync::Semaphore>,
}

impl InferenceGate {
    pub(crate) fn new(max_concurrency: usize) -> Self {
        Self {
            permits: Arc::new(tokio::sync::Semaphore::new(max_concurrency)),
        }
    }

    pub(crate) async fn acquire(
        &self,
    ) -> Result<(tokio::sync::OwnedSemaphorePermit, Duration), tokio::sync::AcquireError> {
        let started = Instant::now();
        let permit = self.permits.clone().acquire_owned().await?;
        Ok((permit, started.elapsed()))
    }

    pub(crate) fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;
    use tokio::sync::oneshot;

    async fn bounded<F: Future>(future: F) -> F::Output {
        tokio::time::timeout(Duration::from_secs(5), future)
            .await
            .expect("model runtime test must make progress")
    }

    struct ControlledRuntime {
        state: StdMutex<ControlledRuntimeState>,
    }

    struct ControlledRuntimeState {
        now: Duration,
        scheduled: Vec<ScheduledJob>,
    }

    struct ScheduledJob {
        deadline: Duration,
        trigger: oneshot::Sender<()>,
        completed: oneshot::Receiver<()>,
    }

    impl ControlledRuntime {
        fn new() -> Self {
            Self {
                state: StdMutex::new(ControlledRuntimeState {
                    now: Duration::ZERO,
                    scheduled: Vec::new(),
                }),
            }
        }

        async fn advance_by(&self, duration: Duration) {
            let due = {
                let mut state = self.state.lock().expect("controlled runtime lock");
                state.now = state.now.saturating_add(duration);
                let now = state.now;
                let (due, pending): (Vec<_>, Vec<_>) = std::mem::take(&mut state.scheduled)
                    .into_iter()
                    .partition(|job| job.deadline <= now);
                state.scheduled = pending;
                due
            };

            let completions = due
                .into_iter()
                .filter_map(|job| job.trigger.send(()).ok().map(|()| job.completed))
                .collect::<Vec<_>>();
            for completion in completions {
                // Aborting a scheduled task may race with its trigger.
                let _ = bounded(completion).await;
            }
        }
    }

    impl MonotonicRuntime for ControlledRuntime {
        fn now(&self) -> Duration {
            self.state.lock().expect("controlled runtime lock").now
        }

        fn spawn_at(&self, deadline: Duration, task: ScheduledTask) -> tokio::task::JoinHandle<()> {
            let (trigger, wait) = oneshot::channel();
            let (complete, completed) = oneshot::channel();
            self.state
                .lock()
                .expect("controlled runtime lock")
                .scheduled
                .push(ScheduledJob {
                    deadline,
                    trigger,
                    completed,
                });

            tokio::spawn(async move {
                if wait.await.is_ok() {
                    task.await;
                    let _ = complete.send(());
                }
            })
        }
    }

    fn controlled_model(
        idle_unload: Option<Duration>,
    ) -> (LoadedModel<String>, Arc<ControlledRuntime>) {
        let runtime = Arc::new(ControlledRuntime::new());
        let model_runtime: Arc<dyn MonotonicRuntime> = runtime.clone();
        let model = LoadedModel::with_runtime(idle_unload, model_runtime);
        (model, runtime)
    }

    #[tokio::test]
    async fn constructs_and_reuses_the_same_model() {
        let (model, _) = controlled_model(None);
        let constructed = Arc::new("model".to_string());
        let loaded = model
            .get_or_load({
                let constructed = Arc::clone(&constructed);
                move || Ok(constructed)
            })
            .await
            .expect("first construction");
        assert!(Arc::ptr_eq(&loaded, &constructed));
        assert_eq!(loaded.as_str(), "model");

        let cached = model
            .get_or_load(|| Ok(Arc::new("unexpected".to_string())))
            .await
            .expect("cached model");
        assert!(Arc::ptr_eq(&cached, &constructed));
    }

    #[tokio::test]
    async fn installation_returns_the_installed_model_and_cancels_pending_unload() {
        let (model, runtime) = controlled_model(Some(Duration::from_secs(10)));
        let first = Arc::new("first".to_string());
        model.install_loaded(Arc::clone(&first)).await;
        model.arm_unload().await;

        runtime.advance_by(Duration::from_secs(5)).await;
        let replacement = Arc::new("replacement".to_string());
        model.install_loaded(Arc::clone(&replacement)).await;
        runtime.advance_by(Duration::from_secs(5)).await;

        let loaded = model
            .get_or_load(|| Ok(Arc::new("unexpected".to_string())))
            .await
            .expect("installed model remains loaded");
        assert!(Arc::ptr_eq(&loaded, &replacement));
        assert_eq!(loaded.as_str(), "replacement");
    }

    #[tokio::test]
    async fn no_unload_is_scheduled_before_the_first_arm() {
        let (model, runtime) = controlled_model(Some(Duration::from_secs(10)));
        let original = model
            .get_or_load(|| Ok(Arc::new("original".to_string())))
            .await
            .expect("initial model");

        runtime.advance_by(Duration::from_secs(20)).await;

        let loaded = model
            .get_or_load(|| Ok(Arc::new("unexpected".to_string())))
            .await
            .expect("model remains loaded before first arm");
        assert!(Arc::ptr_eq(&loaded, &original));
    }

    #[tokio::test]
    async fn rearming_retains_model_past_the_original_deadline() {
        let (model, runtime) = controlled_model(Some(Duration::from_millis(500)));
        let original = model
            .get_or_load(|| Ok(Arc::new("original".to_string())))
            .await
            .expect("initial model");
        model.arm_unload().await;

        runtime.advance_by(Duration::from_millis(100)).await;
        model.arm_unload().await;
        runtime.advance_by(Duration::from_millis(450)).await;

        let loaded = model
            .get_or_load(|| Ok(Arc::new("reloaded too early".to_string())))
            .await
            .expect("renewed idle window retains model");
        assert!(Arc::ptr_eq(&loaded, &original));
        assert_eq!(loaded.as_str(), "original");
    }

    #[tokio::test]
    async fn rearmed_model_unloads_at_the_renewed_deadline() {
        let (model, runtime) = controlled_model(Some(Duration::from_millis(500)));
        let original = model
            .get_or_load(|| Ok(Arc::new("original".to_string())))
            .await
            .expect("initial model");
        model.arm_unload().await;

        runtime.advance_by(Duration::from_millis(100)).await;
        model.arm_unload().await;
        runtime.advance_by(Duration::from_millis(500)).await;

        let reloaded = model
            .get_or_load(|| Ok(Arc::new("reloaded".to_string())))
            .await
            .expect("expired model is reconstructed");
        assert!(!Arc::ptr_eq(&reloaded, &original));
        assert_eq!(reloaded.as_str(), "reloaded");
    }

    #[tokio::test]
    async fn disabled_unload_keeps_model_loaded_after_an_arm() {
        let (model, runtime) = controlled_model(None);
        let original = model
            .get_or_load(|| Ok(Arc::new("original".to_string())))
            .await
            .expect("initial model");
        model.arm_unload().await;
        runtime.advance_by(Duration::from_secs(100)).await;

        let loaded = model
            .get_or_load(|| Ok(Arc::new("unexpected".to_string())))
            .await
            .expect("unload is disabled");
        assert!(Arc::ptr_eq(&loaded, &original));
    }

    #[tokio::test]
    async fn loader_failure_does_not_prevent_a_later_construction() {
        let (model, _) = controlled_model(None);
        let failure = model
            .get_or_load(|| {
                Err(MemoryError::Storage(
                    "planned model load failure".to_string(),
                ))
            })
            .await;
        assert!(matches!(
            failure,
            Err(MemoryError::Storage(message)) if message == "planned model load failure"
        ));

        let constructed = Arc::new("recovered".to_string());
        let recovered = model
            .get_or_load({
                let constructed = Arc::clone(&constructed);
                move || Ok(constructed)
            })
            .await
            .expect("loader recovers after failure");
        assert!(Arc::ptr_eq(&recovered, &constructed));
        assert_eq!(recovered.as_str(), "recovered");
    }

    #[tokio::test]
    async fn concurrent_construction_shares_the_model_from_the_first_loader() {
        let (model, _) = controlled_model(None);
        let model = Arc::new(model);
        let (started, started_rx) = oneshot::channel();
        let (release, release_rx) = oneshot::channel();
        let first_model = Arc::clone(&model);
        let first = tokio::spawn(async move {
            first_model
                .get_or_load(move || {
                    started.send(()).expect("test receiver remains open");
                    release_rx
                        .blocking_recv()
                        .expect("test releases first loader");
                    Ok(Arc::new("first".to_string()))
                })
                .await
                .expect("first loader succeeds")
        });
        bounded(started_rx).await.expect("first loader started");

        let second_model = Arc::clone(&model);
        let second = second_model.get_or_load(|| Ok(Arc::new("second".to_string())));
        tokio::pin!(second);
        tokio::select! {
            biased;
            result = &mut second => panic!("second loader must wait for the first: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }

        release.send(()).expect("first loader is waiting");
        let first = bounded(first).await.expect("first task completes");
        let second = bounded(second)
            .await
            .expect("waiting caller receives cached model");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(second.as_str(), "first");
    }

    /// Integration: the default Tokio scheduler actually releases its model.
    #[tokio::test]
    async fn default_runtime_releases_the_resource_after_idle_expiry() {
        struct ModelResource(Option<oneshot::Sender<()>>);
        impl Drop for ModelResource {
            fn drop(&mut self) {
                if let Some(released) = self.0.take() {
                    let _ = released.send(());
                }
            }
        }
        let (released, release_observed) = oneshot::channel();
        let model = LoadedModel::new(Some(Duration::from_millis(1)));
        let loaded = model
            .get_or_load(move || Ok(Arc::new(ModelResource(Some(released)))))
            .await
            .expect("load owned resource");
        drop(loaded);
        model.arm_unload().await;
        bounded(release_observed)
            .await
            .expect("actual model resource was released");
    }

    // InferenceGate deliberately uses Tokio scheduling and std::time::Instant.
    // This test controls permit ordering, not elapsed wall-clock time.
    #[tokio::test]
    async fn inference_gate_waits_until_the_only_permit_is_released() {
        let gate = InferenceGate::new(1);
        let (first, _) = gate.acquire().await.expect("first permit");
        let waiting = gate.acquire();
        tokio::pin!(waiting);

        tokio::select! {
            biased;
            result = &mut waiting => panic!("acquisition should remain pending: {result:?}"),
            _ = tokio::task::yield_now() => {}
        }

        drop(first);
        let (second, _) = bounded(waiting)
            .await
            .expect("released permit wakes waiter");
        drop(second);
    }

    #[tokio::test]
    async fn configured_parallelism_allows_two_acquisitions() {
        let gate = InferenceGate::new(2);
        let (first, _) = gate.acquire().await.expect("first permit");
        let (second, _) = bounded(gate.acquire()).await.expect("second permit");
        drop((first, second));
    }
}
