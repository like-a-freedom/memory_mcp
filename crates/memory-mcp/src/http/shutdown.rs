//! Coordinated shutdown state for the HTTP profile (spec §17).
//!
//! Each `HttpState` owns its own `ShutdownState` so test/application
//! instances do not share a cancelled token.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio_util::sync::CancellationToken;

#[derive(Clone)]
pub struct ShutdownState {
    flag: Arc<AtomicBool>,
    token: CancellationToken,
}

impl Default for ShutdownState {
    fn default() -> Self {
        Self {
            flag: Arc::new(AtomicBool::new(false)),
            token: CancellationToken::new(),
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
        self.flag.store(true, Ordering::SeqCst);
        self.token.cancel();
    }
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}
