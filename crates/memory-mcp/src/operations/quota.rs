//! Quota admission and usage reconciliation.
//!
//! The rule that decides whether a tenant may ingest, and the rule that
//! repairs a counter that has drifted from the source tables, both live here
//! because they are decisions about a tenant's usage — not about HTTP.
//! ADR-0066 records why, and what it costs: the durable store still expresses
//! the same admission rule in SQL, and `tests/http_registry_storage.rs` pins
//! the two together.
//!
//! Everything in this module is a pure function over state the caller
//! supplies. Nothing here reads a database, and nothing here writes one — the
//! store adapters hand over a counter and get back a decision, which is what
//! lets `InMemoryStore` apply the increment inside its lock (where the
//! increment *is* the write) and `SurrealRegistryStore` apply it on a
//! discarded copy (where the SQL `WHERE` was the real gate).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::models::registry::{
    DEFAULT_EXTRACTION_CONCURRENCY, DEFAULT_INGEST_PER_MINUTE, DEFAULT_MAX_ACTIVE_API_KEYS,
    DEFAULT_MAX_EPISODE_COUNT, DEFAULT_MAX_INGESTED_BYTES, DEFAULT_MAX_OPEN_APP_SESSIONS,
    DEFAULT_PER_TENANT_REQUEST_CONCURRENCY,
};

/// Per-tenant plan limits, as the quota reads them.
///
/// Named `QuotaPlan` rather than `Plan` because `models::registry::Plan` is
/// the durable contract this is built from, and two types called `Plan` in one
/// call site forces a `From` import to say which is meant. That import is the
/// kind of noise that makes a reader pick the wrong one.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuotaPlan {
    pub plan_id: String,
    /// Max episodes that can be ingested per minute. 0 means
    /// "ingest is disabled".
    pub ingest_per_minute: u32,
    /// Cumulative source-byte ceiling.
    pub max_ingested_bytes: u64,
    /// Cumulative episode ceiling.
    pub max_episode_count: u64,
    /// Max concurrent extractions for a single tenant.
    pub extraction_concurrency: u32,
    /// Max open app sessions per tenant.
    pub max_open_app_sessions: u32,
    /// Max active API keys per account.
    pub max_active_api_keys: u32,
    /// Max in-flight requests for a single tenant. The runtime
    /// pool enforces this independently of the global
    /// admission gate.
    pub per_tenant_request_concurrency: u32,
    /// Reconciler drift threshold. The reconciler runs a
    /// count per source table; if `|count - usage_counter|`
    /// exceeds it, the counter is rewritten to the source
    /// count. One constant for both the episode and the byte
    /// comparison — `tests/quota_policy.rs` pins that.
    pub reconciler_drift_threshold: u32,
}

impl From<&crate::models::registry::Plan> for QuotaPlan {
    fn from(value: &crate::models::registry::Plan) -> Self {
        Self {
            plan_id: format!("{}:{}", value.id, value.version),
            ingest_per_minute: value.limits.ingest_per_minute,
            max_ingested_bytes: value.limits.max_ingested_bytes,
            max_episode_count: value.limits.max_episode_count,
            extraction_concurrency: value.limits.extraction_concurrency,
            max_open_app_sessions: value.limits.max_open_app_sessions,
            max_active_api_keys: value.limits.max_active_api_keys,
            per_tenant_request_concurrency: value.limits.per_tenant_request_concurrency,
            ..Self::default()
        }
    }
}

impl Default for QuotaPlan {
    fn default() -> Self {
        // Free tier default. The control plane can promote a tenant to a
        // higher plan by rewriting `Tenant.plan_version` and the registry's
        // plan table.
        Self {
            plan_id: "free".into(),
            ingest_per_minute: DEFAULT_INGEST_PER_MINUTE,
            max_ingested_bytes: DEFAULT_MAX_INGESTED_BYTES,
            max_episode_count: DEFAULT_MAX_EPISODE_COUNT,
            extraction_concurrency: DEFAULT_EXTRACTION_CONCURRENCY,
            max_open_app_sessions: DEFAULT_MAX_OPEN_APP_SESSIONS,
            max_active_api_keys: DEFAULT_MAX_ACTIVE_API_KEYS,
            per_tenant_request_concurrency: DEFAULT_PER_TENANT_REQUEST_CONCURRENCY,
            reconciler_drift_threshold: 5,
        }
    }
}

/// Result of an admission check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaDecision {
    Allow,
    /// The limit was exceeded. The HTTP layer maps this to a
    /// stable 429 with a `Retry-After` header and a `guidance`
    /// string the client surfaces to the human.
    Deny {
        reason: String,
        retry_after_secs: u32,
        guidance: String,
    },
}

impl QuotaDecision {
    #[must_use]
    pub fn is_deny(&self) -> bool {
        matches!(self, QuotaDecision::Deny { .. })
    }
}

