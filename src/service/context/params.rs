use crate::models::AccessContext;

use super::query_mode::QueryFlags;

/// Parameters for the default context assembly pipeline.
pub(super) struct DefaultContextParams<'a> {
    pub(super) namespace: &'a str,
    pub(super) scope: &'a str,
    pub(super) cutoff_iso: &'a str,
    pub(super) cutoff: chrono::DateTime<chrono::Utc>,
    pub(super) raw_query_opt: Option<&'a str>,
    pub(super) query_opt: Option<&'a str>,
    pub(super) query_terms: &'a [String],
    pub(super) project_opt: Option<&'a str>,
    pub(super) fact_types: &'a [String],
    pub(super) budget: i32,
    pub(super) window_start: Option<chrono::DateTime<chrono::Utc>>,
    pub(super) window_end: Option<chrono::DateTime<chrono::Utc>>,
    pub(super) resolved_view_mode: Option<&'a str>,
    pub(super) query_flags: &'a QueryFlags,
    pub(super) access: &'a AccessContext,
}
