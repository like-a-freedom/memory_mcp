//! HTTP composition-root helpers.
//!
//! Pure startup functions, no CLI parsing. The binary's `main` only
//! reads env, dispatches to these helpers, and serves.

use std::process::ExitCode;
use std::sync::Arc;

use crate::logging::StdoutLogger;

use super::super::HttpState;
use super::super::composition::HttpProductionComposition;
use super::super::config::HttpConfig;
use super::super::leases::migration::ApplyMigrations;
use crate::http::runtime::storage::EmbeddingPolicy;
use crate::knowledge::entity_extraction::EntityExtractor;
use crate::platform::fault_injection::FaultInjector;

/// The startup-composed HTTP runtime. The binary keeps
/// `tenant_migrations` alive for the scheduler hooks; it never
/// re-selects either adapter after this point (ADR-0053). The fault
/// injector is threaded into the scheduler and the deletion worker
/// the same way.
pub struct HttpRuntime {
    pub state: std::sync::Arc<HttpState>,
    pub tenant_migrations: std::sync::Arc<dyn ApplyMigrations>,
    pub fault_injector: Arc<dyn FaultInjector>,
    /// The deployment-level policy every tenant runtime is built from.
    ///
    /// Resolved once here because the provider is process-global (spec
    /// §7.1, §13) and its dimension probe is a network round trip.
    /// Per-namespace eligibility is still decided per tenant at activation.
    pub deployment_policy: DeploymentPolicy,
}

/// The deployment-level embedding and entity-extractor policy.
#[derive(Clone)]
pub struct DeploymentPolicy {
    /// `None` when the operator did not enable embeddings, in which
    /// case tenants serve lexical retrieval only.
    pub embedding: Option<EmbeddingPolicy>,
    pub entity_extractor: Option<Arc<dyn EntityExtractor>>,
    pub lifecycle: crate::config::LifecycleConfig,
    /// `EMBEDDINGS_AUTO_RECOVERY`: whether the backfill scheduler job scans
    /// tenants at all.
    ///
    /// Parsed here, once, and carried into the job rather than read at the
    /// tick. Config belongs to the composition root (12-factor III), and a tick
    /// that called `env::var` would make its own behaviour depend on the
    /// process environment at a moment nobody logged.
    ///
    /// The default is `true`, matching `EmbeddingConfig::auto_recovery`, so
    /// the two readings of one variable cannot disagree.
    pub auto_recovery: bool,
    /// `QUERY_LOGGING_ENABLED`: whether `assemble_context` persists an
    /// analytics row per tenant. Read here so the HTTP profile honours the
    /// variable the stdio profile reads through `SurrealConfig`.
    pub query_logging_enabled: bool,
    /// `QUERY_LOG_RETENTION_DAYS`: how long those rows are kept before
    /// best-effort pruning.
    pub query_log_retention_days: u32,
    /// `EMBEDDINGS_SIMILARITY_THRESHOLD`: the minimum cosine similarity for a
    /// semantic match. The stdio profile passes it straight to the service;
    /// the HTTP profile used to fall back to the hard-coded default, so an
    /// operator's `0.9` silently stayed `0.7`.
    pub embedding_similarity_threshold: f64,
}