/// The view of `usage_counter` an admission check sees.
///
/// The counter is durable (a Surreal table) in production and an in-memory map
/// in the test backend. Both read the same shape, which is what lets the
/// policy be one function for both.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageCounter {
    /// Ingest events in the current minute window.
    pub ingest_current_minute: u32,
    /// Wall-clock for the start of the current minute window.
    pub window_start: DateTime<Utc>,
    pub ingested_bytes: u64,
    pub episode_count: u64,
}

impl UsageCounter {
    /// Roll the window forward if 60 seconds or more have elapsed.
    ///
    /// `now` is a parameter rather than a clock read, so a test can place the
    /// boundary exactly and so this stays a pure function of its inputs.
    pub fn roll_if_expired(&mut self, now: DateTime<Utc>) {
        if (now - self.window_start).num_seconds() >= 60 {
            self.window_start = now;
            self.ingest_current_minute = 0;
        }
    }
}

/// Decide whether one ingest is admitted, and charge it if it is.
///
/// **A denial leaves the counter untouched.** The three increments are the last
/// statements before `Allow`, and that is a property callers depend on:
/// `InMemoryStore` applies this inside its lock, where the increment *is* the
/// write, so a denial that mutated the counter would spend a tenant's quota on
/// a request it then refused.
///
/// The order of the checks is load-bearing and is not alphabetical. `ingest
/// disabled` is answered before the window rolls, because a plan that forbids
/// ingest must say so regardless of how long ago the window started. The byte
/// ceiling is checked before the episode ceiling because bytes are the
/// unbounded one. The rate limit is last, because it is the only check whose
/// answer depends on `now` and it is the only one with a useful
/// `retry_after_secs`.
pub fn enforce_ingest(
    plan: &QuotaPlan,
    counter: &mut UsageCounter,
    source_bytes: u64,
    now: DateTime<Utc>,
) -> QuotaDecision {
    if plan.ingest_per_minute == 0 {
        return QuotaDecision::Deny {
            reason: "ingest_disabled".into(),
            retry_after_secs: 0,
            guidance: "this plan does not allow ingest".into(),
        };
    }
    counter.roll_if_expired(now);
    if counter.ingested_bytes.saturating_add(source_bytes) > plan.max_ingested_bytes {
        return QuotaDecision::Deny {
            reason: "ingested_bytes_exceeded".into(),
            retry_after_secs: 0,
            guidance: "the tenant has reached its cumulative ingest-byte limit".into(),
        };
    }
    if counter.episode_count >= plan.max_episode_count {
        return QuotaDecision::Deny {
            reason: "episode_count_exceeded".into(),
            retry_after_secs: 0,
            guidance: "the tenant has reached its cumulative episode limit".into(),
        };
    }
    if counter.ingest_current_minute >= plan.ingest_per_minute {
        // The window expires 60s after the start of the current minute. A more
        // accurate retry-after would project forward; this is the coarse
        // "wait out the rest of this window" value.
        let elapsed = (now - counter.window_start).num_seconds();
        let retry = (60 - elapsed).max(1) as u32;
        return QuotaDecision::Deny {
            reason: "ingest_rate_exceeded".into(),
            retry_after_secs: retry,
            guidance: format!(
                "ingest limited to {} per minute; wait {retry}s",
                plan.ingest_per_minute
            ),
        };
    }
    counter.ingest_current_minute += 1;
    counter.ingested_bytes = counter.ingested_bytes.saturating_add(source_bytes);
    counter.episode_count = counter.episode_count.saturating_add(1);
    QuotaDecision::Allow
}

/// What the reconciler found between the durable counter and the source table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsageDriftReport {
    pub tenant_id: String,
    pub source_count: u32,
    pub counter_count: u32,
    pub drift: i64,
    /// True when the drift exceeds the plan's threshold and the
    /// counter should be rewritten to the source count.
    pub repaired: bool,
}

/// Report drift between the durable `usage_counter` and the source table.
///
/// Named `usage_drift_report` rather than `reconcile_usage` because the
/// `UsageStore` trait method already owns the bare name. Two things in one
/// module called `reconcile_usage` is a trap: this one reads two counts and
/// reports, that one writes a repaired counter, and a reader who imports both
/// would assume they are the same operation.
///
/// The durable counter remains the authoritative admission gate; the
/// reconciler is repair-only.
pub fn usage_drift_report(
    plan: &QuotaPlan,
    tenant_id: &str,
    source_count: u32,
    counter_count: u32,
) -> UsageDriftReport {
    let drift = source_count as i64 - counter_count as i64;
    let threshold = plan.reconciler_drift_threshold as i64;
    UsageDriftReport {
        tenant_id: tenant_id.to_string(),
        source_count,
        counter_count,
        drift,
        repaired: drift.abs() > threshold,
    }
}
