//! Plan and quota enforcement.
//!
//! The `Plan` struct captures the per-tenant limits. The
//! enforcement points live at the tool handlers
//! (ingest/extract/open_app/api_key_create) and at the
//! runtime pool (per_tenant_request_concurrency). The
//! `enforce_ingest` function is the most-trafficked path;
//! it returns a `QuotaDecision` that the HTTP layer maps
//! to a stable 429 plus retry/guidance metadata.
//!
//! The reconciler walks the durable `usage_counter` table
//! and re-derives counts from the source tables. Drift
//! above a threshold rewrites the counter; the counter
//! remains the authoritative admission gate so the
//! reconciler is repair-only, never authoritative.

#[cfg(feature = "streamable-http")]
use crate::storage::client::DbClient;
use serde::{Deserialize, Serialize};
#[cfg(feature = "streamable-http")]
use std::sync::{Arc, Mutex, OnceLock};

/// Per-tenant plan limits. The plan id lives on the
/// `Tenant.plan_version`; the spec treats the plan as
/// opaque except for the limits below.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Plan {
    pub plan_id: String,
    /// Max episodes that can be ingested per minute. 0 means
    /// "ingest is disabled".
    pub ingest_per_minute: u32,
    /// Cumulative source-byte ceiling. The HTTP profile uses a finite free-tier
    /// default; commercial values come from the registry plan.
    pub max_ingested_bytes: u64,
    /// Cumulative episode ceiling.
    pub max_episode_count: u64,
    /// Max concurrent extractions for a single tenant. The
    /// extract tool checks this at handler entry.
    pub extraction_concurrency: u32,
    /// Max open app sessions per tenant. The App Session
    /// handler enforces this; the field is tracked on the
    /// `Plan` so the limit can be queried without an
    /// extra round trip.
    pub max_open_app_sessions: u32,
    /// Max active API keys per account. The control plane
    /// rejects new keys above the cap.
    pub max_active_api_keys: u32,
    /// Max in-flight requests for a single tenant. The
    /// runtime pool enforces this independently of
    /// `admission_gate` (which is global).
    pub per_tenant_request_concurrency: u32,
    /// Reconciler drift threshold. The reconciler runs
    /// `select count(*) ...` per source table; if
    /// `|count - usage_counter| > drift_threshold` the
    /// counter is rewritten to the source count.
    pub reconciler_drift_threshold: u32,
}

impl From<&crate::http::registry::models::Plan> for Plan {
    fn from(value: &crate::http::registry::models::Plan) -> Self {
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

impl Default for Plan {
    fn default() -> Self {
        // Free tier default. The control plane can promote
        // a tenant to a higher plan by rewriting the
        // `Tenant.plan_version` and the registry's plan
        // table.
        Self {
            plan_id: "free".into(),
            ingest_per_minute: crate::http::registry::models::DEFAULT_INGEST_PER_MINUTE,
            max_ingested_bytes: crate::http::registry::models::DEFAULT_MAX_INGESTED_BYTES,
            max_episode_count: crate::http::registry::models::DEFAULT_MAX_EPISODE_COUNT,
            extraction_concurrency: crate::http::registry::models::DEFAULT_EXTRACTION_CONCURRENCY,
            max_open_app_sessions: crate::http::registry::models::DEFAULT_MAX_OPEN_APP_SESSIONS,
            max_active_api_keys: crate::http::registry::models::DEFAULT_MAX_ACTIVE_API_KEYS,
            per_tenant_request_concurrency:
                crate::http::registry::models::DEFAULT_PER_TENANT_REQUEST_CONCURRENCY,
            reconciler_drift_threshold: 5,
        }
    }
}

/// Result of an admission check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaDecision {
    Allow,
    /// The limit was exceeded. The HTTP layer maps this
    /// to a stable 429 with a `Retry-After` header
    /// (carried in `retry_after_secs`) and a `guidance`
    /// string the client surfaces to the human.
    Deny {
        reason: String,
        retry_after_secs: u32,
        guidance: String,
    },
}

impl QuotaDecision {
    pub fn is_deny(&self) -> bool {
        matches!(self, QuotaDecision::Deny { .. })
    }
}

/// The view of `usage_counter` an admission check sees.
/// The counter is durable (Surreal table) and the in-memory
/// test backend exposes the same shape via `InMemoryStore`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageCounter {
    /// Ingest events in the current minute window.
    pub ingest_current_minute: u32,
    /// Wall-clock for the start of the current minute window.
    pub window_start: chrono::DateTime<chrono::Utc>,
    pub ingested_bytes: u64,
    pub episode_count: u64,
}