/// Resolve the deployment-level embedding and entity-extractor policy from the
/// environment.
///
/// Mirrors what `bootstrap::stdio` does for the Active Namespace, minus the
/// per-namespace decision: this reads only `EMBEDDINGS_*`,
/// `SURREALDB_EMBEDDING_DIMENSION`, `NER_*` and `LIFECYCLE_*`, and returns what
/// every tenant will start from.
pub async fn resolve_deployment_policy(logger: &StdoutLogger) -> Result<DeploymentPolicy, String> {
    // `EmbeddingConfig::from_env` is the one parser of `EMBEDDINGS_AUTO_RECOVERY`
    // in this crate, and this profile already calls it through
    // `resolve_embedding_policy`. The flag is read off that config rather than
    // through a second `env::var`, so the two profiles cannot disagree about
    // what the variable means and the composition root remains the only place
    // the environment is read.
    //
    // It is re-read here rather than returned alongside `embedding` because
    // that function returns `None` for a disabled provider, and a lexical-only
    // deployment still has to know what its gate says.
    let embedding_config = crate::config::EmbeddingConfig::from_env()
        .map_err(|err| format!("embedding config error: {err}"))?;
    let embedding = resolve_embedding_policy(logger).await?;
    let lifecycle = crate::config::LifecycleConfig::from_env();
    let entity_extractor = resolve_entity_extractor(logger, embedding.is_some()).await?;
    // The stdio profile reads these through `SurrealConfig::from_env`, which
    // this binary never calls; parsing them here is what makes the same two
    // variables mean the same thing in both profiles.
    let query_logging_enabled =
        crate::config::parse_bool_env("QUERY_LOGGING_ENABLED").unwrap_or(false);
    let query_log_retention_days = crate::config::parse_env::<u32>("QUERY_LOG_RETENTION_DAYS")
        .map_err(|err| format!("query log config error: {err}"))?
        .unwrap_or(crate::config::DEFAULT_QUERY_LOG_RETENTION_DAYS);
    Ok(DeploymentPolicy {
        embedding,
        entity_extractor,
        lifecycle,
        auto_recovery: embedding_config.auto_recovery,
        query_logging_enabled,
        query_log_retention_days,
        // Parsed by `EmbeddingConfig` above; carried here because the stdio
        // profile passes it straight to its service and the HTTP profile must
        // not silently drop it.
        embedding_similarity_threshold: embedding_config.similarity_threshold,
    })
}

/// Where a model-backed extractor keeps its weights.
///
/// HTTP has no per-client data directory, so an operator sets
/// `MEMORY_MCP_HTTP_DATA_DIR`; the temp dir keeps a local Candle/GLiNER
/// deployment working with no configuration.
fn ner_model_dir() -> String {
    std::env::var("MEMORY_MCP_HTTP_DATA_DIR").unwrap_or_else(|_| {
        std::env::temp_dir()
            .join("memory-mcp-http-models")
            .to_string_lossy()
            .to_string()
    })
}

/// Build the configured entity extractor, or `None` to let every tenant fall
/// back to its built-in one.
///
/// When embeddings are on, a model this deployment cannot load is fatal: the
/// operator asked for an environment this profile cannot provide, and silently
/// dropping to `anno` would change what ingest produces. With embeddings off
/// the fallback is the documented default, so a failure is only a warning.
async fn resolve_entity_extractor(
    logger: &StdoutLogger,
    embeddings_enabled: bool,
) -> Result<Option<Arc<dyn EntityExtractor>>, String> {
    let config =
        crate::config::NerConfig::from_env().map_err(|err| format!("NER config error: {err}"))?;
    let attempt = crate::knowledge::entity_extraction::create_entity_extractor_with_progress(
        &config,
        &ner_model_dir(),
        logger,
        Arc::new(crate::embedding::model_artifacts::JsonLineProgressSink::new()),
    )
    .await
    .map_err(|err| format!("entity extractor init error: {err}"));
    match attempt {
        Ok(extractor) => Ok(Some(extractor)),
        Err(message) if embeddings_enabled => Err(message),
        Err(message) => {
            let mut event = std::collections::HashMap::new();
            event.insert(
                "op".to_string(),
                serde_json::json!("http.entity_extractor_unavailable"),
            );
            event.insert("reason".to_string(), serde_json::json!(message));
            logger.log(event, crate::logging::LogLevel::Warn);
            Ok(None)
        }
    }
}

/// Resolve the deployment-level embedding policy from the environment.
///
/// Reads only `EMBEDDINGS_*` and `SURREALDB_EMBEDDING_DIMENSION`, resolves the
/// provider's real dimension, and returns the shared provider every tenant will
/// be reconciled against.
pub async fn resolve_embedding_policy(
    logger: &StdoutLogger,
) -> Result<Option<EmbeddingPolicy>, String> {
    let config = crate::config::EmbeddingConfig::from_env()
        .map_err(|err| format!("embedding config error: {err}"))?;
    if !config.is_enabled() {
        return Ok(None);
    }
    // A data directory only matters to local providers (their model cache).
    let data_dir = ner_model_dir();
    let target = match crate::service::startup::resolve_embedding_preflight(
        &config, &data_dir, logger,
    )
    .await
    {
        Some(target) => target,
        None => {
            return Err(format!(
                "EMBEDDINGS_ENABLED is set but the configured provider could not be resolved: \
                 provider={} model={:?}",
                config.provider_label(),
                config.model
            ));
        }
    };
    let provider = crate::embedding::providers::create_embedding_provider_with_dimension(
        &config,
        &data_dir,
        target.dimension,
    )
    .await
    .map_err(|err| format!("embedding provider init error: {err}"))?;
    Ok(Some(EmbeddingPolicy {
        provider,
        dimension: target.dimension,
        signature: target.signature,
        model: target.model,
        provider_label: target.provider_label,
    }))
}

