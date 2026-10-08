//! Byte-bounded LRU for query embeddings with operation-driven expiry cleanup.

use std::mem::size_of;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use lru::LruCache;

const CACHE_ENTRY_NODE_ALLOWANCE: usize = 128;

struct QueryEmbeddingEntry {
    embedding: Vec<f64>,
    expires_at: Instant,
    accounted_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryCacheInsertOutcome {
    Stored,
    Oversized,
    InvalidExpiry,
}

/// Process-local query embedding cache with entry, byte and TTL limits.
pub(crate) struct QueryEmbeddingCacheState {
    entries: LruCache<String, QueryEmbeddingEntry>,
    max_bytes: NonZeroUsize,
    ttl: Duration,
    accounted_bytes: usize,
}

impl QueryEmbeddingCacheState {
    #[must_use]
    pub(crate) fn new(max_entries: NonZeroUsize, max_bytes: NonZeroUsize, ttl: Duration) -> Self {
        Self {
            entries: LruCache::new(max_entries),
            max_bytes,
            ttl,
            accounted_bytes: 0,
        }
    }

    /// Returns an unexpired embedding and removes every expired entry seen in
    /// this state before cloning the hit.
    pub(crate) fn get(&mut self, key: &str, now: Instant) -> Option<Vec<f64>> {
        self.purge_expired(now);
        self.entries.get(key).map(|entry| entry.embedding.clone())
    }

    /// Retains the owned key/vector if they fit; oversize candidates bypass the
    /// cache without changing the provider result returned by the caller.
    pub(crate) fn insert(
        &mut self,
        key: String,
        embedding: Vec<f64>,
        now: Instant,
    ) -> QueryCacheInsertOutcome {
        let Some(expires_at) = now.checked_add(self.ttl) else {
            return QueryCacheInsertOutcome::InvalidExpiry;
        };
        self.purge_expired(now);

        let Some(weight) = estimated_entry_bytes(key.capacity(), embedding.capacity()) else {
            self.remove(&key);
            return QueryCacheInsertOutcome::Oversized;
        };
        if weight > self.max_bytes.get() {
            self.remove(&key);
            return QueryCacheInsertOutcome::Oversized;
        }

        self.remove(&key);
        while self.entries.len() >= self.entries.cap().get()
            || self.accounted_bytes() > self.max_bytes.get() - weight
        {
            let Some((_, entry)) = self.entries.pop_lru() else {
                return QueryCacheInsertOutcome::Oversized;
            };
            self.accounted_bytes = self.accounted_bytes.saturating_sub(entry.accounted_bytes);
        }

        self.entries.put(
            key,
            QueryEmbeddingEntry {
                embedding,
                expires_at,
                accounted_bytes: weight,
            },
        );
        self.accounted_bytes = self.accounted_bytes.saturating_add(weight);
        QueryCacheInsertOutcome::Stored
    }

    /// Removes all entries whose expiry is at or before `now`.
    pub(crate) fn purge_expired(&mut self, now: Instant) -> usize {
        let entries_to_check = self.entries.len();
        let mut purged = 0;
        for _ in 0..entries_to_check {
            let Some((key, entry)) = self.entries.pop_lru() else {
                break;
            };
            if entry.expires_at <= now {
                self.accounted_bytes = self.accounted_bytes.saturating_sub(entry.accounted_bytes);
                purged += 1;
            } else {
                self.entries.put(key, entry);
            }
        }
        purged
    }

