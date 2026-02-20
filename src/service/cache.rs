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
pub fn invalidate_cache_by_scope(cache: &Arc<Mutex<LruCache<CacheKey, Vec<Value>>>>, scope: &str) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use serde_json::json;
    use std::num::NonZeroUsize;

    #[test]
    fn cache_key_new_normalizes_query() {
        let cutoff = Utc.with_ymd_and_hms(2024, 1, 1, 12, 30, 0).unwrap();
        let key = CacheKey::new("  Test Query  ", "org", cutoff, 5, None);
        assert_eq!(key.query, "test query");
        assert_eq!(key.scope, "org");
        assert_eq!(key.budget, 5);
    }

    #[test]
    fn cache_key_new_buckets_cutoff_to_hour() {
        let cutoff = Utc.with_ymd_and_hms(2024, 1, 1, 12, 30, 45).unwrap();
        let key = CacheKey::new("query", "org", cutoff, 5, None);
        assert_eq!(key.cutoff, "2024-01-01T12:00:00Z");
    }

    #[test]
    fn cache_key_new_sorts_tags() {
        let cutoff = Utc::now();
        let tags = Some(vec!["zebra".to_string(), "apple".to_string()]);
        let key = CacheKey::new("query", "org", cutoff, 5, tags);
        assert_eq!(
            key.tags,
            Some(vec!["apple".to_string(), "zebra".to_string()])
        );
    }

    #[test]
    fn cache_key_matches_scope_returns_true_for_same_scope() {
        let cutoff = Utc::now();
        let key = CacheKey::new("query", "org", cutoff, 5, None);
        assert!(key.matches_scope("org"));
    }

    #[test]
    fn cache_key_matches_scope_returns_false_for_different_scope() {
        let cutoff = Utc::now();
        let key = CacheKey::new("query", "org", cutoff, 5, None);
        assert!(!key.matches_scope("personal"));
    }

    #[test]
    fn safe_mutex_handles_poisoned_lock() {
        let mutex = Mutex::new(42);
        let guard = mutex.safe_lock();
        assert_eq!(*guard, 42);
        drop(guard);

        // Poison the mutex
        let _ = std::panic::catch_unwind(|| {
            let _guard = mutex.lock().unwrap();
            panic!("poison");
        });

        // Should still be able to lock
        let guard = mutex.safe_lock();
        assert_eq!(*guard, 42);
    }

    #[test]
    fn invalidate_cache_by_scope_removes_matching_entries() {
        let cache: Arc<Mutex<LruCache<CacheKey, Vec<Value>>>> =
            Arc::new(Mutex::new(LruCache::new(NonZeroUsize::new(10).unwrap())));

        let cutoff = Utc::now();

        // Add entries for different scopes
        let key1 = CacheKey::new("query1", "org", cutoff, 5, None);
        let key2 = CacheKey::new("query2", "org", cutoff, 5, None);
        let key3 = CacheKey::new("query3", "personal", cutoff, 5, None);

        {
            let mut guard = cache.safe_lock();
            guard.put(key1.clone(), vec![json!("value1")]);
            guard.put(key2.clone(), vec![json!("value2")]);
            guard.put(key3.clone(), vec![json!("value3")]);
        }

        // Invalidate org scope
        invalidate_cache_by_scope(&cache, "org");

        // Check that org entries are removed but personal remains
        let mut guard = cache.safe_lock();
        assert!(guard.get(&key1).is_none());
        assert!(guard.get(&key2).is_none());
        assert!(guard.get(&key3).is_some());
    }
}
