//! Process-signal watcher. Closes the admission gate and begins
//! shutdown on the first SIGINT or SIGTERM.

use std::sync::Arc;

use crate::http::runtime::pool::AdmissionGate;
use crate::http::shutdown::ShutdownState;

/// Spawn a task that watches SIGINT/SIGTERM and on the first signal:
/// 1. closes the admission gate (so `/health/ready` flips to 503),
/// 2. calls `shutdown.begin()` so axum's graceful-shutdown fires.
///
/// On non-unix the watcher falls back to `ctrl_c`.
pub fn spawn(shutdown: ShutdownState, admission: Arc<AdmissionGate>) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            let mut terminate = signal(SignalKind::terminate()).ok();
            if let Some(t) = terminate.as_mut() {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = t.recv() => {}
                }
            } else {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        admission.close();
        shutdown.begin();
    });
}
