//! Weighted, generation-fenced storage for assembled context results.
//!
//! The byte count is a conservative retained-allocation estimate, not allocator
//! telemetry. It includes owned capacities and a fixed allowance for cache and
//! nested JSON nodes; it is intended to make retention bounded across payload
//! shapes, not to predict RSS exactly.

use std::mem::size_of;
use std::num::NonZeroUsize;

use lru::LruCache;

use crate::models::{AssembledContextItem, ClaimReconciliationMetadata, ClaimRelationSummary};
use crate::platform::context_cache_key::CacheKey;

// Estimated allocator/hash/list overhead per retained LRU key/value node.
const CACHE_ENTRY_NODE_ALLOWANCE: usize = 128;
// Estimated tree/array node overhead for each retained nested JSON value.
const JSON_NODE_ALLOWANCE: usize = 64;

/// The cache generation observed by a retrieval miss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheGeneration(u64);

/// A cache result, including the generation a later insertion must still match.
#[derive(Debug, Clone, PartialEq)]
pub enum ContextCacheLookup {
    Hit(Vec<AssembledContextItem>),
    Miss(CacheGeneration),
}

/// Result of trying to retain an assembled context result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheInsertOutcome {
    Stored,
    Oversized,
    StaleGeneration,
}

struct CacheEntry {
    items: Vec<AssembledContextItem>,
    accounted_bytes: usize,
}

/// LRU state with entry and estimated-byte limits.
///
/// The raw LRU is private so callers cannot mutate entries without updating
/// accounting. A generation is captured on misses and advanced on clear,
/// preventing in-flight retrievals from repopulating invalidated results.
pub struct ContextCacheState {
    entries: LruCache<CacheKey, CacheEntry>,
    max_bytes: NonZeroUsize,
    accounted_bytes: usize,
    generation: u64,
    generation_exhausted: bool,
}

impl ContextCacheState {
    #[must_use]
    pub fn new(max_entries: NonZeroUsize, max_bytes: NonZeroUsize) -> Self {
        Self {
            entries: LruCache::new(max_entries),
            max_bytes,
            accounted_bytes: 0,
            generation: 0,
            generation_exhausted: false,
        }
    }

    /// Returns a cloned hit, or a token that fences a later insertion.
    pub fn lookup(&mut self, key: &CacheKey) -> ContextCacheLookup {
        match self.entries.get(key) {
            Some(entry) => ContextCacheLookup::Hit(entry.items.clone()),
            None => ContextCacheLookup::Miss(CacheGeneration(self.generation)),
        }
    }

    /// Retains results only if they fit both limits and the miss generation is
    /// still current. The candidate is measured before its payload is cloned.
    pub fn insert(
        &mut self,
        generation: CacheGeneration,
        key: CacheKey,
        items: &[AssembledContextItem],
    ) -> CacheInsertOutcome {
        if self.generation_exhausted || generation.0 != self.generation {
            return CacheInsertOutcome::StaleGeneration;
        }

        let Some(weight) = estimated_entry_bytes(&key, items) else {
            self.remove(&key);
            return CacheInsertOutcome::Oversized;
        };
        if weight > self.max_bytes.get() {
            self.remove(&key);
            return CacheInsertOutcome::Oversized;
        }

        self.remove(&key);
        while self.entries.len() >= self.entries.cap().get()
            || self.accounted_bytes > self.max_bytes.get() - weight
        {
            let Some((_, entry)) = self.entries.pop_lru() else {
                break;
            };
            self.accounted_bytes = self.accounted_bytes.saturating_sub(entry.accounted_bytes);
        }

        self.entries.put(
            key,
            CacheEntry {
                items: items.to_vec(),
                accounted_bytes: weight,
            },
        );
        self.accounted_bytes = self.accounted_bytes.saturating_add(weight);
        CacheInsertOutcome::Stored
    }

