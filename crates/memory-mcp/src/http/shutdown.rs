//! Coordinated shutdown state for the HTTP profile (spec §17).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

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

/// Process-global cancellation token used as the default for the
/// Streamable HTTP service config. Test/application code that needs
/// an independent lifecycle should construct a `ShutdownState` and
/// pass its token to `transport::build_server_config`.
pub fn cancellation_token() -> CancellationToken {
    static CT: OnceLock<CancellationToken> = OnceLock::new();
    CT.get_or_init(CancellationToken::new).clone()
}
