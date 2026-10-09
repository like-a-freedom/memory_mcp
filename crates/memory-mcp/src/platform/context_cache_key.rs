//! The key the context and embedding caches are indexed by.
//!
//! A hashable description of one retrieval request's shape. Both the
//! assembled-context cache and the query-embedding cache key on it, so it
//! names no context and sits in the platform layer.

use chrono::{DateTime, Utc};

use crate::shared::search::normalize_text;
use crate::shared::temporal::{bucket_to_five_minutes, normalize_dt};

/// Cache identity for a temporal cutoff. Current requests intentionally share
/// a short freshness bucket; an explicit historical request must identify the
/// exact instant whose view it asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CacheCutoff {
    CurrentBucket(DateTime<Utc>),
    HistoricalExact(DateTime<Utc>),
}

impl CacheCutoff {
    fn identity(self) -> String {
        match self {
            Self::CurrentBucket(cutoff) => {
                format!("current:{}", bucket_to_five_minutes(cutoff))
            }
            Self::HistoricalExact(cutoff) => format!("historical:{}", normalize_dt(cutoff)),
        }
    }

    fn window_identity(self, bound: DateTime<Utc>) -> String {
        match self {
            Self::CurrentBucket(_) => bucket_to_five_minutes(bound),
            Self::HistoricalExact(_) => normalize_dt(bound),
        }
    }
}

impl From<DateTime<Utc>> for CacheCutoff {
    fn from(cutoff: DateTime<Utc>) -> Self {
        Self::CurrentBucket(cutoff)
    }
}

/// Cache key for context assembly results.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    pub(crate) query: String,
    pub(crate) cutoff: String,
    pub(crate) budget: i32,
    pub(crate) fact_types: Vec<String>,
    pub(crate) view: CacheView,
    pub(crate) tags: Option<Vec<String>>,
}

/// Timeline-specific cache parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct CacheView {
    pub(crate) view_mode: Option<String>,
    pub(crate) window_start: Option<String>,
    pub(crate) window_end: Option<String>,
}

impl CacheView {
    #[must_use]
    pub fn new(
        view_mode: Option<&str>,
        window_start: Option<DateTime<Utc>>,
        window_end: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            view_mode: view_mode.map(ToString::to_string),
            window_start: window_start.map(bucket_to_five_minutes),
            window_end: window_end.map(bucket_to_five_minutes),
        }
    }

    /// Build a view key with the same temporal precision as the request
    /// cutoff. Historical windows must not collide after time bucketing.
    #[must_use]
    pub fn new_with_cutoff(
        view_mode: Option<&str>,
        window_start: Option<DateTime<Utc>>,
        window_end: Option<DateTime<Utc>>,
        cutoff: CacheCutoff,
    ) -> Self {
        Self {
            view_mode: view_mode.map(ToString::to_string),
            window_start: window_start.map(|bound| cutoff.window_identity(bound)),
            window_end: window_end.map(|bound| cutoff.window_identity(bound)),
        }
    }
}

impl CacheKey {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        query: &str,
        cutoff: impl Into<CacheCutoff>,
        budget: i32,
        fact_types: &[String],
        view: CacheView,
        tags: Option<Vec<String>>,
    ) -> Self {
        let cutoff = cutoff.into();
        let mut tags = tags;
        if let Some(ref mut tag_list) = tags {
            tag_list.sort();
        }
        let mut fact_types = fact_types.to_vec();
        fact_types.sort();
        fact_types.dedup();
        Self {
            query: normalize_text(query),
            cutoff: cutoff.identity(),
            budget,
            fact_types,
            view,
            tags,
        }
    }
}

/// Extension over the assembled-context cache handle, so a holder of the
/// cache can clear it without depending on the context that fills it. The
/// embedding service writes a vector and then clears the cache, and that
/// is not a dependency from embedding onto memory.
pub trait InvalidateContextCache: Send + Sync {
    /// Invalidate all cached context results for the process-bound
    /// namespace.
    ///
    /// A boxed future rather than `async fn`: a public trait cannot
    /// write auto-trait bounds on an `async fn`, and the clippy lint
    /// that enforces this would otherwise make the trait unnameable
    /// outside the crate.
    fn invalidate_context_cache(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>;
}
