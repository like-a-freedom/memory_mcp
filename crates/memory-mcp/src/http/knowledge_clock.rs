//! Freshness-clock restore at HTTP startup.
//!
//! The knowledge clock is memory-only, so a restart wipes it while the store
//! still knows when knowledge last landed. This walk runs once at boot,
//! before serving, and puts the newest stored ingest time back on the gauge
//! — so the age panels and the staleness alert read the true age instead of
//! silence until the next capture.
//!
//! One shot, never fatal: a tenant whose store cannot be read is skipped
//! with a warning, because a metric must not stop a boot. An empty store
//! restores nothing, which reads as absent — never as knowledge from the
//! epoch.

use std::sync::Arc;

use crate::http::logging::log_warn;
use crate::http::registry::RegistryHandle;
use crate::memory::episode_store::EpisodeStoreClient;

/// Restore the freshness clock from every durable tenant binding.
///
/// Runs once at boot, after the recorder is installed and the registry is
/// assembled. Lists every durable binding — ready or not, because knowledge
/// outlives tenant status — and stamps the newest ingest moment found. A
/// tenant that cannot be bound or read is skipped with a warning; the walk
/// itself never fails, so a metric can neither stop nor delay a boot.
pub(crate) async fn restore_knowledge_clock(registry: &RegistryHandle) {
    restore_knowledge_clock_with(registry, crate::observability::restore_knowledge_clock).await;
}

// Keep the durable scan independent from the process-global metrics recorder.
// This lets storage tests assert the selected timestamp without racing
// unrelated metric writers.
async fn restore_knowledge_clock_with(
    registry: &RegistryHandle,
    restore: impl FnOnce(Option<f64>),
) {
    let tenants = match registry.tenants().list_tenants(10_000).await {
        Ok(tenants) => tenants,
        Err(error) => {
            log_warn(
                "http.lifecycle.clock_restore_failed",
                &format!("tenant listing failed: {error}"),
            );
            return;
        }
    };
    // The conformance suite's in-memory registry has no privileged engine;
    // nothing can be bound, so there is nothing to restore — quietly, like
    // the scheduler walks.
    if registry.tenant_engine_optional().is_none() {
        return;
    }

    let mut newest: Option<f64> = None;
    for tenant in &tenants {
        let namespace = tenant.namespace_binding.namespace.clone();
        let lease = match registry.acquire_maintenance_client(tenant).await {
            Ok(Some(lease)) => lease,
            Ok(None) => return,
            Err(error) => {
                log_warn(
                    "http.lifecycle.clock_restore_failed",
                    &format!("tenant {}: bind failed: {error}", tenant.id),
                );
                continue;
            }
        };
        let db = lease.client();
        let store = EpisodeStoreClient::new(
            Arc::clone(&db) as Arc<dyn crate::storage::DbClient>,
            namespace,
        );
        match store.newest_t_ingested().await {
            Ok(candidate) => {
                let candidate = candidate.map(|moment| moment.timestamp() as f64);
                newest = match (newest, candidate) {
                    (Some(current), Some(candidate)) => Some(current.max(candidate)),
                    (Some(current), None) => Some(current),
                    (None, candidate) => candidate,
                };
            }
            Err(error) => {
                log_warn(
                    "http.lifecycle.clock_restore_failed",
                    &format!("tenant {}: newest ingest read failed: {error}", tenant.id),
                );
            }
        }
    }
    restore(newest);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::http::registry::RegistryHandle;
    use crate::http::registry::models::{NamespaceBinding, Tenant, TenantStatus};
    use crate::memory::episode_store::EpisodeStoreClient;
    use crate::storage::client::{DbClient, SurrealDbClient};

    /// Provision a ready tenant with migrations applied, mirroring the
    /// lifecycle scheduler tests: a real mem registry the walk can list.
    async fn provision(namespace: &str) -> (RegistryHandle, Arc<SurrealDbClient>) {
        let registry = RegistryHandle::in_memory_with_default_mem_engine().await;
        let db = registry
            .tenant_engine_optional()
            .expect("engine wired")
            .bind_to_test_namespace(namespace)
            .await;
        db.apply_migrations(namespace).await.expect("migrations");
        registry
            .tenants()
            .write_tenant(&Tenant {
                id: format!("ten_clock_{namespace}"),
                status: TenantStatus::Ready,
                namespace_binding: NamespaceBinding {
                    namespace: namespace.to_string(),
                    database: "memory".into(),
                },
                plan_version: 1,
                schema_version: 0,
                retry_stage: None,
                provisioning_lease: None,
                created_at: chrono::Utc::now(),
                version: 0,
            })
            .await
            .expect("register ready tenant");
        (registry, db)
    }

    async fn seed_episode(db: &Arc<SurrealDbClient>, namespace: &str, t_ingested: &str) {
        let store = EpisodeStoreClient::new(
            Arc::clone(db) as Arc<dyn crate::storage::DbClient>,
            namespace,
        );
        store
            .create(
                "episode:clock_seed",
                serde_json::json!({
                    "episode_id": "episode:clock_seed",
                    "source_type": "inline",
                    "source_id": "clock_seed",
                    "content": "seeded knowledge",
                    "policy_tags": [],
                    "t_ref": "2026-01-01T00:00:00Z",
                    "t_ingested": t_ingested,
                }),
            )
            .await
            .expect("seed episode should succeed");
    }

    /// A stored episode restores the clock to its ingest moment.
    ///
    /// This checks the durable scan's selected timestamp. The recorder's
    /// effect is covered separately by the observability unit tests.
    #[tokio::test]
    async fn boot_restores_the_clock_from_stored_episodes() {
        const STORED: &str = "2026-03-10T10:00:00Z";
        let (registry, db) = provision("tns_clock_seed").await;
        seed_episode(&db, "tns_clock_seed", STORED).await;
        let mut restored = None;

        super::restore_knowledge_clock_with(&registry, |moment| restored = moment).await;

        let expected = chrono::DateTime::parse_from_rfc3339(STORED)
            .expect("seed literal is RFC 3339")
            .timestamp() as f64;
        assert_eq!(
            restored,
            Some(expected),
            "boot must restore the newest stored ingest moment"
        );
    }

    /// An empty store produces no timestamp for the recorder to restore.
    #[tokio::test]
    async fn boot_without_episodes_restores_no_timestamp() {
        let (registry, _db) = provision("tns_clock_empty").await;
        let mut restored = Some(1.0);

        super::restore_knowledge_clock_with(&registry, |moment| restored = moment).await;

        assert_eq!(
            restored, None,
            "an empty store must not synthesize a knowledge timestamp"
        );
    }
}