impl UsageCounter {
    /// Roll the window forward if more than 60 seconds
    /// have elapsed. The `now` parameter is the wall
    /// clock at the call site.
    pub fn roll_if_expired(&mut self, now: chrono::DateTime<chrono::Utc>) {
        if (now - self.window_start).num_seconds() >= 60 {
            self.window_start = now;
            self.ingest_current_minute = 0;
        }
    }
}

/// Enforce the ingest rate limit. Increments the counter
/// atomically (caller must hold the registry's write lock
/// or use `update_tenant_state_fenced` for the durable
/// path) and returns `Allow` or `Deny`. The test backend
/// holds the counter in `InMemoryStore`; the production
/// backend writes the counter to the `usage_counter`
/// table.
pub fn enforce_ingest(
    plan: &Plan,
    counter: &mut UsageCounter,
    source_bytes: u64,
    now: chrono::DateTime<chrono::Utc>,
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
        // The window expires 60s after the start of the
        // current minute. A more accurate retry-after
        // would project forward; we use the coarse
        // "wait the rest of this window" value.
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

/// Reconciler drift report. The reconciler rewrites the
/// counter only when `drift.abs() > plan.reconciler_drift_threshold`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconcilerReport {
    pub tenant_id: String,
    pub source_count: u32,
    pub counter_count: u32,
    pub drift: i64,
    pub repaired: bool,
}

/// Repair drift between the durable `usage_counter` and
/// the source table. The durable counter remains the
/// authoritative admission gate; the reconciler rewrites
/// it only when drift exceeds the plan's threshold.
pub fn reconcile_usage(
    plan: &Plan,
    tenant_id: &str,
    source_count: u32,
    counter_count: u32,
) -> ReconcilerReport {
    let drift = source_count as i64 - counter_count as i64;
    let threshold = plan.reconciler_drift_threshold as i64;
    let repaired = drift.abs() > threshold;
    ReconcilerReport {
        tenant_id: tenant_id.to_string(),
        source_count,
        counter_count,
        drift,
        repaired,
    }
}

/// Tracked process-level usage reconciliation job. The durable counter remains
/// the admission authority; this pass repairs only the cumulative episode
/// count/bytes derived from canonical tenant records.
#[cfg(feature = "streamable-http")]
pub fn scheduler_job() -> crate::http::leases::scheduler::SchedulerJob {
    Arc::new(|registry| Box::pin(reconcile_all(registry)))
}

