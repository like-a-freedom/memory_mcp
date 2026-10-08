use std::collections::HashMap;

use serde_json::json;

use super::{ContextCache, InvalidateContextCache};
use crate::logging::LogLevel;

/// Invalidate all cached context results for the process-bound namespace.
pub async fn invalidate_cache(cache: &ContextCache) {
    let mut guard = cache.write().await;
    let count = guard.len();
    guard.clear();
    if count > 0 {
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("cache.invalidate"));
        event.insert("invalidated_count".to_string(), json!(count));
        // A trace event the operator opts into with `RUST_LOG=trace`; it is not
        // forced on, so the documented dial governs it like every other event.
        crate::logging::emit(event, LogLevel::Trace);
    }
}

impl InvalidateContextCache for ContextCache {
    fn invalidate_context_cache(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        Box::pin(invalidate_cache(self))
    }
}
