//! Application composition and integration adapters.

#[cfg(feature = "control-plane")]
use std::sync::Arc;

#[cfg(feature = "control-plane")]
pub mod integration;
pub mod stdio;

/// Run one crash-safe account-deletion recovery pass.
///
/// Exposed so tests (and any future second scheduler) compose the
/// recovery loop exactly the way production does, without
/// reaching for the crate-private adapter type.
#[cfg(feature = "control-plane")]
pub async fn run_deletion_recovery_pass(
    registry: crate::http::registry::RegistryHandle,
    fault_injector: Arc<dyn crate::platform::fault_injection::FaultInjector>,
) -> Result<(), crate::MemoryError> {
    let adapter = integration::registry_operations::RegistryDeletionRecoveryAdapter::new(
        registry,
        fault_injector,
    );
    let owner_id = crate::http::leases::scheduler::replica_id();
    crate::operations::api::run_deletion_recovery(
        &adapter,
        &integration::registry_operations::TenantRetainedWork,
        &owner_id,
        chrono::Utc::now(),
    )
    .await
}

#[cfg(feature = "control-plane")]
pub fn provisioning_scheduler_hooks(
    migrations: Arc<dyn crate::http::leases::migration::ApplyMigrations>,
    fault_injector: Arc<dyn crate::platform::fault_injection::FaultInjector>,
) -> Result<crate::http::leases::scheduler::SchedulerHooks, crate::MemoryError> {
    let hooks = crate::http::leases::scheduler::SchedulerHooks::with_provisioning_only(
        migrations,
        Arc::clone(&fault_injector),
    )?;
    let deletion_job: crate::http::leases::scheduler::SchedulerJob = Arc::new(move |registry| {
        let fault_injector = Arc::clone(&fault_injector);
        Box::pin(async move { run_deletion_recovery_pass(registry, fault_injector).await })
    });
    Ok(hooks.with_additional_job(deletion_job))
}
