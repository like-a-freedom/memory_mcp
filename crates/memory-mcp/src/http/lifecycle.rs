//! HTTP lifecycle maintenance: confidence decay, episode archival, and
//! community rebuild.
//!
//! The stdio profile spawns these as per-namespace background workers
//! (`spawn_workers_from_config`); the HTTP profile never called that, so
//! `LIFECYCLE_ENABLED` reached every tenant service and governed nothing —
//! the flag was read, copied, and had no effect. This job is the HTTP shape
//! of the same work: a process-level pass over the ready tenants, exactly
//! like the embedding backfill, because the runtime pool evicts idle
//! tenants and lifecycle maintenance must outlive any one runtime.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::MemoryError;
use crate::http::leases::scheduler::{SchedulerJob, cadence_due_at};
use crate::http::registry::RegistryHandle;
use crate::http::registry::models::Tenant;
use crate::http::runtime::bootstrap::DeploymentPolicy;
use crate::storage::client::SurrealDbClient;

/// How many ready tenants one walk visits — the same bound every other
/// maintenance job uses, so a pass never holds the maintenance semaphore for
/// as long as the backlog.
const TENANT_BATCH: usize = 100;

/// Which lifecycle work one walk performs over the ready tenants.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pass {
    Decay,
    ArchivalAndCommunity,
}

impl Pass {
    /// The job metric this pass records.
    const fn metric(self) -> &'static str {
        match self {
            Self::Decay => "lifecycle_decay",
            Self::ArchivalAndCommunity => "lifecycle_archival",
        }
    }

    /// The operation a per-tenant failure is logged under, so a breakage
    /// names which half of the work stopped.
    const fn failed_op(self) -> &'static str {
        match self {
            Self::Decay => "http.lifecycle.decay_failed",
            Self::ArchivalAndCommunity => "http.lifecycle.archival_failed",
        }
    }
}

/// The lifecycle job. Registers itself with
/// `SchedulerHooks::with_additional_job` only when the deployment enabled
/// lifecyle, so a lexical-only or opted-out deployment carries no job at all
/// rather than a job that always no-ops.
///
/// Two independent cadences ride one job: decay on
/// `LIFECYCLE_DECAY_INTERVAL_SECS`, archival (and the community rebuild that
/// stdio runs on the same interval) on `LIFECYCLE_ARCHIVAL_INTERVAL_SECS`.
/// Each gate lives in the closure, so every job instance owns its own window
/// and tests do not share one.
pub fn lifecycle_scheduler_job(policy: DeploymentPolicy) -> SchedulerJob {
    let decay_last = Arc::new(std::sync::Mutex::new(None));
    let archival_last = Arc::new(std::sync::Mutex::new(None));
    Arc::new(move |registry| {
        let policy = policy.clone();
        let decay_last = Arc::clone(&decay_last);
        let archival_last = Arc::clone(&archival_last);
        Box::pin(async move {
            // Belt-and-suspenders with the registration gate: a disabled
            // deployment must do nothing even if the job is somehow invoked.
            if !policy.lifecycle.enabled {
                return Ok(());
            }
            let now = Instant::now();
            let decay_due = cadence_due_at(
                &decay_last,
                Duration::from_secs(policy.lifecycle.decay_interval_secs),
                now,
            );
            let archival_due = cadence_due_at(
                &archival_last,
                Duration::from_secs(policy.lifecycle.archival_interval_secs),
                now,
            );
            if decay_due {
                run_walk(&registry, &policy, Pass::Decay).await?;
            }
            if archival_due {
                run_walk(&registry, &policy, Pass::ArchivalAndCommunity).await?;
            }
            Ok(())
        })
    })
}

