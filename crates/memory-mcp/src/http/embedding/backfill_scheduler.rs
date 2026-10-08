//! Automatic per-tenant backfill of missing fact embeddings.
//!
//! A namespace whose facts carry no vector is a *gap*, not a disagreement: the
//! facts were written while the provider was unreachable, or before embeddings
//! were enabled, and nothing about them is wrong. Filling those gaps is
//! therefore automatic — this job — while rewriting a vector another provider
//! wrote is an operator's decision and belongs to the reembed route.
//!
//! The pass is a bounded process-level walk, exactly like
//! [`crate::http::app_sessions::scheduler`]: it never creates a per-tenant
//! loop, because the runtime pool evicts idle tenants and the need outlives any
//! one runtime.
//!
//! Two properties this module is shaped around:
//!
//! - **It never reimplements the batch loop.** `service::embedding_recovery::run_backfill`
//!   already selects on `embedding IS NONE`, writes through
//!   `embedding::api::generate_and_update` with `VectorWritePolicy::FillMissing`,
//!   honours the input-length limit and the disabled-provider check, and
//!   invalidates the context cache per fact. This module supplies the policy,
//!   the per-tenant service, and the durable marker — nothing else.
//! - **It reads no environment.** `EMBEDDINGS_AUTO_RECOVERY` was parsed at the
//!   composition root and arrives inside [`DeploymentPolicy`]. A tick that
//!   called `env::var` would make its own behaviour depend on a moment nobody
//!   logged, and would make the gate untestable without mutating the process
//!   environment the parallel test harness shares.

use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::MemoryError;
use crate::http::leases::scheduler::SchedulerJob;
use crate::http::leases::scheduler::cadence_due_at;
use crate::http::registry::RegistryHandle;
use crate::http::registry::models::Tenant;
use crate::http::runtime::bootstrap::DeploymentPolicy;
use crate::http::runtime::storage::EmbeddingPolicy;
use crate::storage::client::BoundDbClient;

/// How many ready tenants one pass walks, and how many facts one tenant's
/// pass batches. Both bounded, both the same numbers the other maintenance
/// jobs use: a tick that grew unbounded would hold the maintenance semaphore
/// for as long as the backlog, and the scheduler runs jobs concurrently.
const TENANT_BATCH: usize = 100;
const FACT_BATCH: i32 = 100;

/// The metric label for one completed pass. `ok` means every tenant in the
/// batch was backfilled (or had nothing to fill); `degraded` means at least
/// one tenant's pass failed and the pass continued past it.
const JOB_METRIC: &str = "embedding_backfill";

/// How often the backfill job actually walks the registry.
///
/// The scheduler ticks every second; backfill is not a per-second job. It
/// reads the tenant list, binds each namespace and probes its HNSW index —
/// work a small box pays for on every tick if it is ungated, and whose only
/// observable output was a line an operator could not pace. 60s is the same
/// cadence the plan-reconcile and provisioning passes already use.
const BACKFILL_CADENCE: Duration = Duration::from_secs(60);

/// The backfill job. Registers itself with
/// `SchedulerHooks::with_additional_job`.
///
/// The deployment policy is captured by the closure rather than read at
/// startup of the tick: a [`SchedulerJob`] receives only the
/// [`RegistryHandle`], so there is nowhere else to carry it — and capturing it
/// is what makes the gate a value the operator set once, rather than a lookup
/// the tick repeats.
///
/// The cadence gate is captured the same way. It lives in the closure, not a
/// `static`, so every job instance owns its own window: a process-wide static
/// would be shared by every test in the binary, and the second test's first
/// tick would be skipped because the first test opened it. Production builds
/// one job, so one window governs it.
pub fn backfill_scheduler_job(policy: DeploymentPolicy) -> SchedulerJob {
    let last_run: Arc<std::sync::Mutex<Option<Instant>>> = Arc::new(std::sync::Mutex::new(None));
    Arc::new(move |registry| {
        let policy = policy.clone();
        let last_run = Arc::clone(&last_run);
        Box::pin(async move {
            // Cheapest check first: a tick inside the window must not even
            // read the registry, which is the cost this gate exists to avoid.
            if !cadence_due_at(&last_run, BACKFILL_CADENCE, Instant::now()) {
                return Ok(());
            }
            run_backfill_pass(&registry, &policy).await
        })
    })
}

