use std::collections::HashMap;
use std::sync::Arc;

use lru::LruCache;
use serde_json::json;
use tokio::sync::RwLock;

use super::CacheKey;
use crate::logging::LogLevel;
use crate::models::AssembledContextItem;

/// Invalidate all cached context results for the process-bound namespace.
pub async fn invalidate_cache(cache: &Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>) {
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
