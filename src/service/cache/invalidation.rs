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

/// Invalidate cache entries for a specific scope.
pub async fn invalidate_cache_by_scope(
    cache: &Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>>,
    scope: &str,
) {
    let mut guard = cache.write().await;
    let keys_to_remove: Vec<CacheKey> = guard
        .iter()
        .filter(|(key, _)| key.matches_scope(scope))
        .map(|(key, _)| key.clone())
        .collect();

    let count = keys_to_remove.len();
    if count > 0 {
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("cache.invalidate_by_scope"));
        event.insert("scope".to_string(), json!(scope));
        event.insert("invalidated_count".to_string(), json!(count));
        cache_logger().log(event, LogLevel::Trace);
    }

    for key in keys_to_remove {
        guard.pop(&key);
    }
}