/// One bounded pass over the ready tenants for a single lifecycle work.
///
/// Errors never abort the walk: a tenant whose store is broken must not stop
/// the rest, and the walk's outcome is `degraded` if any tenant failed — the
/// same contract the embedding backfill's walk follows.
async fn run_walk(
    registry: &RegistryHandle,
    policy: &DeploymentPolicy,
    pass: Pass,
) -> Result<(), MemoryError> {
    let tenants = registry
        .tenants()
        .list_ready_tenants(None, TENANT_BATCH)
        .await?;
    // The conformance suite's in-memory registry has no privileged engine;
    // nothing can be bound, so there is nothing to maintain — and saying so
    // quietly is better than reporting a failed pass.
    let Some(engine) = registry.tenant_engine_optional() else {
        return Ok(());
    };

    let started = Instant::now();
    let mut degraded = false;
    for tenant in tenants {
        let db = match engine.bind(&tenant).await {
            Ok(db) => db,
            Err(error) => {
                degraded = true;
                crate::http::logging::log_warn(
                    pass.failed_op(),
                    &format!("tenant {}: bind failed: {error}", tenant.id),
                );
                continue;
            }
        };
        if let Err(error) = run_tenant(&db, &tenant, policy, pass).await {
            degraded = true;
            crate::http::logging::log_warn(
                pass.failed_op(),
                &format!("tenant {}: {error}", tenant.id),
            );
        }
    }

    crate::observability::record_job_metric(
        pass.metric(),
        if degraded { "degraded" } else { "ok" },
        started.elapsed().as_secs_f64(),
    );
    Ok(())
}