/// Compose the production adapters, build `HttpState`, and optionally
/// install a Prometheus recorder. Maps every error to a
/// `startup_error()` log line + `ExitCode::from(2)`.
///
/// The fault injector is read from the test-fixtures env var when the
/// `test-fixtures` feature is enabled; production builds install
/// [`NoFaults`].
pub async fn build_state(
    cfg: &HttpConfig,
    logger: &StdoutLogger,
) -> Result<HttpRuntime, (ExitCode, String)> {
    #[cfg(feature = "prometheus")]
    let metrics_handle = match crate::http::metrics::install_recorder() {
        Ok(h) => Some(h),
        Err(err) => return Err((ExitCode::from(2), format!("metrics init error: {err}"))),
    };
    #[cfg(not(feature = "prometheus"))]
    let metrics_handle = None;

    let fault_injector: Arc<dyn FaultInjector> = load_fault_injector();

    // Resolve the deployment policy before the state is assembled: the pool
    // holds the `RuntimeOptions` every tenant runtime is built from, so the
    // policy has to exist by the time the pool is constructed.
    let deployment_policy = match resolve_deployment_policy(logger).await {
        Ok(policy) => policy,
        Err(msg) => return Err((ExitCode::from(2), msg)),
    };
    if let Some(policy) = deployment_policy.embedding.as_ref() {
        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            serde_json::json!("http.embedding_policy_resolved"),
        );
        event.insert(
            "provider".to_string(),
            serde_json::json!(policy.provider_label),
        );
        event.insert("dimension".to_string(), serde_json::json!(policy.dimension));
        event.insert("model".to_string(), serde_json::json!(policy.model));
        event.insert("signature".to_string(), serde_json::json!(policy.signature));
        logger.log(event, crate::logging::LogLevel::Info);
    }

    let composition =
        match HttpProductionComposition::connect_with_injector(cfg, fault_injector.clone()).await {
            Ok(c) => c,
            Err(err) => {
                return Err((
                    ExitCode::from(2),
                    format!("tenant runtime init error: {err}"),
                ));
            }
        };
    let state = match HttpState::assemble_with_deployment_policy(
        cfg.clone(),
        composition.registry,
        metrics_handle,
        None,
        Some(deployment_policy.clone()),
    )
    .await
    {
        Ok(s) => s,
        Err(err) => {
            return Err((
                ExitCode::from(2),
                format!("tenant runtime init error: {err}"),
            ));
        }
    };
    Ok(HttpRuntime {
        state,
        tenant_migrations: composition.tenant_migrations,
        fault_injector: composition.fault_injector,
        deployment_policy,
    })
}

/// Resolve the fault injector the binary runs with. When the
/// `test-fixtures` feature is enabled the test-only env var
/// `MEMORY_MCP_HTTP_TEST_FAULT_POINT` selects a `FailOnceAt`; in every
/// other build the binary uses [`NoFaults`].
fn load_fault_injector() -> Arc<dyn FaultInjector> {
    #[cfg(any(test, feature = "test-fixtures"))]
    {
        crate::platform::fault_injection::FailOnceAt::from_env()
    }
    #[cfg(not(any(test, feature = "test-fixtures")))]
    {
        Arc::new(crate::platform::fault_injection::NoFaults)
    }
}

/// Validate that the stdio profile's `MEMORY_PROMETHEUS_LISTEN_ADDR`
/// is not set in HTTP mode. Only meaningful under the `prometheus`
/// feature.
#[cfg(feature = "prometheus")]
pub fn validate_no_listener_env() -> Result<(), String> {
    crate::http::metrics::validate_no_listener_env().map_err(|err| format!("config invalid: {err}"))
}

#[cfg(not(feature = "prometheus"))]
pub fn validate_no_listener_env() -> Result<(), String> {
    Ok(())
}

