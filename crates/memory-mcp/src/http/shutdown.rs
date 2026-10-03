//! Coordinated shutdown state for the HTTP profile.
//!
//! Each `HttpState` owns its own `ShutdownState` so test/application
//! instances do not share a cancelled token.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ShutdownState {
    flag: Arc<AtomicBool>,
    token: CancellationToken,
    publication: Arc<Mutex<()>>,
}

impl Default for ShutdownState {
    fn default() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            token: CancellationToken::new(),
            publication: Arc::new(Mutex::new(())),
        }
    }
}

impl ShutdownState {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn is_shutting_down(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }
    pub fn begin(&self) {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !self.flag.swap(true, Ordering::SeqCst) {
            self.token.cancel();
        }
    }

    /// Run one synchronous publication step only if it linearizes before
    /// shutdown. `begin` takes the same lock before setting the flag, so a
    /// runtime cannot transition to Ready and an acquisition cannot hand out a
    /// new lease after shutdown wins this gate.
    pub(crate) fn publish_while_running<T>(&self, publish: impl FnOnce() -> T) -> Option<T> {
        let _publication = self
            .publication
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if self.is_shutting_down() {
            return None;
        }
        Some(publish())
    }
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}
