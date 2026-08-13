//! Context cache management.

pub use invalidation::invalidate_cache;
pub use key::{CacheKey, CacheView};

mod invalidation;
mod key;

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use lru::LruCache;
    use serde_json::json;
    use std::num::NonZeroUsize;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::models::AssembledContextItem;

    #[test]
    fn cache_key_new_normalizes_query() {
        let cutoff = Utc.with_ymd_and_hms(2024, 1, 1, 12, 30, 0).unwrap();
        let key = CacheKey::new("  Test Query  ", cutoff, 5, &[], CacheView::default(), None);
        assert_eq!(key.query, "test query");
        assert_eq!(key.budget, 5);
    }

    #[test]
    fn cache_key_new_buckets_cutoff_to_five_minutes() {
        let cutoff = Utc.with_ymd_and_hms(2024, 1, 1, 12, 34, 45).unwrap();
        let key = CacheKey::new("query", cutoff, 5, &[], CacheView::default(), None);
        assert_eq!(key.cutoff, "2024-01-01T12:30:00Z");
    }

    #[test]
    fn cache_key_new_distinguishes_adjacent_five_minute_buckets() {
        let key1 = CacheKey::new(
            "query",
            Utc.with_ymd_and_hms(2024, 1, 1, 12, 34, 59).unwrap(),
            5,
            &[],
            CacheView::default(),
            None,
        );
        let key2 = CacheKey::new(
            "query",
            Utc.with_ymd_and_hms(2024, 1, 1, 12, 35, 0).unwrap(),
            5,
            &[],
            CacheView::default(),
            None,
        );

        assert_ne!(key1.cutoff, key2.cutoff);
    }

    #[test]
    fn cache_view_new_buckets_window_bounds_to_five_minutes() {
        let view = CacheView::new(
            Some("timeline"),
            Some(Utc.with_ymd_and_hms(2024, 1, 1, 12, 34, 59).unwrap()),
            Some(Utc.with_ymd_and_hms(2024, 1, 1, 13, 2, 1).unwrap()),
        );

        assert_eq!(view.window_start.as_deref(), Some("2024-01-01T12:30:00Z"));
        assert_eq!(view.window_end.as_deref(), Some("2024-01-01T13:00:00Z"));
    }

    #[test]
    fn cache_key_new_sorts_tags() {
        let key = CacheKey::new(
            "query",
            Utc::now(),
            5,
            &[],
            CacheView::default(),
            Some(vec!["zebra".to_string(), "apple".to_string()]),
        );
        assert_eq!(
            key.tags,
            Some(vec!["apple".to_string(), "zebra".to_string()])
        );
    }

    #[test]
    fn cache_key_new_sorts_and_deduplicates_fact_types() {
        let fact_types = vec![
            "promise".to_string(),
            "metric".to_string(),
            "promise".to_string(),
        ];
        let key = CacheKey::new(
            "query",
            Utc::now(),
            5,
            &fact_types,
            CacheView::default(),
            None,
        );

        assert_eq!(
            key.fact_types,
            vec!["metric".to_string(), "promise".to_string()]
        );
    }

    #[tokio::test]
    async fn invalidate_cache_clears_all_process_local_entries() {
        let cache: Arc<RwLock<LruCache<CacheKey, Vec<AssembledContextItem>>>> =
            Arc::new(RwLock::new(LruCache::new(NonZeroUsize::new(10).unwrap())));
        let cutoff = Utc::now();
        let key1 = CacheKey::new("query1", cutoff, 5, &[], CacheView::default(), None);
        let key2 = CacheKey::new("query2", cutoff, 5, &[], CacheView::default(), None);

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
                retrieval_tier: None,
                ..Default::default()
            };
            guard.put(key1.clone(), vec![item("fact:1")]);
            guard.put(key2.clone(), vec![item("fact:2")]);
        }

        invalidate_cache(&cache).await;

        let mut guard = cache.write().await;
        assert!(guard.get(&key1).is_none());
        assert!(guard.get(&key2).is_none());
        assert!(guard.is_empty());
    }
}