    /// Clears retained entries and invalidates miss tokens issued earlier.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.accounted_bytes = 0;
        match self.generation.checked_add(1) {
            Some(next) => self.generation = next,
            None => self.generation_exhausted = true,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[must_use]
    pub fn accounted_bytes(&self) -> usize {
        self.accounted_bytes
    }

    fn remove(&mut self, key: &CacheKey) {
        if let Some(entry) = self.entries.pop(key) {
            self.accounted_bytes = self.accounted_bytes.saturating_sub(entry.accounted_bytes);
        }
    }
}

fn estimated_entry_bytes(key: &CacheKey, items: &[AssembledContextItem]) -> Option<usize> {
    let mut bytes = size_of::<CacheKey>()
        .checked_add(size_of::<CacheEntry>())?
        .checked_add(CACHE_ENTRY_NODE_ALLOWANCE)?;
    add_string(&mut bytes, &key.query)?;
    add_string(&mut bytes, &key.cutoff)?;
    add_string_vec(&mut bytes, &key.fact_types, key.fact_types.capacity())?;
    if let Some(tags) = &key.tags {
        add_string_vec(&mut bytes, tags, tags.capacity())?;
    }
    if let Some(view_mode) = &key.view.view_mode {
        add_string(&mut bytes, view_mode)?;
    }
    if let Some(window_start) = &key.view.window_start {
        add_string(&mut bytes, window_start)?;
    }
    if let Some(window_end) = &key.view.window_end {
        add_string(&mut bytes, window_end)?;
    }
    add_bytes(
        &mut bytes,
        items.len().checked_mul(size_of::<AssembledContextItem>())?,
    )?;
    for item in items {
        add_string(&mut bytes, &item.fact_id)?;
        add_string(&mut bytes, &item.content)?;
        add_string(&mut bytes, &item.quote)?;
        add_string(&mut bytes, &item.source_episode)?;
        add_bytes(&mut bytes, json_heap_bytes(&item.provenance)?)?;
        add_string(&mut bytes, &item.rationale)?;
        if let Some(retrieval_tier) = &item.retrieval_tier {
            add_string(&mut bytes, retrieval_tier)?;
        }
        if let Some(reconciliation) = &item.reconciliation {
            add_reconciliation(&mut bytes, reconciliation)?;
        }
    }
    Some(bytes)
}

fn add_reconciliation(
    bytes: &mut usize,
    reconciliation: &ClaimReconciliationMetadata,
) -> Option<()> {
    add_bytes(
        bytes,
        reconciliation
            .claim_ids
            .capacity()
            .checked_mul(size_of::<String>())?,
    )?;
    for claim_id in &reconciliation.claim_ids {
        add_string(bytes, claim_id)?;
    }
    add_bytes(
        bytes,
        reconciliation
            .relations
            .capacity()
            .checked_mul(size_of::<ClaimRelationSummary>())?,
    )?;
    for relation in &reconciliation.relations {
        add_string(bytes, &relation.relation_id)?;
        if let Some(episode_id) = &relation.counterpart_source_episode_id {
            add_string(bytes, episode_id)?;
        }
        if let Some(fact_id) = &relation.superseded_by_fact_id {
            add_string(bytes, fact_id)?;
        }
        add_string(bytes, &relation.reason_code)?;
        add_string(bytes, &relation.evaluator_version)?;
    }
    Some(())
}

fn json_heap_bytes(value: &serde_json::Value) -> Option<usize> {
    match value {
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            Some(0)
        }
        serde_json::Value::String(value) => Some(value.capacity()),
        serde_json::Value::Array(values) => {
            let mut bytes = values
                .capacity()
                .checked_mul(size_of::<serde_json::Value>())?
                .checked_add(values.capacity().checked_mul(JSON_NODE_ALLOWANCE)?)?;
            for value in values {
                add_bytes(&mut bytes, json_heap_bytes(value)?)?;
            }
            Some(bytes)
        }
        serde_json::Value::Object(values) => {
            let mut bytes = 0;
            for (key, value) in values {
                add_string(&mut bytes, key)?;
                add_bytes(
                    &mut bytes,
                    size_of::<serde_json::Value>().checked_add(JSON_NODE_ALLOWANCE)?,
                )?;
                add_bytes(&mut bytes, json_heap_bytes(value)?)?;
            }
            Some(bytes)
        }
    }
}

fn add_string_vec(bytes: &mut usize, values: &[String], capacity: usize) -> Option<()> {
    add_bytes(bytes, capacity.checked_mul(size_of::<String>())?)?;
    for value in values {
        add_string(bytes, value)?;
    }
    Some(())
}

fn add_string(bytes: &mut usize, value: &String) -> Option<()> {
    add_bytes(bytes, value.capacity())
}

