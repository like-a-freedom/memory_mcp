//! Context cache management.

use std::sync::{Arc, Mutex, PoisonError};

use chrono::{DateTime, Utc};
use lru::LruCache;
use serde_json::Value;

/// Cache key for context assembly results.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    query: String,
    scope: String,
    cutoff: String,
    budget: i32,
    tags: Option<Vec<String>>,
}

impl CacheKey {
    #[must_use]
    pub fn new(
        query: &str,
        scope: &str,
        cutoff: DateTime<Utc>,
        budget: i32,
        tags: Option<Vec<String>>,
    ) -> Self {
        let mut tags = tags;
        if let Some(ref mut tag_list) = tags {
            tag_list.sort();
        }
        Self {
            query: super::normalize_text(query),
            scope: scope.to_string(),
            cutoff: super::bucket_to_hour(cutoff),
            budget,
            tags,
        }
    }

    /// Check if this cache key matches the given scope.
    #[must_use]
    pub fn matches_scope(&self, scope: &str) -> bool {
        self.scope == scope
    }
}

/// Trait for safe mutex locking that handles poisoned locks gracefully.
pub trait SafeMutex<T> {
    fn safe_lock(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> SafeMutex<T> for Mutex<T> {
    fn safe_lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Invalidate cache entries for a specific scope.
pub fn invalidate_cache_by_scope(
    cache: &Arc<Mutex<LruCache<CacheKey, Vec<Value>>>>,
    scope: &str,
) {
    let mut guard = cache.safe_lock();
    let keys_to_remove: Vec<CacheKey> = guard
        .iter()
        .filter(|(key, _)| key.matches_scope(scope))
        .map(|(key, _)| key.clone())
        .collect();
    for key in keys_to_remove {
        guard.pop(&key);
    }
}
