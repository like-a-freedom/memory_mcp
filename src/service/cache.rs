//! Context cache management.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use lru::LruCache;
use tokio::sync::RwLock;

use crate::models::AssembledContextItem;

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

    #[tokio::test]
    async fn invalidate_cache_by_scope_removes_matching_entries() {
        let cache: Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>> =
            Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(10).unwrap())));

        let cutoff = Utc::now();

        let key1 = CacheKey::new("query1", "org", cutoff, 5, None);
        let key2 = CacheKey::new("query2", "org", cutoff, 5, None);
        let key3 = CacheKey::new("query3", "personal", cutoff, 5, None);

        {
            let mut guard = cache.write().await;
            let item = |fact_id: &str| AssembledContextItem {
                fact_id: fact_id.to_string(),
                content: "content".to_string(),
                quote: "quote".to_string(),
                source_episode: "episode:test".to_string(),
                confidence: 0.9,
                provenance: json!({}),
                rationale: "rationale".to_string(),
            };
            guard.put(key1.clone(), vec![item("fact:1")]);
            guard.put(key2.clone(), vec![item("fact:2")]);
            guard.put(key3.clone(), vec![item("fact:3")]);
        }

        invalidate_cache_by_scope(&cache, "org").await;

        let mut guard = cache.write().await;
        assert!(guard.get(&key1).is_none());
        assert!(guard.get(&key2).is_none());
        assert!(guard.get(&key3).is_some());
    }
}