/// The profile this build serves, as it appears in the startup line.
///
/// It is the Cargo feature name, so a log query and a build command use the
/// same word. It was `streamable_http_saas`, which described the business
/// model rather than the shape: the profile is a Streamable HTTP server, and
/// whether it is sold as a SaaS is a deployment decision this binary cannot
/// see.
///
/// The `op` is the event's own name, so `RUST_LOG=http.start=debug` reaches
/// this line and a startup event is filterable like any other. It was `event`,
/// which the subsystem filter does not read.
const PROFILE: &str = "streamable-http";

/// Single startup line. Other structured events go through
/// `logging::request_log` middleware.
pub fn emit_startup_log(logger: &StdoutLogger, cfg: &HttpConfig) {
    let mut fields = std::collections::HashMap::new();
    fields.insert("op".into(), serde_json::Value::from("http.start"));
    fields.insert("profile".into(), serde_json::Value::from(PROFILE));
    fields.insert("bind".into(), serde_json::Value::from(cfg.bind.to_string()));
    fields.insert(
        "embedded_tenant_db".into(),
        serde_json::Value::from(cfg.tenant_db.url == "mem://"),
    );
    logger.log(fields, crate::logging::LogLevel::Info);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::runtime::storage::EmbeddingPolicy;

    /// The startup line is how an operator tells which build a deployment is
    /// running, and the first thing they compare is the profile against the
    /// feature they think they enabled. Those two words have to be the same
    /// word, and the feature is `streamable-http` — a Cargo feature name, not
    /// a Rust identifier, so the dash is kept rather than folded into an
    /// underscore.
    #[test]
    fn the_startup_profile_names_the_feature_this_build_compiled() {
        assert_eq!(PROFILE, "streamable-http");
    }

    /// The old name described the business model rather than the shape, and it
    /// was the only place in the repository still using it.
    #[test]
    fn the_profile_no_longer_names_a_business_model() {
        assert!(
            !PROFILE.contains("saas"),
            "the profile is a transport, not a commercial model: {PROFILE}"
        );
    }

    /// Run `test` with `vars` set, restoring the process environment after.
    ///
    /// `crate::config::env_lock` is the repository's single serialization
    /// point for environment access, and every env-mutating test takes it. It
    /// is a `std::sync::Mutex` guard and so cannot be held across an `.await`,
    /// which is why the callers below block a runtime *inside* the closure
    /// rather than making these tests `async`.
    fn with_env_vars<T>(vars: &[(&str, Option<&str>)], test: impl FnOnce() -> T) -> T {
        let _guard = crate::config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let saved = vars
            .iter()
            .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
            .collect::<Vec<_>>();
        for (key, value) in vars {
            // SAFETY: the guard above is this repository's env lock, so no
            // other test mutates the environment concurrently.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
        let outcome = test();
        for (key, value) in saved {
            // SAFETY: same guard as above.
            unsafe {
                match value {
                    Some(value) => std::env::set_var(&key, value),
                    None => std::env::remove_var(&key),
                }
            }
        }
        outcome
    }

    /// Resolve the embedding policy on a throwaway current-thread runtime.
    fn policy_for_env(vars: &[(&str, Option<&str>)]) -> Result<Option<EmbeddingPolicy>, String> {
        with_env_vars(vars, || {
            let logger = StdoutLogger::new("error");
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(resolve_embedding_policy(&logger))
        })
    }

    /// Resolve the whole deployment policy on a throwaway current-thread
    /// runtime, with `vars` in force for the duration.
    fn deployment_policy_for_env(vars: &[(&str, Option<&str>)]) -> DeploymentPolicy {
        with_env_vars(vars, || {
            let logger = StdoutLogger::new("error");
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime")
                .block_on(resolve_deployment_policy(&logger))
                .expect("deployment policy resolves")
        })
    }

    /// `QUERY_LOGGING_ENABLED` and `QUERY_LOG_RETENTION_DAYS` were read only
    /// by the stdio `SurrealConfig`, which the HTTP binary never calls, so an
    /// HTTP deployment that set them got no `query_log` rows at all — silently,
    /// with the default `false`. The policy must carry both from the
    /// environment to the tenant service.
    #[test]
    fn query_logging_reaches_the_deployment_policy_from_the_environment() {
        let policy = deployment_policy_for_env(&[
            ("QUERY_LOGGING_ENABLED", Some("true")),
            ("QUERY_LOG_RETENTION_DAYS", Some("7")),
        ]);

        assert!(
            policy.query_logging_enabled,
            "QUERY_LOGGING_ENABLED=true must reach the policy, not stay at the default false"
        );
        assert_eq!(
            policy.query_log_retention_days, 7,
            "QUERY_LOG_RETENTION_DAYS must reach the policy, not the 90-day default"
        );
    }

    /// `EMBEDDINGS_SIMILARITY_THRESHOLD` was parsed by `EmbeddingConfig` and
    /// then dropped: the policy never carried it, so the tenant service always
    /// ran on the hard-coded default and an operator's `0.9` silently stayed
    /// `0.7`. README promises the variable has the same meaning in HTTP as in
    /// stdio, so the policy must carry it.
    #[test]
    fn the_similarity_threshold_reaches_the_deployment_policy_from_the_environment() {
        let policy = deployment_policy_for_env(&[("EMBEDDINGS_SIMILARITY_THRESHOLD", Some("0.9"))]);

        assert_eq!(
            policy.embedding_similarity_threshold, 0.9,
            "EMBEDDINGS_SIMILARITY_THRESHOLD must reach the policy, not the 0.7 default"
        );
    }

    /// The regression that made this an HTTP-profile bug at all: setting
    /// `EMBEDDINGS_*` on `memory_mcp_http` produced no provider whatsoever,
    /// because the HTTP composition root never read those variables.
    ///
    /// A deployment that enables a remote provider and pins its width must get
    /// a policy at that width. This asserts the whole path — env parse,
    /// preflight, provider construction — rather than a config field, because
    /// the gap was never in the config reader.
    #[test]
    fn the_http_profile_resolves_an_embedding_policy_from_the_environment() {
        let policy = policy_for_env(&[
            ("EMBEDDINGS_ENABLED", Some("true")),
            ("EMBEDDINGS_PROVIDER", Some("openai-compatible")),
            ("EMBEDDINGS_MODEL", Some("nvidia/nemotron-3-embed-1b")),
            (
                "EMBEDDINGS_BASE_URL",
                Some("https://integrate.api.nvidia.com/v1"),
            ),
            // Pins the width so the preflight is offline and deterministic.
            ("SURREALDB_EMBEDDING_DIMENSION", Some("2048")),
            ("EMBEDDINGS_API_KEY", Some("test-key")),
        ])
        .expect("an enabled, pinned provider must resolve")
        .expect("policy must be present when EMBEDDINGS_ENABLED is true");

        assert_eq!(policy.provider_label, "openai-compatible");
        assert_eq!(
            policy.dimension, 2048,
            "the pinned dimension must reach the tenant index"
        );
        assert_eq!(policy.provider.dimension(), 2048);
        assert!(
            policy.provider.is_enabled(),
            "the deployment provider must not be the disabled one"
        );
    }

    /// Absent `EMBEDDINGS_*` means lexical-only, not a failed startup and not a
    /// fabricated policy.
    #[test]
    fn an_http_profile_without_embedding_variables_resolves_no_policy() {
        let policy = policy_for_env(&[
            ("EMBEDDINGS_ENABLED", None),
            ("EMBEDDINGS_PROVIDER", None),
            ("SURREALDB_EMBEDDING_DIMENSION", None),
        ])
        .expect("embeddings off must not fail startup");

        assert!(policy.is_none(), "no variables means no embedding policy");
    }

    /// `EMBEDDINGS_ENABLED=true` with a provider that cannot be resolved is a
    /// startup failure, not a silent downgrade: the operator asked for an
    /// environment this profile cannot provide.
    #[test]
    fn an_enabled_provider_that_cannot_be_resolved_fails_startup() {
        let error = policy_for_env(&[
            ("EMBEDDINGS_ENABLED", Some("true")),
            ("EMBEDDINGS_PROVIDER", Some("openai-compatible")),
            ("EMBEDDINGS_MODEL", Some("nvidia/nemotron-3-embed-1b")),
            ("EMBEDDINGS_BASE_URL", Some("https://127.0.0.1:1/v1")),
            // No `SURREALDB_EMBEDDING_DIMENSION`: the preflight must reach
            // an endpoint that refuses the connection.
            ("SURREALDB_EMBEDDING_DIMENSION", None),
            ("EMBEDDINGS_API_KEY", Some("test-key")),
        ])
        .expect_err("an unresolvable enabled provider must not start");

        assert!(
            error.contains("could not be resolved"),
            "the failure must name the cause: {error}"
        );
    }
}