/// Build the maintenance service for one tenant and run one pass against it.
///
/// The service is built the way the embedding backfill builds its own: the
/// deployment's log directives, the lifecycle policy and the claim config the
/// composition root resolved. `handles_from` reads the claim service, so a
/// lifecycle pass that ignored the rollout stage would reconcile claims under
/// the wrong one.
async fn run_tenant(
    db: &Arc<SurrealDbClient>,
    tenant: &Tenant,
    policy: &DeploymentPolicy,
    pass: Pass,
) -> Result<(), MemoryError> {
    let namespace = tenant.namespace_binding.namespace.clone();
    let mut service = crate::service::MemoryService::new(
        Arc::clone(db) as Arc<dyn crate::storage::client::DbClient>,
        namespace,
        crate::logging::StdoutLogger::directives_from_env(),
        100, // rate_limit_rps; a maintenance pass is not request traffic
        100, // rate_limit_burst
    )?;
    service.lifecycle_config = policy.lifecycle.clone();
    service.claim_service = service
        .claim_service
        .clone()
        .with_config(policy.claim_config.clone());

    match pass {
        Pass::Decay => {
            crate::platform::lifecycle_runtime::decay_pass(
                &service,
                policy.lifecycle.decay_confidence_threshold,
                policy.lifecycle.decay_half_life_days,
            )
            .await?;
        }
        Pass::ArchivalAndCommunity => {
            crate::platform::lifecycle_runtime::archival_pass(
                &service,
                policy.lifecycle.archival_age_days,
            )
            .await?;
            crate::platform::lifecycle_runtime::run_community_rebuild_pass(&service).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::RegistryHandle;
    use crate::http::registry::models::{NamespaceBinding, Tenant, TenantStatus};
    use crate::storage::client::{DbClient, SurrealDbClient};

    fn tenant(namespace: &str) -> Tenant {
        Tenant {
            id: "ten_lifecycle".to_string(),
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
        }
    }

    fn policy(lifecycle: crate::config::LifecycleConfig) -> DeploymentPolicy {
        DeploymentPolicy {
            embedding: None,
            entity_extractor: None,
            lifecycle,
            auto_recovery: false,
            query_logging_enabled: false,
            query_log_retention_days: crate::config::DEFAULT_QUERY_LOG_RETENTION_DAYS,
            embedding_similarity_threshold: crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            claim_config: crate::config::claims::ClaimConfig::default(),
        }
    }

    /// Enabled, with a decay that will actually fire on a 400-day-old
    /// 0.4-confidence fact at a 0.3 threshold and a 100-day half-life.
    fn decay_enabled() -> crate::config::LifecycleConfig {
        crate::config::LifecycleConfig {
            enabled: true,
            decay_interval_secs: 3600,
            archival_interval_secs: 86_400,
            decay_confidence_threshold: 0.3,
            archival_age_days: 90,
            decay_half_life_days: 100.0,
        }
    }

    /// A fact old enough and uncertain enough that decay must invalidate it.
    async fn seed_decayed_fact(db: &SurrealDbClient, namespace: &str, fact_id: &str) {
        let old =
            crate::shared::temporal::normalize_dt(chrono::Utc::now() - chrono::Duration::days(400));
        db.create(
            fact_id,
            serde_json::json!({
                "fact_id": fact_id,
                "fact_type": "note",
                "content": format!("content {fact_id}"),
                "quote": format!("content {fact_id}"),
                "source_episode": "episode:seed",
                "t_valid": old,
                "t_ingested": old,
                "confidence": 0.4,
                "index_keys": [],
                "access_count": 0,
                "entity_links": [],
                "scope": namespace,
                "policy_tags": [],
                "provenance": {"source_episode": "episode:seed"},
            }),
            namespace,
            crate::knowledge::queries::FACT_TEMPORAL_FIELDS,
        )
        .await
        .expect("seed a decayable fact");
    }

    /// Provision a ready tenant with migrations applied and return the bound
    /// handle so the test can seed and read facts in its namespace.
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
            .write_tenant(&tenant(namespace))
            .await
            .expect("register ready tenant");
        (registry, db)
    }

    async fn fact_is_invalid(db: &SurrealDbClient, namespace: &str, fact_id: &str) -> bool {
        let fact = db
            .select_one(fact_id, namespace)
            .await
            .expect("read fact")
            .expect("fact exists");
        fact.get("t_invalid").is_some()
    }

    /// The whole point of the job: an enabled deployment runs decay over the
    /// ready tenants, and the cadence gate makes the next tick a no-op.
    #[tokio::test]
    async fn the_lifecycle_job_decays_and_then_gates_its_cadence() {
        let namespace = "tns_lifecycle_decay";
        let (registry, db) = provision(namespace).await;
        seed_decayed_fact(&db, namespace, "fact:old1").await;

        let job = lifecycle_scheduler_job(policy(decay_enabled()));
        job(registry.clone()).await.expect("first tick");

        assert!(
            fact_is_invalid(&db, namespace, "fact:old1").await,
            "the first tick must run decay and invalidate the old fact"
        );

        // A second decayable fact appearing immediately must wait for the
        // cadence window, exactly as backfill's does.
        seed_decayed_fact(&db, namespace, "fact:old2").await;
        job(registry)
            .await
            .expect("second tick is a successful no-op");
        assert!(
            !fact_is_invalid(&db, namespace, "fact:old2").await,
            "the second tick inside the decay interval must not run decay"
        );
    }

    /// A deployment that did not opt in must have nothing happen, whether or
    /// not the job is somehow invoked: the flag is the whole switch.
    #[tokio::test]
    async fn a_disabled_lifecycle_job_decays_nothing() {
        let namespace = "tns_lifecycle_off";
        let (registry, db) = provision(namespace).await;
        seed_decayed_fact(&db, namespace, "fact:old").await;

        let mut disabled = decay_enabled();
        disabled.enabled = false;
        lifecycle_scheduler_job(policy(disabled))(registry)
            .await
            .expect("a disabled tick is a successful no-op");

        assert!(
            !fact_is_invalid(&db, namespace, "fact:old").await,
            "a disabled lifecycle must leave facts untouched"
        );
    }

    /// The conformance suite's in-memory registry has no privileged tenant
    /// engine; nothing can be bound, so there is nothing to maintain — and
    /// saying nothing about it is the correct answer, not an error.
    #[tokio::test]
    async fn a_registry_without_an_engine_is_a_quiet_no_op() {
        let registry = RegistryHandle::in_memory();

        lifecycle_scheduler_job(policy(decay_enabled()))(registry)
            .await
            .expect("a registry with no engine is a successful no-op");
    }
}