fn add_bytes(bytes: &mut usize, amount: usize) -> Option<()> {
    *bytes = bytes.checked_add(amount)?;
    Some(())
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroUsize;

    use chrono::{TimeZone, Utc};

    use crate::models::AssembledContextItem;
    use crate::platform::context_cache_key::{CacheKey, CacheView};

    use super::{CacheInsertOutcome, ContextCacheLookup, ContextCacheState};

    fn key(query: &str) -> CacheKey {
        CacheKey::new(
            query,
            Utc.with_ymd_and_hms(2026, 10, 7, 10, 0, 0)
                .single()
                .expect("valid timestamp"),
            5,
            &[],
            CacheView::default(),
            None,
        )
    }

    fn item(content_len: usize) -> AssembledContextItem {
        AssembledContextItem {
            fact_id: "fact:test".to_string(),
            content: "x".repeat(content_len),
            source_episode: "episode:test".to_string(),
            provenance: serde_json::json!({}),
            ..Default::default()
        }
    }

    #[test]
    fn byte_cap_evicts_before_entry_cap() {
        let mut cache = ContextCacheState::new(
            NonZeroUsize::new(10).expect("nonzero entry cap"),
            NonZeroUsize::new(1_300).expect("nonzero byte cap"),
        );
        let first_key = key("first");
        let second_key = key("second");
        let first_generation = match cache.lookup(&first_key) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("new cache cannot contain a hit"),
        };
        assert_eq!(
            cache.insert(first_generation, first_key.clone(), &[item(300)]),
            CacheInsertOutcome::Stored
        );
        let second_generation = match cache.lookup(&second_key) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("second key cannot already be cached"),
        };
        assert_eq!(
            cache.insert(second_generation, second_key.clone(), &[item(300)]),
            CacheInsertOutcome::Stored
        );

        assert!(matches!(
            cache.lookup(&first_key),
            ContextCacheLookup::Miss(_)
        ));
        assert!(matches!(
            cache.lookup(&second_key),
            ContextCacheLookup::Hit(_)
        ));
        assert_eq!(cache.len(), 1);
        assert!(cache.accounted_bytes() <= 1_300);
    }

    #[test]
    fn oversized_replacement_removes_old_entry_and_bypasses_candidate() {
        let mut cache = ContextCacheState::new(
            NonZeroUsize::new(10).expect("nonzero entry cap"),
            NonZeroUsize::new(1_300).expect("nonzero byte cap"),
        );
        let cache_key = key("replace");
        let generation = match cache.lookup(&cache_key) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("new cache cannot contain a hit"),
        };
        assert_eq!(
            cache.insert(generation, cache_key.clone(), &[item(16)]),
            CacheInsertOutcome::Stored
        );
        let generation = match cache.lookup(&cache_key) {
            ContextCacheLookup::Hit(_) => match cache.lookup(&key("other")) {
                ContextCacheLookup::Miss(generation) => generation,
                ContextCacheLookup::Hit(_) => panic!("unrelated key cannot be cached"),
            },
            ContextCacheLookup::Miss(_) => panic!("small item should have been cached"),
        };

        assert_eq!(
            cache.insert(generation, cache_key.clone(), &[item(2_000)]),
            CacheInsertOutcome::Oversized
        );
        assert!(matches!(
            cache.lookup(&cache_key),
            ContextCacheLookup::Miss(_)
        ));
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.accounted_bytes(), 0);
    }

    #[test]
    fn nested_provenance_and_reconciliation_capacities_are_accounted() {
        let cache_key = key("nested");
        let mut simple = ContextCacheState::new(
            NonZeroUsize::new(10).expect("nonzero entry cap"),
            NonZeroUsize::new(16_384).expect("nonzero byte cap"),
        );
        let generation = match simple.lookup(&cache_key) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("new cache cannot contain a hit"),
        };
        assert_eq!(
            simple.insert(generation, cache_key.clone(), &[item(8)]),
            CacheInsertOutcome::Stored
        );

        let mut enriched = item(8);
        enriched.provenance = serde_json::json!({
            "graph_trace": { "path": [{ "entity": "e".repeat(256) }] }
        });
        enriched.reconciliation = Some(crate::models::ClaimReconciliationMetadata {
            claim_ids: vec!["claim-".to_string() + &"c".repeat(128)],
            relations: vec![crate::models::ClaimRelationSummary {
                relation_id: "relation-".to_string() + &"r".repeat(128),
                outcome: crate::models::claim::ClaimRelationOutcome::Supersession,
                counterpart_source_episode_id: Some("episode-".to_string() + &"e".repeat(128)),
                superseded_by_fact_id: Some("fact-".to_string() + &"f".repeat(128)),
                reason_code: "reason-".to_string() + &"x".repeat(128),
                evaluator_version: "version-".to_string() + &"v".repeat(128),
            }],
        });
        let mut detailed = ContextCacheState::new(
            NonZeroUsize::new(10).expect("nonzero entry cap"),
            NonZeroUsize::new(16_384).expect("nonzero byte cap"),
        );
        let generation = match detailed.lookup(&cache_key) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("new cache cannot contain a hit"),
        };
        assert_eq!(
            detailed.insert(generation, cache_key, &[enriched]),
            CacheInsertOutcome::Stored
        );

        assert!(detailed.accounted_bytes() > simple.accounted_bytes() + 1_000);
    }

    #[test]
    fn key_string_capacity_contributes_to_accounted_weight() {
        let small_key = key("q");
        let large_key = key(&"q".repeat(512));
        let small_item = item(8);
        let mut small = ContextCacheState::new(
            NonZeroUsize::new(10).expect("nonzero entry cap"),
            NonZeroUsize::new(16_384).expect("nonzero byte cap"),
        );
        let generation = match small.lookup(&small_key) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("new cache cannot contain a hit"),
        };
        assert_eq!(
            small.insert(generation, small_key, std::slice::from_ref(&small_item)),
            CacheInsertOutcome::Stored
        );
        let mut large = ContextCacheState::new(
            NonZeroUsize::new(10).expect("nonzero entry cap"),
            NonZeroUsize::new(16_384).expect("nonzero byte cap"),
        );
        let generation = match large.lookup(&large_key) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("new cache cannot contain a hit"),
        };
        assert_eq!(
            large.insert(generation, large_key, &[small_item]),
            CacheInsertOutcome::Stored
        );

        assert!(large.accounted_bytes() > small.accounted_bytes() + 500);
    }

    #[test]
    fn replacement_updates_accounted_weight_instead_of_accumulating_old_weight() {
        let mut cache = ContextCacheState::new(
            NonZeroUsize::new(10).expect("nonzero entry cap"),
            NonZeroUsize::new(5_000).expect("nonzero byte cap"),
        );
        let cache_key = key("replace");
        let generation = match cache.lookup(&cache_key) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("new cache cannot contain a hit"),
        };
        assert_eq!(
            cache.insert(generation, cache_key.clone(), &[item(16)]),
            CacheInsertOutcome::Stored
        );
        let original_weight = cache.accounted_bytes();
        let generation = match cache.lookup(&key("unrelated")) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("unrelated key cannot be cached"),
        };

        assert_eq!(
            cache.insert(generation, cache_key, &[item(500)]),
            CacheInsertOutcome::Stored
        );
        assert_eq!(cache.len(), 1);
        assert!(cache.accounted_bytes() > original_weight);
        assert!(cache.accounted_bytes() <= 5_000);
    }

    #[test]
    fn oversized_entry_is_not_retained() {
        let mut cache = ContextCacheState::new(
            NonZeroUsize::new(10).expect("nonzero entry cap"),
            NonZeroUsize::new(1_300).expect("nonzero byte cap"),
        );
        let cache_key = key("oversized");
        let generation = match cache.lookup(&cache_key) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("new cache cannot contain a hit"),
        };

        assert_eq!(
            cache.insert(generation, cache_key.clone(), &[item(2_000)]),
            CacheInsertOutcome::Oversized
        );
        assert!(matches!(
            cache.lookup(&cache_key),
            ContextCacheLookup::Miss(_)
        ));
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.accounted_bytes(), 0);
    }

    #[test]
    fn clear_releases_accounted_bytes_and_fences_prior_misses() {
        let mut cache = ContextCacheState::new(
            NonZeroUsize::new(10).expect("nonzero entry cap"),
            NonZeroUsize::new(5_000).expect("nonzero byte cap"),
        );
        let cache_key = key("stale");
        let old_generation = match cache.lookup(&cache_key) {
            ContextCacheLookup::Miss(generation) => generation,
            ContextCacheLookup::Hit(_) => panic!("new cache cannot contain a hit"),
        };
        assert_eq!(
            cache.insert(old_generation, cache_key.clone(), &[item(32)]),
            CacheInsertOutcome::Stored
        );
        assert!(cache.accounted_bytes() > 0);

        cache.clear();

        assert_eq!(cache.len(), 0);
        assert_eq!(cache.accounted_bytes(), 0);
        assert_eq!(
            cache.insert(old_generation, cache_key, &[item(32)]),
            CacheInsertOutcome::StaleGeneration
        );
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.accounted_bytes(), 0);
    }
}
