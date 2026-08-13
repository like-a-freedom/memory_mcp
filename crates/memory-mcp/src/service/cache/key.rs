use chrono::{DateTime, Utc};

use crate::service::{normalize_text, query::bucket_to_five_minutes};

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
}

impl CacheKey {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn new(
        query: &str,
        cutoff: DateTime<Utc>,
        budget: i32,
        fact_types: &[String],
        view: CacheView,
        tags: Option<Vec<String>>,
    ) -> Self {
        let mut tags = tags;
        if let Some(ref mut tag_list) = tags {
            tag_list.sort();
        }
        let mut fact_types = fact_types.to_vec();
        fact_types.sort();
        fact_types.dedup();
        Self {
            query: normalize_text(query),
            cutoff: bucket_to_five_minutes(cutoff),
            budget,
            fact_types,
            view,
            tags,
        }
    }
}
