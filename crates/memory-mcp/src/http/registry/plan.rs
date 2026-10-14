//! Process-level usage reconciliation for the SaaS profile.
//!
//! The quota *policy* — whether a tenant may ingest, and how much drift
//! justifies rewriting a counter — lives in [`crate::operations::quota`]. What
//! stays here is the wiring: the scheduler job, and the pass that walks
//! tenants, reads their usage, and hands the two counts to the policy.
//! ADR-0066 records why the split is where it is.

#[cfg(feature = "streamable-http")]
use crate::http::leases::scheduler::{MaintenanceJob, MaintenanceSchedule, SchedulerJob};
#[cfg(feature = "streamable-http")]
use crate::storage::client::DbClient;
#[cfg(feature = "streamable-http")]
use std::sync::Arc;

use crate::operations::quota::{QuotaPlan, UsageCounter, usage_drift_report};

/// Tracked process-level usage reconciliation job. The durable counter remains
/// the admission authority; this pass repairs only the cumulative episode
/// count/bytes derived from canonical tenant records.
#[cfg(feature = "streamable-http")]
pub fn scheduler_job() -> SchedulerJob {
    SchedulerJob::new(MaintenanceJob::UsageReconcile, |registry, schedule| {
        Box::pin(reconcile_all(registry, schedule))
    })
}

#[cfg(feature = "streamable-http")]
async fn reconcile_all(
    registry: crate::http::registry::RegistryHandle,
    schedule: Arc<MaintenanceSchedule>,
) -> Result<(), crate::error::MemoryError> {
    let usage = registry.usage();
    let store = registry.tenants();
    let tenants = schedule
        .next_ready_tenants(MaintenanceJob::UsageReconcile, store.as_ref(), 100)
        .await?;
    if registry.tenant_engine_optional().is_none() {
        return Ok(());
    }
    for tenant in tenants {
        let Some(lease) = registry.acquire_maintenance_client(&tenant).await? else {
            return Ok(());
        };
        let db = lease.client();
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
        let current = usage.load_usage(&tenant.id).await?;
        let registry_plan = usage.load_plan(tenant.plan_version).await?;
        let plan = QuotaPlan::from(&registry_plan);
        let report = usage_drift_report(
            &plan,
            &tenant.id,
            u32::try_from(source_count).unwrap_or(u32::MAX),
            u32::try_from(current.episode_count).unwrap_or(u32::MAX),
        );
        let bytes_drift = ingested_bytes.abs_diff(current.ingested_bytes);
        if report.repaired || bytes_drift > u64::from(plan.reconciler_drift_threshold) {
            usage
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

// The policy's own tests moved with it, to `tests/quota_policy.rs`.
// What is left here is wiring, and wiring is covered by the scheduler
// job's tests in `tests/http_durable_tasks.rs` and the registry suite.