#[cfg(feature = "streamable-http")]
async fn reconcile_all(
    registry: crate::http::registry::RegistryHandle,
) -> Result<(), crate::error::MemoryError> {
    static LAST_RUN: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();
    {
        let mut last = LAST_RUN
            .get_or_init(|| Mutex::new(None))
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.is_some_and(|instant| instant.elapsed() < std::time::Duration::from_secs(60)) {
            return Ok(());
        }
        *last = Some(std::time::Instant::now());
    }
    let store = registry.store_clone();
    let tenants = store.list_ready_tenants(None, 100).await?;
    let Some(engine) = registry.tenant_engine_optional() else {
        return Ok(());
    };
    for tenant in tenants {
        let db = engine.bind(&tenant).await?;
        let aggregate = db
            .query(
                "SELECT count() AS episode_count, math::sum(string::len(content)) AS ingested_bytes FROM episode GROUP ALL",
                None,
                &tenant.namespace_binding.namespace,
            )
            .await?;
        let rows: Vec<serde_json::Value> = serde_json::from_value(aggregate).map_err(|error| {
            crate::error::MemoryError::Storage(format!("usage aggregate decode failed: {error}"))
        })?;
        let row = rows.first();
        let source_count = row
            .and_then(|value| value.get("episode_count"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let ingested_bytes = row
            .and_then(|value| value.get("ingested_bytes"))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        let current = store.load_usage(&tenant.id).await?;
        let registry_plan = store.load_plan(tenant.plan_version).await?;
        let plan = Plan::from(&registry_plan);
        let report = reconcile_usage(
            &plan,
            &tenant.id,
            u32::try_from(source_count).unwrap_or(u32::MAX),
            u32::try_from(current.episode_count).unwrap_or(u32::MAX),
        );
        let bytes_drift = ingested_bytes.abs_diff(current.ingested_bytes);
        if report.repaired || bytes_drift > u64::from(plan.reconciler_drift_threshold) {
            store
                .reconcile_usage(
                    &tenant.id,
                    UsageCounter {
                        ingest_current_minute: current.ingest_current_minute,
                        window_start: current.window_start,
                        ingested_bytes,
                        episode_count: source_count,
                    },
                )
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(per_minute: u32) -> Plan {
        Plan {
            ingest_per_minute: per_minute,
            ..Plan::default()
        }
    }

    #[test]
    fn ingest_allows_under_limit() {
        let mut counter = UsageCounter::default();
        let now = chrono::Utc::now();
        counter.window_start = now;
        for _ in 0..3 {
            let d = enforce_ingest(&plan(5), &mut counter, 0, now);
            assert!(matches!(d, QuotaDecision::Allow));
        }
        assert_eq!(counter.ingest_current_minute, 3);
    }

    #[test]
    fn quota_exceeded_rejects_ingest_with_retry_guidance() {
        let mut counter = UsageCounter::default();
        let now = chrono::Utc::now();
        counter.window_start = now;
        for _ in 0..5 {
            assert!(matches!(
                enforce_ingest(&plan(5), &mut counter, 0, now),
                QuotaDecision::Allow
            ));
        }
        // 6th request must be denied with retry guidance.
        let d = enforce_ingest(&plan(5), &mut counter, 0, now);
        match d {
            QuotaDecision::Deny {
                reason,
                retry_after_secs,
                guidance,
            } => {
                assert_eq!(reason, "ingest_rate_exceeded");
                assert!(retry_after_secs > 0);
                assert!(guidance.contains("ingest limited to 5"));
            }
            QuotaDecision::Allow => panic!("expected deny"),
        }
    }

    #[test]
    fn zero_per_minute_disables_ingest() {
        let mut counter = UsageCounter::default();
        let now = chrono::Utc::now();
        counter.window_start = now;
        let d = enforce_ingest(&plan(0), &mut counter, 0, now);
        assert!(d.is_deny());
    }

    #[test]
    fn window_rolls_after_60s() {
        let mut counter = UsageCounter::default();
        let t0 = chrono::Utc::now();
        counter.window_start = t0;
        for _ in 0..2 {
            assert!(matches!(
                enforce_ingest(&plan(2), &mut counter, 0, t0),
                QuotaDecision::Allow
            ));
        }
        // 3rd request at t0 denied (cap=2).
        assert!(enforce_ingest(&plan(2), &mut counter, 0, t0).is_deny());
        // Roll forward 60s; counter resets.
        let t1 = t0 + chrono::Duration::seconds(61);
        let d = enforce_ingest(&plan(2), &mut counter, 0, t1);
        assert!(matches!(d, QuotaDecision::Allow));
        assert_eq!(counter.ingest_current_minute, 1);
    }

    #[test]
    fn reconciler_repairs_drift_above_threshold() {
        // drift = |10 - 0| = 10 > threshold (5)
        let report = reconcile_usage(&plan(5), "ten_1", 10, 0);
        assert!(report.repaired);
        assert_eq!(report.drift, 10);
    }

    #[test]
    fn registry_plan_converts_to_runtime_plan() {
        let registry_plan = crate::http::registry::models::Plan {
            id: "free".into(),
            version: 7,
            limits: crate::http::registry::models::PlanLimits {
                max_ingested_bytes: 100,
                max_episode_count: 10,
                ingest_per_minute: 3,
                max_open_app_sessions: 8,
                max_active_api_keys: 2,
                per_tenant_request_concurrency: 6,
                extraction_concurrency: 4,
            },
        };
        let runtime_plan = Plan::from(&registry_plan);
        assert_eq!(runtime_plan.plan_id, "free:7");
        assert_eq!(runtime_plan.max_ingested_bytes, 100);
        assert_eq!(runtime_plan.max_episode_count, 10);
        assert_eq!(runtime_plan.ingest_per_minute, 3);
        assert_eq!(runtime_plan.max_open_app_sessions, 8);
        assert_eq!(runtime_plan.max_active_api_keys, 2);
        assert_eq!(runtime_plan.per_tenant_request_concurrency, 6);
        assert_eq!(runtime_plan.extraction_concurrency, 4);
    }

    #[test]
    fn reconciler_skips_drift_below_threshold() {
        // drift = |7 - 5| = 2 <= threshold (5)
        let report = reconcile_usage(&plan(5), "ten_2", 7, 5);
        assert!(!report.repaired);
        assert_eq!(report.drift, 2);
    }
}
