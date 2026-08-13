use std::collections::HashMap;
use std::sync::Arc;

use lru::LruCache;
use serde_json::json;
use tokio::sync::RwLock;

use super::CacheKey;
use crate::logging::{LogLevel, StdoutLogger};
use crate::models::AssembledContextItem;

fn cache_logger() -> StdoutLogger {
    StdoutLogger::new("trace")
}

/// Invalidate all cached context results for the process-bound namespace.
pub async fn invalidate_cache(cache: &Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>) {
    let mut guard = cache.write().await;
    let count = guard.len();
    guard.clear();
    if count > 0 {
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("cache.invalidate"));
        event.insert("invalidated_count".to_string(), json!(count));
        cache_logger().log(event, LogLevel::Trace);
    }
}