/// One bounded pass over the ready tenants.
///
/// Errors never abort the pass: a provider that is down for one tenant is down
/// for the next hundred, and stopping at the first failure would make the job's
/// progress a function of tenant ordering. Each failure is logged with an
/// `http.embedding.*` operation and counted into the `degraded` outcome.
pub async fn run_backfill_pass(
    registry: &RegistryHandle,
    policy: &DeploymentPolicy,
) -> Result<(), MemoryError> {
    let Some(embedding) = policy.embedding.as_ref() else {
        // No embedding policy means the deployment serves lexical retrieval
        // only; there is nothing to backfill and nothing to say about it every
        // tick.
        log_disabled("no_embedding_policy");
        return Ok(());
    };
    if !policy.auto_recovery {
        log_disabled("auto_recovery_off");
        return Ok(());
    }

    let tenants = registry
        .tenants()
        .list_ready_tenants(None, TENANT_BATCH)
        .await?;
    // The in-memory registry used by the conformance suite has no privileged
    // tenant engine. Nothing can be bound, so there is nothing to backfill —
    // and saying so is better than reporting an empty pass as a successful one.
    let Some(engine) = registry.tenant_engine_optional() else {
        return Ok(());
    };

    // Measured around the whole pass, like every other maintenance job: a
    // growing backlog shows up here as one rising series rather than as one
    // histogram per tenant.
    let started = Instant::now();
    let mut degraded = false;

    for tenant in tenants {
        let db = match engine.bind(&tenant).await {
            Ok(db) => db,
            Err(error) => {
                degraded = true;
                log_tenant_failure("http.embedding.backfill_bind_failed", &tenant, &error);
                continue;
            }
        };
        if let Err(error) = backfill_tenant(db, &tenant, embedding, policy).await {
            degraded = true;
            log_tenant_failure("http.embedding.backfill_failed", &tenant, &error);
        }
    }

    crate::observability::record_job_metric(
        JOB_METRIC,
        if degraded { "degraded" } else { "ok" },
        started.elapsed().as_secs_f64(),
    );
    Ok(())
}

/// Fill one tenant's gaps and record the namespace as ready.
///
/// A service rather than a bare client, because `run_backfill` needs the
/// service's logger and context cache — it invalidates the cache per fact so
/// a fact's new vector is visible to the next read, which a bare client has no
/// cache to invalidate.
async fn backfill_tenant(
    db: Arc<crate::storage::client::SurrealDbClient>,
    tenant: &Tenant,
    embedding: &EmbeddingPolicy,
    policy: &DeploymentPolicy,
) -> Result<(), MemoryError> {
    let namespace = tenant.namespace_binding.namespace.clone();
    // One decision, shared with the activation path. A namespace whose stored
    // vectors sit at another dimension is Class B — only an operator's reembed
    // may rewrite them — and backfill must decline it *before* it calls the
    // provider: a 2048-wide vector written into a 1536-wide index is rejected
    // by the database, but only after the provider has been paid for it. On a
    // tick that runs for every affected tenant, every time, with the job
    // reported degraded on each of them.
    //
    // The same call also repairs the Class A case activation never touched — a
    // ready tenant that has never been activated — by re-declaring an index
    // that stands under no vectors, which is what makes its writes land.
    //
    // `Unreadable` declines for the same reason as `ForeignVectors`, and the
    // gate's own contract says why: the caller must not guess, because a wrong
    // guess is a write that fails after it has been paid for. Activation
    // tolerates an unreadable index because lexical retrieval does not need
    // it; backfill cannot, because it is about to *write vectors*. The two
    // callers share one reading and draw different consequences from it.
    let gate =
        crate::http::runtime::storage::reconcile_tenant_index_dimension(&db, &namespace, embedding)
            .await;
    if let Some(reason) = declined_reason(gate) {
        log_declined(tenant, embedding.dimension, reason);
        return Ok(());
    }
    let extractor = match policy.entity_extractor.clone() {
        Some(extractor) => extractor,
        None => Arc::new(crate::knowledge::entity_extraction::AnnoEntityExtractor::new()?)
            as Arc<dyn crate::knowledge::entity_extraction::EntityExtractor>,
    };
    let service = crate::service::MemoryService::new_with_embedding_provider(
        Arc::clone(&db) as Arc<dyn crate::storage::DbClient>,
        namespace.clone(),
        crate::logging::StdoutLogger::directives_from_env(),
        100, // rate_limit_rps; the maintenance pass is not request traffic
        100, // rate_limit_burst
        embedding.provider.clone(),
        crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
        extractor,
    )?;

    let processed = match crate::service::embedding_recovery::run_backfill(
        &crate::service::embedding_recovery::RecoveryHandles::from(&service),
        embedding.provider.clone(),
        &embedding.signature,
        embedding.model.as_deref(),
        embedding.dimension,
        FACT_BATCH,
    )
    .await?
    {
        crate::service::embedding_recovery::BackfillOutcome::Complete { processed } => processed,
    };

    // The namespace stops reporting `backfill_pending` only once its vectors
    // are actually there. Written after the pass rather than before, so a crash
    // mid-pass leaves the marker in place and the next tick resumes — which is
    // what makes the marker worth writing at all.
    write_bootstrap_ready(&namespace, embedding, &db).await?;

    log_backfill_completed(&namespace, processed);
    Ok(())
}