    #[must_use]
    pub(crate) fn accounted_bytes(&self) -> usize {
        self.accounted_bytes
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    #[cfg(test)]
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.pop(key) {
            self.accounted_bytes = self.accounted_bytes.saturating_sub(entry.accounted_bytes);
        }
    }
}

fn estimated_entry_bytes(key_capacity: usize, embedding_capacity: usize) -> Option<usize> {
    size_of::<String>()
        .checked_add(size_of::<QueryEmbeddingEntry>())?
        .checked_add(CACHE_ENTRY_NODE_ALLOWANCE)?
        .checked_add(key_capacity)?
        .checked_add(embedding_capacity.checked_mul(size_of::<f64>())?)
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;
    use std::time::{Duration, Instant};

    use super::{QueryCacheInsertOutcome, QueryEmbeddingCacheState};

    fn cache(max_bytes: usize) -> QueryEmbeddingCacheState {
        QueryEmbeddingCacheState::new(
            NonZeroUsize::new(8).expect("nonzero entry cap"),
            NonZeroUsize::new(max_bytes).expect("nonzero byte cap"),
            Duration::from_secs(5),
        )
    }

    #[test]
    fn exact_expiry_is_a_cache_miss() {
        let start = Instant::now();
        let mut cache = cache(4_096);
        assert_eq!(
            cache.insert("provider/query".to_string(), vec![0.25, 0.5], start),
            QueryCacheInsertOutcome::Stored
        );

        assert_eq!(
            cache.get("provider/query", start + Duration::from_secs(5)),
            None
        );
    }

    #[test]
    fn looking_up_another_key_purges_expired_entries() {
        let start = Instant::now();
        let mut cache = cache(4_096);
        assert_eq!(
            cache.insert("expired".to_string(), vec![1.0], start),
            QueryCacheInsertOutcome::Stored
        );
        assert_eq!(
            cache.insert(
                "live".to_string(),
                vec![2.0],
                start + Duration::from_secs(2)
            ),
            QueryCacheInsertOutcome::Stored
        );
        let bytes_before_purge = cache.accounted_bytes();

        assert_eq!(
            cache.get("live", start + Duration::from_secs(5)),
            Some(vec![2.0])
        );
        assert_eq!(cache.len(), 1);
        assert!(cache.accounted_bytes() < bytes_before_purge);
    }

    #[test]
    fn insertion_purges_expired_mru_before_evicting_a_live_lru() {
        let start = Instant::now();
        let mut cache = cache(500);
        assert_eq!(
            cache.insert("expired".to_string(), vec![1.0], start),
            QueryCacheInsertOutcome::Stored
        );
        assert_eq!(
            cache.insert(
                "live".to_string(),
                vec![2.0],
                start + Duration::from_secs(1)
            ),
            QueryCacheInsertOutcome::Stored
        );
        assert_eq!(
            cache.get("expired", start + Duration::from_secs(2)),
            Some(vec![1.0])
        );

        assert_eq!(
            cache.insert("new".to_string(), vec![3.0], start + Duration::from_secs(5)),
            QueryCacheInsertOutcome::Stored
        );
        assert_eq!(
            cache.get("live", start + Duration::from_secs(5)),
            Some(vec![2.0])
        );
        assert_eq!(cache.get("expired", start + Duration::from_secs(5)), None);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn cache_hit_does_not_extend_expiry() {
        let start = Instant::now();
        let mut cache = cache(4_096);
        assert_eq!(
            cache.insert("provider/query".to_string(), vec![0.25], start),
            QueryCacheInsertOutcome::Stored
        );

        assert_eq!(
            cache.get("provider/query", start + Duration::from_secs(4)),
            Some(vec![0.25])
        );
        assert_eq!(
            cache.get("provider/query", start + Duration::from_secs(5)),
            None
        );
    }

    #[test]
    fn replacing_an_entry_releases_its_previous_weight() {
        let start = Instant::now();
        let mut cache = cache(600);
        assert_eq!(
            cache.insert("same".to_string(), vec![1.0], start),
            QueryCacheInsertOutcome::Stored
        );
        let mut replacement = Vec::with_capacity(40);
        replacement.push(9.0);

        assert_eq!(
            cache.insert(
                "same".to_string(),
                replacement,
                start + Duration::from_secs(1)
            ),
            QueryCacheInsertOutcome::Stored
        );
        assert_eq!(cache.len(), 1);
        assert!(cache.accounted_bytes() <= 600);
        assert_eq!(
            cache.get("same", start + Duration::from_secs(2)),
            Some(vec![9.0])
        );
    }

    #[test]
    fn vector_spare_capacity_is_charged_and_oversized_vectors_bypass() {
        let start = Instant::now();
        let mut cache = cache(512);
        assert_eq!(
            cache.insert("small".to_string(), vec![1.0], start),
            QueryCacheInsertOutcome::Stored
        );
        let mut oversized = Vec::with_capacity(128);
        oversized.push(2.0);

        assert_eq!(
            cache.insert("oversized".to_string(), oversized, start),
            QueryCacheInsertOutcome::Oversized
        );
        assert_eq!(cache.len(), 1);
        assert!(cache.get("small", start).is_some());
        assert!(cache.get("oversized", start).is_none());
    }

    #[test]
    fn expiry_overflow_is_reported_without_retaining_the_vector() {
        let start = Instant::now();
        assert!(start.checked_add(Duration::MAX).is_none());
        let mut cache = QueryEmbeddingCacheState::new(
            NonZeroUsize::new(8).expect("nonzero entry cap"),
            NonZeroUsize::new(4_096).expect("nonzero byte cap"),
            Duration::MAX,
        );

        assert_eq!(
            cache.insert("provider/query".to_string(), vec![0.25], start),
            QueryCacheInsertOutcome::InvalidExpiry
        );
        assert!(cache.is_empty());
    }
}
