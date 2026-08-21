use crate::service::embedding::embedding_from_value;
use crate::service::error::MemoryError;
use crate::storage::{BoundDbClient, DbClient};
use std::sync::Arc;

pub(crate) use crate::storage::EMBEDDING_STATE_RECORD_ID;
pub(crate) const STORED_EMBEDDING_SAMPLE_SIZE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingActivationMode {
    Standard,
    ForceEnabledForReembed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EmbeddingStartupDecision {
    UseConfiguredProvider,
    ResumePendingBackfill { active_signature: String },
    RecoverMissingEmbeddings { target_signature: String },
    BootstrapReadyNamespace { active_signature: String },
    DisableSemantic { reason: String },
}

/// Build a startup versions event payload used for diagnostic logging.
pub(crate) fn build_startup_versions_event(
    client_version: &str,
    server_version: Option<&str>,
) -> std::collections::HashMap<String, serde_json::Value> {
    let mut m = std::collections::HashMap::new();
    m.insert("op".to_string(), serde_json::json!("startup.versions"));
    m.insert(
        "client_version".to_string(),
        serde_json::json!(client_version),
    );
    if let Some(sv) = server_version {
        m.insert(
            "surrealdb_server_version".to_string(),
            serde_json::json!(sv),
        );
    }
    m
}

/// Apply startup migrations only to the process-bound Active Namespace.
pub(crate) async fn apply_startup_migrations(
    db_client: &Arc<dyn DbClient>,
    active_namespace: &str,
) -> Result<(), MemoryError> {
    BoundDbClient::new(db_client.clone(), active_namespace)
        .apply_migrations()
        .await
}

pub(crate) async fn load_embedding_state(
    db: &BoundDbClient,
) -> Result<Option<serde_json::Value>, MemoryError> {
    db.select_one(EMBEDDING_STATE_RECORD_ID).await
}

async fn count_facts(db: &BoundDbClient) -> Result<usize, MemoryError> {
    Ok(db.select_table("fact").await?.len())
}

async fn count_facts_missing_embeddings(db: &BoundDbClient) -> Result<usize, MemoryError> {
    let rows = db
        .query_rows(
            "SELECT count() AS count FROM fact WHERE embedding IS NONE GROUP ALL",
            None,
        )
        .await?;
    Ok(rows
        .first()
        .and_then(|row| row.get("count"))
        .and_then(serde_json::Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or(0))
}

async fn sample_stored_embedding_dimensions(
    db: &BoundDbClient,
    sample_size: usize,
) -> Result<Vec<usize>, MemoryError> {
    Ok(db
        .select_table("fact")
        .await?
        .into_iter()
        .filter_map(|record| record.get("embedding").and_then(embedding_from_value))
        .map(|embedding| embedding.len())
        .take(sample_size)
        .collect())
}

pub(crate) async fn write_bootstrap_ready_state(
    db: &BoundDbClient,
    active_signature: &str,
    provider: &str,
    model: Option<&str>,
    dimension: usize,
    backfill_pending: bool,
) -> Result<(), MemoryError> {
    use crate::storage::{EmbeddingStateStatus, EmbeddingStateStoreClient};

    let status = if backfill_pending {
        EmbeddingStateStatus::BackfillPending
    } else {
        EmbeddingStateStatus::Ready
    };
    EmbeddingStateStoreClient::from_bound(db.clone())
        .upsert_bootstrap_state(status, active_signature, provider, model, dimension)
        .await
}

pub(crate) fn decide_embedding_startup(
    active_namespace: &str,
    namespace_state: Option<&serde_json::Value>,
    sample_dimensions: &[usize],
    fact_count: usize,
    missing_embedding_count: usize,
    target_signature: &str,
    target_dimension: usize,
) -> EmbeddingStartupDecision {
    match namespace_state {
        Some(state)
            if matches!(
                state.get("status").and_then(serde_json::Value::as_str),
                Some("rebuilding") | Some("failed")
            ) =>
        {
            EmbeddingStartupDecision::DisableSemantic {
                reason: format!(
                    "embedding maintenance is incomplete in namespace `{active_namespace}`"
                ),
            }
        }
        Some(state)
            if matches!(
                state.get("status").and_then(serde_json::Value::as_str),
                Some("ready") | Some("backfill_pending")
            ) && state
                .get("active_signature")
                .and_then(serde_json::Value::as_str)
                == Some(target_signature) =>
        {
            if state.get("status").and_then(serde_json::Value::as_str) == Some("backfill_pending")
                || missing_embedding_count > 0
            {
                EmbeddingStartupDecision::ResumePendingBackfill {
                    active_signature: target_signature.to_string(),
                }
            } else {
                EmbeddingStartupDecision::UseConfiguredProvider
            }
        }
        Some(state) if state.get("status").and_then(serde_json::Value::as_str) == Some("ready") => {
            if missing_embedding_count > 0 {
                EmbeddingStartupDecision::RecoverMissingEmbeddings {
                    target_signature: target_signature.to_string(),
                }
            } else {
                EmbeddingStartupDecision::DisableSemantic {
                    reason: format!(
                        "configured embedding signature differs from persisted state in namespace `{active_namespace}`"
                    ),
                }
            }
        }
        None if fact_count == 0
            || (!sample_dimensions.is_empty()
                && sample_dimensions
                    .iter()
                    .all(|dimension| *dimension == target_dimension)) =>
        {
            EmbeddingStartupDecision::BootstrapReadyNamespace {
                active_signature: target_signature.to_string(),
            }
        }
        None => EmbeddingStartupDecision::DisableSemantic {
            reason: format!(
                "legacy embeddings in namespace `{active_namespace}` require reembed before semantic search can resume"
            ),
        },
        Some(_) => EmbeddingStartupDecision::DisableSemantic {
            reason: format!(
                "embedding state in namespace `{active_namespace}` is invalid or incomplete"
            ),
        },
    }
}

/// Resolves embedding startup target and decision.
///
/// Combines: target preflight (dimension detection) → namespace state
/// loading → fact counts → sample dimensions → startup decision.
/// Returns `(decision, target)` where `target` is `None` when preflight
/// fails or embedding is disabled.
pub(crate) async fn resolve_embedding_startup(
    config: &crate::config::EmbeddingConfig,
    db_client: &Arc<dyn DbClient>,
    active_namespace: &str,
    data_dir: &str,
    startup_logger: &crate::logging::StdoutLogger,
) -> Result<
    (
        EmbeddingStartupDecision,
        Option<crate::service::embedding::ResolvedEmbeddingTarget>,
    ),
    MemoryError,
> {
    use crate::service::embedding::resolve_embedding_target_identity;

    let target = if config.is_enabled() {
        match resolve_embedding_target_identity(config, data_dir).await {
            Ok(target) => Some(target),
            Err(err) => {
                let mut event = std::collections::HashMap::new();
                event.insert(
                    "op".to_string(),
                    serde_json::json!("embedding.preflight_failed"),
                );
                event.insert("error".to_string(), serde_json::json!(err.to_string()));
                event.insert(
                    "provider".to_string(),
                    serde_json::json!(config.provider_label()),
                );
                event.insert(
                    "endpoint".to_string(),
                    serde_json::json!(
                        config
                            .base_url
                            .as_deref()
                            .map(crate::service::embedding::embedding_endpoint_for_log)
                    ),
                );
                event.insert("model".to_string(), serde_json::json!(config.model.clone()));
                startup_logger.log(event, crate::logging::LogLevel::Warn);
                None
            }
        }
    } else {
        None
    };

    let decision = if let Some(target) = target.as_ref() {
        let db = BoundDbClient::new(db_client.clone(), active_namespace);
        let namespace_state = load_embedding_state(&db).await?;
        let fact_count = count_facts(&db).await?;
        let sample_dimensions =
            sample_stored_embedding_dimensions(&db, STORED_EMBEDDING_SAMPLE_SIZE).await?;
        let missing_embedding_count = count_facts_missing_embeddings(&db).await?;

        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            serde_json::json!("embedding.startup_state_loaded"),
        );
        event.insert("namespace".to_string(), serde_json::json!(active_namespace));
        event.insert(
            "state_present".to_string(),
            serde_json::json!(namespace_state.is_some()),
        );
        event.insert("fact_count".to_string(), serde_json::json!(fact_count));
        event.insert(
            "missing_embedding_count".to_string(),
            serde_json::json!(missing_embedding_count),
        );
        startup_logger.log(event, crate::logging::LogLevel::Debug);

        decide_embedding_startup(
            active_namespace,
            namespace_state.as_ref(),
            &sample_dimensions,
            fact_count,
            missing_embedding_count,
            &target.signature,
            target.dimension,
        )
    } else if config.is_enabled() {
        EmbeddingStartupDecision::DisableSemantic {
            reason: "embedding target preflight failed".to_string(),
        }
    } else {
        EmbeddingStartupDecision::UseConfiguredProvider
    };

    let mut decision_event = std::collections::HashMap::new();
    decision_event.insert(
        "op".to_string(),
        serde_json::json!("embedding.startup_decision"),
    );
    decision_event.insert(
        "decision".to_string(),
        serde_json::json!(format!("{:?}", decision)),
    );
    decision_event.insert("namespace".to_string(), serde_json::json!(active_namespace));
    decision_event.insert(
        "target_signature".to_string(),
        serde_json::json!(target.as_ref().map(|value| value.signature.clone())),
    );
    startup_logger.log(decision_event, crate::logging::LogLevel::Info);

    Ok((decision, target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::util::rate_limiter::SafeMutex;
    use serde_json::Value;

    #[test]
    fn build_startup_versions_event_includes_both_versions() {
        let evt = build_startup_versions_event("0.1.0", Some("SurrealDB 3.0.0"));
        assert_eq!(evt.get("op").unwrap().as_str(), Some("startup.versions"));
        assert_eq!(evt.get("client_version").unwrap().as_str(), Some("0.1.0"));
        assert_eq!(
            evt.get("surrealdb_server_version").unwrap().as_str(),
            Some("SurrealDB 3.0.0")
        );
    }

    #[test]
    fn build_startup_versions_event_omits_server_when_none() {
        let evt = build_startup_versions_event("0.1.0", None);
        assert_eq!(evt.get("op").unwrap().as_str(), Some("startup.versions"));
        assert_eq!(evt.get("client_version").unwrap().as_str(), Some("0.1.0"));
        assert!(!evt.contains_key("surrealdb_server_version"));
    }

    #[tokio::test]
    async fn apply_startup_migrations_uses_only_the_active_namespace() {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct StartupMigrationDbClient {
            calls: Arc<Mutex<Vec<String>>>,
            apply_count: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl DbClient for StartupMigrationDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }
            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }
            #[allow(clippy::too_many_arguments)]
            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }
            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }
            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }
            async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
                self.apply_count.fetch_add(1, Ordering::SeqCst);
                self.calls.safe_lock().push(namespace.to_string());
                Ok(())
            }
        }

        let db_client = Arc::new(StartupMigrationDbClient {
            calls: Arc::new(Mutex::new(Vec::new())),
            apply_count: AtomicUsize::new(0),
        });
        let db_client_dyn: Arc<dyn DbClient> = db_client.clone();

        apply_startup_migrations(&db_client_dyn, "main")
            .await
            .expect("startup migrations");

        assert_eq!(db_client.apply_count.load(Ordering::SeqCst), 1);
        assert_eq!(db_client.calls.safe_lock().as_slice(), ["main"]);
    }

    #[tokio::test]
    async fn resolve_embedding_startup_does_not_activate_unprobed_remote_target() {
        let namespace = "org".to_string();
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory("memory", &namespace, "warn")
                .await
                .expect("in-memory database should connect"),
        ) as Arc<dyn DbClient>;
        apply_startup_migrations(&db_client, &namespace)
            .await
            .expect("startup migrations should apply");

        let config = crate::config::EmbeddingConfig {
            provider: crate::config::EmbeddingProviderKind::OpenAiCompatible,
            base_url: Some("https =//invalid.example/v1/embeddings".to_string()),
            model: Some("test-model".to_string()),
            api_key: None,
            ..crate::config::EmbeddingConfig::default()
        };
        let logger = crate::logging::StdoutLogger::new("warn");

        let (decision, target) =
            resolve_embedding_startup(&config, &db_client, &namespace, "/tmp", &logger)
                .await
                .expect("provider probe failure should degrade semantic startup");

        assert!(
            target.is_none(),
            "an embedding target must not be synthesized when provider probing fails"
        );
        assert!(matches!(
            decision,
            EmbeddingStartupDecision::DisableSemantic { .. }
        ));
    }

    #[tokio::test]
    async fn resolve_embedding_startup_degrades_fast_when_remote_provider_unreachable() {
        // Offline startup regression: a remote provider without an explicit
        // dimension override must fail its probe within the bounded probe
        // budget and degrade to lexical/graph-only retrieval, instead of
        // stalling startup for the full runtime retry budget (~100s).
        let namespace = "org".to_string();
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory("memory", &namespace, "warn")
                .await
                .expect("in-memory database should connect"),
        ) as Arc<dyn DbClient>;
        apply_startup_migrations(&db_client, &namespace)
            .await
            .expect("startup migrations should apply");

        let config = crate::config::EmbeddingConfig {
            provider: crate::config::EmbeddingProviderKind::OpenAiCompatible,
            // TEST-NET-1: unreachable by design, and connect attempts hang
            // until timeout (unlike connection-refused loopback ports).
            base_url: Some("http://192.0.2.1:9999/v1".to_string()),
            model: Some("test-model".to_string()),
            api_key: None,
            timeout_secs: 15,
            ..crate::config::EmbeddingConfig::default()
        };
        let logger = crate::logging::StdoutLogger::new("warn");

        let started = std::time::Instant::now();
        let (decision, target) = tokio::time::timeout(
            std::time::Duration::from_secs(12),
            resolve_embedding_startup(&config, &db_client, &namespace, "/tmp", &logger),
        )
        .await
        .expect("startup must degrade within the probe budget, not the retry budget")
        .expect("probe failure should degrade semantic startup");

        assert!(target.is_none());
        assert!(matches!(
            decision,
            EmbeddingStartupDecision::DisableSemantic { .. }
        ));
        assert!(
            started.elapsed() < std::time::Duration::from_secs(12),
            "startup took {:?}, expected to degrade within the probe budget",
            started.elapsed()
        );
    }

    #[tokio::test]
    async fn resolve_embedding_startup_resolves_offline_with_explicit_dimension_override() {
        // Offline startup with an explicit dimension override must not touch
        // the network at all: the target resolves from configuration and the
        // empty namespace bootstraps to `ready` for that signature.
        let namespace = "org".to_string();
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory("memory", &namespace, "warn")
                .await
                .expect("in-memory database should connect"),
        ) as Arc<dyn DbClient>;
        apply_startup_migrations(&db_client, &namespace)
            .await
            .expect("startup migrations should apply");

        let config = crate::config::EmbeddingConfig {
            provider: crate::config::EmbeddingProviderKind::OpenAiCompatible,
            base_url: Some("http://192.0.2.1:9999/v1".to_string()),
            model: Some("test-model".to_string()),
            api_key: None,
            dimension_override: Some(1536),
            timeout_secs: 15,
            ..crate::config::EmbeddingConfig::default()
        };
        let logger = crate::logging::StdoutLogger::new("warn");

        let (decision, target) = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            resolve_embedding_startup(&config, &db_client, &namespace, "/tmp", &logger),
        )
        .await
        .expect("override must resolve the target without network access")
        .expect("startup should resolve an embedding target from the override");

        let target = target.expect("target resolved from explicit dimension override");
        assert_eq!(target.dimension, 1536);
        assert!(matches!(
            decision,
            EmbeddingStartupDecision::BootstrapReadyNamespace { .. }
        ));
    }

    #[test]
    fn decide_embedding_startup_disables_semantic_when_active_namespace_is_rebuilding() {
        let state = serde_json::json!({"status": "rebuilding", "active_signature": "embsig:ok"});
        let decision = decide_embedding_startup("main", Some(&state), &[], 10, 0, "embsig:ok", 384);
        assert!(matches!(
            decision,
            EmbeddingStartupDecision::DisableSemantic { .. }
        ));
    }

    #[test]
    fn decide_embedding_startup_resumes_pending_backfill_for_matching_ready_state() {
        let state = serde_json::json!({
            "status": "backfill_pending",
            "active_signature": "embsig:ok",
        });
        let decision = decide_embedding_startup("main", Some(&state), &[], 12, 0, "embsig:ok", 384);
        assert!(matches!(
            decision,
            EmbeddingStartupDecision::ResumePendingBackfill { .. }
        ));
    }

    #[test]
    fn decide_embedding_startup_recovers_missing_embeddings_before_reembed_on_signature_change() {
        let state = serde_json::json!({
            "status": "ready",
            "active_signature": "embsig:old",
        });
        let decision =
            decide_embedding_startup("main", Some(&state), &[], 12, 3, "embsig:new", 384);
        assert!(matches!(
            decision,
            EmbeddingStartupDecision::RecoverMissingEmbeddings { .. }
        ));
    }

    #[test]
    fn decide_embedding_startup_resumes_legacy_ready_state_with_missing_embeddings() {
        let state = serde_json::json!({
            "status": "ready",
            "active_signature": "embsig:ok",
        });
        let decision = decide_embedding_startup("main", Some(&state), &[], 12, 3, "embsig:ok", 384);
        assert!(matches!(
            decision,
            EmbeddingStartupDecision::ResumePendingBackfill { .. }
        ));
    }

    #[test]
    fn decide_embedding_startup_bootstraps_active_namespace_when_dimensions_match() {
        let decision =
            decide_embedding_startup("main", None, &[384usize], 12, 0, "embsig:new", 384);
        assert!(matches!(
            decision,
            EmbeddingStartupDecision::BootstrapReadyNamespace { .. }
        ));
    }

    #[test]
    fn decide_embedding_startup_does_not_bootstrap_when_legacy_dimensions_are_unknown() {
        let decision = decide_embedding_startup("main", None, &[], 12, 12, "embsig:new", 384);
        assert!(matches!(
            decision,
            EmbeddingStartupDecision::DisableSemantic { .. }
        ));
    }
}