/// Persist `status = "ready"` for the namespace's embedding identity.
async fn write_bootstrap_ready(
    namespace: &str,
    embedding: &EmbeddingPolicy,
    db: &Arc<crate::storage::client::SurrealDbClient>,
) -> Result<(), MemoryError> {
    // `BoundDbClient` pins the namespace as an invariant rather than a
    // per-call argument, so this write physically cannot reach another
    // tenant's rows.
    let bound_db = BoundDbClient::new(
        Arc::clone(db) as Arc<dyn crate::storage::DbClient>,
        namespace,
    );
    crate::service::startup::write_bootstrap_ready_state(
        &bound_db,
        &embedding.signature,
        embedding.provider_label,
        embedding.model.as_deref(),
        embedding.dimension,
        false,
    )
    .await
}

/// The tick naming itself as a no-op.
///
/// At `Debug`, not `Info`: a deployment that turned recovery off has this line
/// on every tick, and an operator who wants to see why nothing is happening
/// asks for it by name (`RUST_LOG=http.embedding=debug`). At `Info` it would be
/// the loudest line in a log that is otherwise silent.
fn log_disabled(reason: &'static str) {
    let mut event = std::collections::HashMap::new();
    event.insert(
        "op".to_string(),
        serde_json::json!("http.embedding.backfill_disabled"),
    );
    event.insert("reason".to_string(), serde_json::json!(reason));
    crate::logging::emit(event, crate::logging::LogLevel::Debug);
}

fn log_backfill_completed(namespace: &str, processed: usize) {
    let mut event = std::collections::HashMap::new();
    event.insert(
        "op".to_string(),
        serde_json::json!("http.embedding.backfill_completed"),
    );
    event.insert("namespace".to_string(), serde_json::json!(namespace));
    event.insert("processed".to_string(), serde_json::json!(processed));
    crate::logging::emit(event, crate::logging::LogLevel::Info);
}

/// A per-tenant failure, named by the tenant and kept to one prose detail.
///
/// `log_warn` rather than a hand-rolled line: it is the one path every HTTP
/// runtime warning goes through, and the reason it counts its occurrences is
/// that a backfill provider that is down fails the same way for every tenant in
/// the batch.
fn log_tenant_failure(op: &'static str, tenant: &Tenant, error: &MemoryError) {
    crate::http::logging::log_warn(op, &format!("tenant {}: {error}", tenant.id));
}

/// Whether an [`IndexWriteGate`] stops the tick from writing, and why.
///
/// This is the policy that makes backfill safe to run unattended, stated in
/// one place so it can be read and pinned directly. Two of the four readings
/// decline, and the second one is the easy one to "simplify" away:
///
/// - [`ForeignVectors`](crate::http::runtime::storage::IndexWriteGate::ForeignVectors)
///   — vectors exist at a width the deployment no longer writes. Rewriting them
///   is a reembed, and attempting it anyway pays the provider for a write the
///   database rejects.
/// - [`Unreadable`](crate::http::runtime::storage::IndexWriteGate::Unreadable)
///   — the index state could not be determined. The gate's own contract is that
///   the caller must not guess, because guessing wrong is a write that fails
///   after it has been paid for. Activation tolerates this; backfill is about to
///   write vectors, so it does not.
fn declined_reason(gate: crate::http::runtime::storage::IndexWriteGate) -> Option<&'static str> {
    use crate::http::runtime::storage::IndexWriteGate;
    match gate {
        IndexWriteGate::Matches | IndexWriteGate::Redeclared => None,
        IndexWriteGate::ForeignVectors => Some("stored_vectors_at_other_dimension"),
        IndexWriteGate::Unreadable => Some("index_state_unknown"),
    }
}

