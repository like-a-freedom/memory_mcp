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
    let Some(engine) = registry.tenant_engine_optional() else {
        return;
    };

    let mut newest: Option<f64> = None;
    for tenant in &tenants {
        let namespace = tenant.namespace_binding.namespace.clone();
        let db = match engine.bind(tenant).await {
            Ok(db) => db,
            Err(error) => {
                log_warn(
                    "http.lifecycle.clock_restore_failed",
                    &format!("tenant {}: bind failed: {error}", tenant.id),
                );
                continue;
            }
        };
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
    crate::observability::restore_knowledge_clock(newest);
}

#[cfg(all(test, feature = "prometheus"))]
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

    fn gauge_sample(exposition: &str) -> Option<f64> {
        exposition.lines().find_map(|line| {
            let (sample_name, value) = line.split_once(' ')?;
            (sample_name
                == crate::shared::observability::METRIC_KNOWLEDGE_LAST_WRITE_TIMESTAMP_SECONDS)
                .then(|| value.parse().ok())
                .flatten()
        })
    }

    /// A stored episode restores the clock to its ingest moment.
    ///
    /// The handle comes first, mirroring boot order: the recorder is
    /// installed before the walk runs, and a stamp set with no recorder
    /// installed would be lost unnoticed.
    #[tokio::test]
    async fn boot_restores_the_clock_from_stored_episodes() {
        const STORED: &str = "2026-03-10T10:00:00Z";
        let handle = crate::observability::shared_test_handle().expect("prometheus enabled");
        let (registry, db) = provision("tns_clock_seed").await;
        seed_episode(&db, "tns_clock_seed", STORED).await;

        super::restore_knowledge_clock(&registry).await;

        let exposition = handle.render();
        let expected = chrono::DateTime::parse_from_rfc3339(STORED)
            .expect("seed literal is RFC 3339")
            .timestamp() as f64;
        assert_eq!(
            gauge_sample(&exposition),
            Some(expected),
            "boot must put the stored ingest moment back on the clock: {exposition}"
        );
    }

    /// No episodes restores nothing: the clock stays exactly where it was.
    ///
    /// The shared recorder outlives any one test, so this asserts the walk
    /// is a no-op rather than absence, which a sibling test's stamp would
    /// break.
    #[tokio::test]
    async fn boot_without_episodes_leaves_the_clock_alone() {
        let handle = crate::observability::shared_test_handle().expect("prometheus enabled");
        let (registry, _db) = provision("tns_clock_empty").await;
        let before = handle.render();

        super::restore_knowledge_clock(&registry).await;

        assert_eq!(
            gauge_sample(&before),
            gauge_sample(&handle.render()),
            "a walk over an empty store must not move the clock"
        );
    }
}
