//! Ephemeral exposure-trace types for selective recall.
//!
//! Traces are ephemeral by default: a per-session LRU holds at most 32 traces
//! for 30 minutes. A significant captured event may copy a bounded trace link
//! (see `LifecycleTraceLink` in `memory_event.rs`). Unlinked traces create no
//! durable rows.
//!
//! This module is expanded in Task 6 with the full `LifecycleRecall`
//! implementation. For now it provides the bounded trace store skeleton so
//! Task 2's policy types have a concrete home.

use std::collections::VecDeque;

use crate::models::LifecycleTraceLink;

/// Maximum number of in-memory traces per session.
pub const MAX_TRACES_PER_SESSION: usize = 32;

/// Trace time-to-live in seconds (30 minutes).
pub const TRACE_TTL_SECS: u64 = 30 * 60;

/// A bounded in-memory LRU of exposure traces for one session.
///
/// This is **not** persisted. Only a significant captured event copies a
/// bounded `LifecycleTraceLink` into the durable event record.
#[derive(Debug, Clone)]
pub struct ExposureTraceStore {
    traces: VecDeque<ExposureTrace>,
    max_size: usize,
}

/// A single ephemeral exposure trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExposureTrace {
    /// The recall key that produced this trace.
    pub recall_key: String,
    /// The retrieval fingerprint of the context pipeline result.
    pub retrieval_fingerprint: String,
    /// Selected fact IDs in rank order (max 32).
    pub selected_fact_ids: Vec<String>,
    /// Selected experience IDs in rank order (max 8).
    pub selected_experience_ids: Vec<String>,
    /// The policy fingerprint at recall time.
    pub policy_fingerprint: String,
    /// Unix-epoch seconds when the trace was created.
    pub created_at_secs: u64,
}

impl ExposureTrace {
    /// Convert this ephemeral trace into a bounded durable link.
    #[must_use]
    pub fn to_link(&self, created_at_rfc3339: impl Into<String>) -> LifecycleTraceLink {
        LifecycleTraceLink {
            retrieval_fingerprint: self.retrieval_fingerprint.clone(),
            selected_fact_ids: self.selected_fact_ids.iter().take(32).cloned().collect(),
            selected_experience_ids: self
                .selected_experience_ids
                .iter()
                .take(8)
                .cloned()
                .collect(),
            policy_fingerprint: self.policy_fingerprint.clone(),
            created_at: created_at_rfc3339.into(),
        }
    }
}

impl ExposureTraceStore {
    /// Create a new store with the default bounded capacity.
    #[must_use]
    pub fn new() -> Self {
        Self {
            traces: VecDeque::with_capacity(MAX_TRACES_PER_SESSION),
            max_size: MAX_TRACES_PER_SESSION,
        }
    }

    /// Push a trace, evicting the oldest if the capacity is exceeded.
    pub fn push(&mut self, trace: ExposureTrace) {
        if self.traces.len() >= self.max_size {
            self.traces.pop_front();
        }
        self.traces.push_back(trace);
    }

    /// Look up the most recent trace matching a recall key.
    #[must_use]
    pub fn get(&self, recall_key: &str) -> Option<&ExposureTrace> {
        self.traces
            .iter()
            .rev()
            .find(|trace| trace.recall_key == recall_key)
    }

    /// Returns the number of traces currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.traces.len()
    }

    /// Returns `true` if the store holds no traces.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.traces.is_empty()
    }

    /// Returns the timestamp of the oldest trace, or `None` if empty.
    ///
    /// Used by `SessionTraceRegistry` to find the session with the oldest
    /// trace when evicting sessions that exceed the registry cap.
    #[must_use]
    pub fn oldest_trace_secs(&self) -> Option<u64> {
        self.traces.front().map(|t| t.created_at_secs)
    }

    /// Evict traces older than `now_secs - ttl_secs`.
    pub fn evict_expired(&mut self, now_secs: u64, ttl_secs: u64) {
        let cutoff = now_secs.saturating_sub(ttl_secs);
        self.traces.retain(|trace| trace.created_at_secs >= cutoff);
    }
}

impl Default for ExposureTraceStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_trace(key: &str, ts: u64) -> ExposureTrace {
        ExposureTrace {
            recall_key: key.to_string(),
            retrieval_fingerprint: "fp".to_string(),
            selected_fact_ids: vec!["fact:1".to_string()],
            selected_experience_ids: vec![],
            policy_fingerprint: "policy".to_string(),
            created_at_secs: ts,
        }
    }

    #[test]
    fn store_is_bounded_to_max_capacity() {
        let mut store = ExposureTraceStore::new();
        for i in 0..(MAX_TRACES_PER_SESSION + 5) {
            store.push(make_trace(&format!("key-{i}"), 1000 + i as u64));
        }
        assert_eq!(store.len(), MAX_TRACES_PER_SESSION);
    }

    #[test]
    fn store_evicts_oldest_when_full() {
        let mut store = ExposureTraceStore::new();
        store.push(make_trace("oldest", 1000));
        for i in 0..MAX_TRACES_PER_SESSION {
            store.push(make_trace(&format!("key-{i}"), 2000 + i as u64));
        }
        // The oldest entry was evicted.
        assert!(store.get("oldest").is_none());
        // A recent entry is present.
        assert!(
            store
                .get(&format!("key-{}", MAX_TRACES_PER_SESSION - 1))
                .is_some()
        );
    }

    #[test]
    fn store_evicts_expired_traces() {
        let mut store = ExposureTraceStore::new();
        store.push(make_trace("old", 1000));
        store.push(make_trace("new", 5000));
        // TTL of 1000 secs, now is 5000 → old (1000) is expired (5000-1000=4000 cutoff).
        store.evict_expired(5000, 1000);
        assert!(store.get("old").is_none());
        assert!(store.get("new").is_some());
    }

    #[test]
    fn trace_to_link_caps_selected_ids() {
        let trace = ExposureTrace {
            recall_key: "k".to_string(),
            retrieval_fingerprint: "fp".to_string(),
            selected_fact_ids: (0..50).map(|i| format!("fact:{i}")).collect(),
            selected_experience_ids: (0..20).map(|i| format!("exp:{i}")).collect(),
            policy_fingerprint: "p".to_string(),
            created_at_secs: 1000,
        };
        let link = trace.to_link("2026-07-23T00:00:00Z");
        assert_eq!(link.selected_fact_ids.len(), 32);
        assert_eq!(link.selected_experience_ids.len(), 8);
    }

    #[test]
    fn store_default_is_empty() {
        let store = ExposureTraceStore::default();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }
}