/// A tenant the tick deliberately did not touch, with the reason it gave.
///
/// `Info`, not `Warn`, and never counted as degraded: this is the job working
/// correctly. A decline that were reported as a failure would page an operator
/// for every tenant an operator has not reembedded yet — which is every tenant
/// in the fleet after a provider change.
fn log_declined(tenant: &Tenant, dimension: usize, reason: &'static str) {
    let mut event = std::collections::HashMap::new();
    event.insert(
        "op".to_string(),
        serde_json::json!("http.embedding.backfill_declined"),
    );
    event.insert("tenant".to_string(), serde_json::json!(tenant.id));
    event.insert(
        "namespace".to_string(),
        serde_json::json!(tenant.namespace_binding.namespace),
    );
    event.insert("dimension".to_string(), serde_json::json!(dimension));
    event.insert("reason".to_string(), serde_json::json!(reason));
    crate::logging::emit(event, crate::logging::LogLevel::Info);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::runtime::bootstrap::DeploymentPolicy;
    use crate::http::runtime::storage::EmbeddingPolicy;

    async fn empty_registry() -> RegistryHandle {
        RegistryHandle::in_memory_with_default_mem_engine().await
    }

    /// An enabled provider that answers deterministically and offline, so the
    /// gate can be observed without a network round trip.
    struct StaticProvider;

    #[async_trait::async_trait]
    impl crate::embedding::providers::EmbeddingProvider for StaticProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "openai-compatible"
        }

        fn dimension(&self) -> usize {
            crate::config::DEFAULT_EMBEDDING_DIMENSION
        }

        async fn embed(&self, _input: &str) -> Result<Vec<f64>, crate::error::MemoryError> {
            Ok(vec![0.0; self.dimension()])
        }
    }

    fn policy(auto_recovery: bool) -> DeploymentPolicy {
        DeploymentPolicy {
            embedding: Some(EmbeddingPolicy {
                provider: Arc::new(StaticProvider),
                dimension: crate::config::DEFAULT_EMBEDDING_DIMENSION,
                signature: "embsig:test".to_string(),
                model: None,
                provider_label: "openai-compatible",
            }),
            entity_extractor: None,
            lifecycle: crate::config::LifecycleConfig::default(),
            cache_limits: crate::config::CacheLimits::profile_default(),
            auto_recovery,
            query_logging_enabled: false,
            query_log_retention_days: crate::config::DEFAULT_QUERY_LOG_RETENTION_DAYS,
            embedding_similarity_threshold: crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            claim_config: crate::config::claims::ClaimConfig::default(),
        }
    }

    /// The gate has to *say* it is closed. An operator who set
    /// `EMBEDDINGS_AUTO_RECOVERY=false` and then watched nothing happen has no
    /// way to tell an off switch from a job that never ran, and the log line
    /// is the only place that distinction exists. This asserts the rendered
    /// line — including the level — because the level is what `RUST_LOG` reads.
    #[tokio::test]
    async fn a_disabled_tick_names_itself_at_debug() {
        let registry = empty_registry().await;
        let policy = policy(false);

        let sink = crate::logging::capture::install();
        crate::logging::capture::with_level("http=debug", || run_backfill_pass(&registry, &policy))
            .await
            .expect("a disabled tick is a successful no-op");

        let recorded = sink.lines();
        assert!(
            recorded
                .iter()
                .any(|line| line.contains("op=http.embedding.backfill_disabled")
                    && line.contains("DEBUG")
                    && line.contains("auto_recovery_off")),
            "the disabled tick must name itself, and why, at a level an operator \
             can ask for: {recorded:?}"
        );
    }

    /// A deployment with no embeddings has nothing to backfill, and saying so
    /// once per tick under the same name would be indistinguishable from the
    /// opt-out. The two are separate reasons and are logged as such.
    #[tokio::test]
    async fn a_deployment_without_an_embedding_policy_says_so_instead_of_scanning() {
        let registry = empty_registry().await;
        let mut deployment = policy(true);
        deployment.embedding = None;

        let sink = crate::logging::capture::install();
        crate::logging::capture::with_level("http=debug", || {
            run_backfill_pass(&registry, &deployment)
        })
        .await
        .expect("a lexical-only deployment is a successful no-op");

        let recorded = sink.lines();
        assert!(
            recorded.iter().any(|line| {
                line.contains("op=http.embedding.backfill_disabled")
                    && line.contains("no_embedding_policy")
            }),
            "a lexical-only deployment must say the policy is absent, not that \
             recovery was switched off: {recorded:?}"
        );
    }

    /// The job entry point itself, not just the pass: the closure is where the
    /// policy is captured, and a closure that dropped it would leave a job that
    /// silently stops backfilling.
    #[tokio::test]
    async fn the_job_entry_point_runs_a_disabled_pass() {
        let registry = empty_registry().await;

        let observed = backfill_scheduler_job(policy(false))(registry).await;

        assert!(observed.is_ok(), "an empty pass is a valid pass");
    }

    /// The service `backfill_tenant` builds must carry the deployment's level,
    /// not a hardcoded `"info"`.
    ///
    /// This was the logger `RUST_LOG` could not reach: built with the literal
    /// `"info"`, so `embedding.backfill_started` — the very line an operator
    /// tries to quiet — printed no matter what the deployment set, while the
    /// neighbouring `http.embedding.backfill_completed`, which goes through
    /// `from_env`, obeyed the same directive.
    #[tokio::test]
    async fn the_backfill_tick_obeys_the_deployment_log_level() {
        use crate::http::registry::models::{NamespaceBinding, TenantStatus};
        use crate::storage::client::DbClient;

        let namespace = "tns_backfill_level";
        let registry = empty_registry().await;
        let engine = registry.tenant_engine_optional().expect("engine wired");
        let db = engine.bind_to_test_namespace(namespace).await;
        db.apply_migrations(namespace).await.expect("migrations");
        // Three facts, not one: `embedding.backfill_started` carries
        // `total_missing`, and a count no other test produces is what lets this
        // assertion pick *its own* line out of a sink that receives every test's
        // output. One fact would collide with the cadence test's `total_missing=1`.
        for fact_id in ["fact:lvl1", "fact:lvl2", "fact:lvl3"] {
            seed_fact_without_vector(&db, namespace, fact_id).await;
        }
        // Register the tenant ready, so the bounded walk reaches it — a
        // registry with no ready tenant would pass without running a backfill.
        registry
            .tenants()
            .write_tenant(&Tenant {
                id: "ten_backfill_lvl".to_string(),
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
            .expect("register the tenant as ready");

        let sink = crate::logging::capture::install();
        crate::logging::capture::with_level("warn", || {
            backfill_scheduler_job(policy(true))(registry)
        })
        .await
        .expect("a tick under a quiet level still succeeds");

        let recorded = sink.lines();
        // `total_missing=3` is this test's own count, so a sibling backfill's
        // `total_missing=1` (the cadence test) or `0` (the empty-store test)
        // cannot satisfy — or mask — the assertion.
        assert!(
            !recorded.iter().any(|line| {
                line.contains("op=embedding.backfill_started") && line.contains("total_missing=3")
            }),
            "the backfill service logger must obey the deployment level, not a \
             hardcoded info: {recorded:?}"
        );
    }

    /// The gate must open, not just close. [`the_backfill_job_gates_its_cadence`]
    /// pins the closed side; a gate that opened once and then stayed shut for
    /// the process's lifetime would pass that test while backfilling exactly
    /// once ever, so the opening is pinned here against synthetic instants —
    /// no test should sleep through a real minute to prove it.
    #[test]
    fn the_cadence_gate_opens_when_the_window_has_elapsed() {
        let last_run = std::sync::Mutex::new(None);
        let t0 = Instant::now();

        assert!(
            cadence_due_at(&last_run, Duration::from_secs(60), t0),
            "the first run always opens the gate"
        );
        assert!(
            !cadence_due_at(
                &last_run,
                Duration::from_secs(60),
                t0 + Duration::from_secs(30)
            ),
            "a run inside the window is gated"
        );
        assert!(
            cadence_due_at(
                &last_run,
                Duration::from_secs(60),
                t0 + Duration::from_secs(60)
            ),
            "a run at the boundary opens the gate again"
        );
        assert!(
            !cadence_due_at(
                &last_run,
                Duration::from_secs(60),
                t0 + Duration::from_secs(90)
            ),
            "and the window closes from the new anchor"
        );
    }

    /// Seed one fact with no vector, so a backfill pass has a gap to fill.
    async fn seed_fact_without_vector(
        db: &crate::storage::client::SurrealDbClient,
        namespace: &str,
        fact_id: &str,
    ) {
        use crate::storage::client::DbClient;
        let now = crate::shared::temporal::normalize_dt(chrono::Utc::now());
        db.create(
            fact_id,
            serde_json::json!({
                "fact_id": fact_id,
                "fact_type": "note",
                "content": format!("content {fact_id}"),
                "quote": format!("content {fact_id}"),
                "source_episode": "episode:seed",
                "t_valid": now,
                "t_ingested": now,
                "confidence": 0.9,
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
        .expect("seed a fact with no vector");
    }

    /// The scheduler ticks once a second, and an ungated backfill walked the
    /// registry, bound every tenant and probed its index on each of them — a
    /// query storm on a small box and a line an operator could not pace. The
    /// gate lives in the job closure, so a second tick inside the cadence is a
    /// no-op while the first tick runs immediately.
    #[tokio::test]
    async fn the_backfill_job_gates_its_cadence() {
        use crate::http::registry::models::{NamespaceBinding, TenantStatus};
        use crate::storage::client::DbClient;

        let namespace = "tns_backfill_cadence";
        let registry = empty_registry().await;
        let engine = registry.tenant_engine_optional().expect("engine wired");
        let db = engine.bind_to_test_namespace(namespace).await;
        db.apply_migrations(namespace).await.expect("migrations");
        seed_fact_without_vector(&db, namespace, "fact:a").await;
        registry
            .tenants()
            .write_tenant(&Tenant {
                id: "ten_backfill_cadence".to_string(),
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
            .expect("register the tenant as ready");

        let job = backfill_scheduler_job(policy(true));

        // The first tick must run: it fills fact:a.
        job(registry.clone()).await.expect("the first tick runs");
        let a = db
            .select_one("fact:a", namespace)
            .await
            .expect("read fact:a")
            .expect("fact:a exists");
        assert!(
            a.get("embedding_dimension").is_some(),
            "the first tick must run and fill fact:a"
        );

        // A gap that appears immediately after must wait for the cadence.
        seed_fact_without_vector(&db, namespace, "fact:b").await;
        job(registry)
            .await
            .expect("a gated tick is still a successful no-op");
        let b = db
            .select_one("fact:b", namespace)
            .await
            .expect("read fact:b")
            .expect("fact:b exists");
        assert!(
            b.get("embedding_dimension").is_none(),
            "the second tick inside the cadence must not run a backfill"
        );
    }

    /// An enabled deployment with no ready tenants walks the registry and finds
    /// nothing, which is a pass that succeeded — not a failure, and not a pass
    /// that never ran.
    #[tokio::test]
    async fn an_enabled_tick_over_no_ready_tenants_succeeds() {
        let registry = empty_registry().await;

        let observed = run_backfill_pass(&registry, &policy(true)).await;

        assert!(observed.is_ok());
    }

    /// An index already at the deployment's width lets the tick write.
    #[tokio::test]
    async fn a_matching_index_does_not_decline() {
        use crate::http::runtime::storage::IndexWriteGate;
        assert_eq!(declined_reason(IndexWriteGate::Matches), None);
    }

    /// An index re-declared because no vector stands under it is now correct,
    /// which is the case that makes an unactivated tenant backfillable at all.
    #[tokio::test]
    async fn an_index_just_redeclared_for_a_vectorless_namespace_does_not_decline() {
        use crate::http::runtime::storage::IndexWriteGate;
        assert_eq!(declined_reason(IndexWriteGate::Redeclared), None);
    }

    /// Vectors stored at another provider's width are a reembed's business.
    /// Attempting this would pay the provider for a write the database rejects,
    /// on every tick, for as long as the deployment stays this way.
    #[tokio::test]
    async fn a_namespace_holding_vectors_at_another_width_is_declined() {
        use crate::http::runtime::storage::IndexWriteGate;
        assert_eq!(
            declined_reason(IndexWriteGate::ForeignVectors),
            Some("stored_vectors_at_other_dimension")
        );
    }

    /// An index that could not be read must not be assumed correct. The gate's
    /// contract is that the caller must not guess, because a wrong guess is a
    /// write that fails after it has been paid for — and unlike activation,
    /// which tolerates the unknown because lexical retrieval does not need the
    /// index, backfill is about to write vectors into it.
    #[tokio::test]
    async fn an_unreadable_index_is_declined_rather_than_guessed_at() {
        use crate::http::runtime::storage::IndexWriteGate;
        assert_eq!(
            declined_reason(IndexWriteGate::Unreadable),
            Some("index_state_unknown")
        );
    }
}
