//! Lazy load-on-demand with idle-unload for heavyweight model instances.
//!
//! `LazyModel<T>` defers construction of `T` until first use and unloads it
//! after a configurable idle period. Unload is armed at USE COMPLETION (never
//! on load), so a long-running extract cannot be interrupted mid-inference.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::service::MemoryError;

/// State of a lazily loaded model instance.
pub(super) struct LazyModelState<T> {
    loaded: Option<Arc<T>>,
    last_used: Instant,
    unload_handle: Option<tokio::task::JoinHandle<()>>,
}

/// A model that is constructed on first use and dropped after `idle_unload`
/// of inactivity. `None` disables unloading (model stays loaded forever).
pub(super) struct LazyModel<T> {
    state: Arc<Mutex<LazyModelState<T>>>,
    idle_unload: Option<Duration>,
}

impl<T: Send + Sync + 'static> LazyModel<T> {
    pub(super) fn new(idle_unload: Option<Duration>) -> Self {
        Self {
            state: Arc::new(Mutex::new(LazyModelState {
                loaded: None,
                last_used: Instant::now(),
                unload_handle: None,
            })),
            idle_unload,
        }
    }

    // get_or_load + arm_unload + spawn_unload_task are added in Step 3
    // (strict TDD: tests reference them before they exist).

    /// Returns the cached model, or constructs it exactly once under the
    /// state lock. The `load` closure runs on the blocking pool.
    /// Does NOT schedule an unload — call `arm_unload` after use.
    pub(super) async fn get_or_load<F>(&self, load: F) -> Result<Arc<T>, MemoryError>
    where
        F: FnOnce() -> Result<Arc<T>, MemoryError> + Send + 'static,
    {
        let mut guard = self.state.lock().await;
        if guard.loaded.is_some() {
            guard.last_used = Instant::now();
            if let Some(handle) = guard.unload_handle.take() {
                handle.abort();
            }
            let loaded = guard.loaded.as_ref().expect("checked above");
            return Ok(Arc::clone(loaded));
        }
        let loaded = tokio::task::spawn_blocking(load)
            .await
            .map_err(|err| MemoryError::Storage(format!("model load task panicked: {err}")))??;
        guard.last_used = Instant::now();
        guard.loaded = Some(Arc::clone(&loaded));
        Ok(loaded)
    }

    /// Records that the model was used and (re)arms the idle-unload timer.
    /// The idle clock starts at USE COMPLETION, so an unload can never fire
    /// while an extract is still running.
    pub(super) async fn arm_unload(&self) {
        let mut guard = self.state.lock().await;
        guard.last_used = Instant::now();
        if let Some(handle) = guard.unload_handle.take() {
            handle.abort();
        }
        guard.unload_handle = self
            .idle_unload
            .map(|timeout| Self::spawn_unload_task(Arc::clone(&self.state), timeout));
    }

    fn spawn_unload_task(
        state: Arc<Mutex<LazyModelState<T>>>,
        timeout: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            let mut guard = state.lock().await;
            if guard.last_used.elapsed() >= timeout {
                guard.loaded = None;
                guard.unload_handle = None;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_counter() -> Arc<AtomicUsize> {
        Arc::new(AtomicUsize::new(0))
    }

    fn fake_load(
        calls: &Arc<AtomicUsize>,
    ) -> impl FnOnce() -> Result<Arc<String>, MemoryError> + Send + 'static {
        let calls = Arc::clone(calls);
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new("model".to_string()))
        }
    }

    #[tokio::test]
    async fn constructs_on_first_call() {
        let calls = make_counter();
        let model = LazyModel::<String>::new(Some(Duration::from_secs(60)));
        let value = model.get_or_load(fake_load(&calls)).await.unwrap();
        assert_eq!(*value, "model");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn caches_within_idle_timeout() {
        let calls = make_counter();
        let model = LazyModel::<String>::new(Some(Duration::from_secs(60)));
        model.get_or_load(fake_load(&calls)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        model.get_or_load(fake_load(&calls)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unloads_after_idle_timeout() {
        let calls = make_counter();
        let model = LazyModel::<String>::new(Some(Duration::from_millis(60)));
        model.get_or_load(fake_load(&calls)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // The idle clock starts at the end of use.
        model.arm_unload().await;
        await_unloaded(&model).await;
        // A subsequent call must rebuild.
        model.get_or_load(fake_load(&calls)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn arm_after_use_resets_the_idle_timer() {
        let calls = make_counter();
        let model = LazyModel::<String>::new(Some(Duration::from_millis(500)));
        // t=0: load + first use completes -> arm (task A fires at t=500).
        model.get_or_load(fake_load(&calls)).await.unwrap();
        model.arm_unload().await;
        // t=100ms: a new use starts and completes -> get + arm (cancels A,
        // task B fires at t=600).
        tokio::time::sleep(Duration::from_millis(100)).await;
        model.get_or_load(fake_load(&calls)).await.unwrap();
        model.arm_unload().await;
        // t=200ms: inside the fresh 500ms window since the last arm.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!({ model.state.lock().await.loaded.is_some() });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        await_unloaded(&model).await;
    }

    #[tokio::test]
    async fn no_unload_before_first_arm() {
        // The idle clock only starts after the first completed use; a freshly
        // loaded model with no arm yet must stay loaded past the timeout.
        let calls = make_counter();
        let model = LazyModel::<String>::new(Some(Duration::from_millis(40)));
        model.get_or_load(fake_load(&calls)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert!({ model.state.lock().await.loaded.is_some() });
        // First arm schedules the unload.
        model.arm_unload().await;
        await_unloaded(&model).await;
    }

    #[tokio::test]
    async fn concurrent_loads_construct_exactly_once() {
        let calls = make_counter();
        let model = Arc::new(LazyModel::<String>::new(None));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let m = Arc::clone(&model);
            let c = Arc::clone(&calls);
            handles.push(tokio::spawn(async move {
                m.get_or_load(fake_load(&c)).await.unwrap();
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn disabled_unload_keeps_model_loaded() {
        let calls = make_counter();
        let model = LazyModel::<String>::new(None);
        model.get_or_load(fake_load(&calls)).await.unwrap();
        model.arm_unload().await; // idle_unload=None -> no task scheduled
        tokio::time::sleep(Duration::from_millis(30)).await;
        model.get_or_load(fake_load(&calls)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    async fn await_unloaded(model: &LazyModel<String>) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let unloaded = { model.state.lock().await.loaded.is_none() };
            if unloaded {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "model was not unloaded within the deadline"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
