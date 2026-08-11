use crate::service::embedding::embedding_from_value;
use crate::service::error::MemoryError;
use crate::storage::DbClient;
use std::sync::Arc;

pub(crate) const EMBEDDING_STATE_RECORD_ID: &str = "embedding_state:fact";
pub(crate) const STORED_EMBEDDING_SAMPLE_SIZE: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingActivationMode {
    Standard,
    ForceEnabledForReembed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EmbeddingStartupDecision {
    UseConfiguredProvider,
    BootstrapReadyNamespaces {
        namespaces: Vec<String>,
        active_signature: String,
    },
    DisableSemantic {
        reason: String,
    },
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

/// Apply startup migrations to all configured namespaces.
pub(crate) async fn apply_startup_migrations(
    db_client: &Arc<dyn DbClient>,
    namespaces: &[String],
) -> Result<(), MemoryError> {
    for namespace in namespaces {
        db_client.apply_migrations(namespace).await?;
    }
    Ok(())
}

pub(crate) async fn load_embedding_states(
    db_client: &Arc<dyn DbClient>,
    namespaces: &[String],
) -> Result<std::collections::HashMap<String, Option<serde_json::Value>>, MemoryError> {
    let mut states = std::collections::HashMap::new();

    for namespace in namespaces {
        states.insert(
            namespace.clone(),
            db_client
                .select_one(EMBEDDING_STATE_RECORD_ID, namespace)
                .await?,
        );
    }

    Ok(states)
}

pub(crate) async fn count_facts_per_namespace(
    db_client: &Arc<dyn DbClient>,
    namespaces: &[String],
) -> Result<std::collections::HashMap<String, usize>, MemoryError> {
    let mut counts = std::collections::HashMap::new();

    for namespace in namespaces {
        counts.insert(
            namespace.clone(),
            db_client.select_table("fact", namespace).await?.len(),
        );
    }

    Ok(counts)
}

pub(crate) async fn sample_stored_embedding_dimensions(
    db_client: &Arc<dyn DbClient>,
    namespaces: &[String],
    sample_size: usize,
) -> Result<std::collections::HashMap<String, Vec<usize>>, MemoryError> {
    let mut sampled = std::collections::HashMap::new();

    for namespace in namespaces {
        let dimensions = db_client
            .select_table("fact", namespace)
            .await?
            .into_iter()
            .filter_map(|record| record.get("embedding").and_then(embedding_from_value))
            .map(|embedding| embedding.len())
            .take(sample_size)
            .collect::<Vec<_>>();
        sampled.insert(namespace.clone(), dimensions);
    }

    Ok(sampled)
}

pub(crate) async fn write_bootstrap_ready_states(
    db_client: &Arc<dyn DbClient>,
    namespaces: &[String],
    active_signature: &str,
    provider: &str,
    model: Option<&str>,
    dimension: usize,
) -> Result<(), MemoryError> {
    let updated_at = chrono::Utc::now().to_rfc3339();

    for namespace in namespaces {
        let payload = serde_json::json!({
            "status": "ready",
            "active_signature": active_signature,
            "provider": provider,
            "model": model,
            "dimension": dimension,
            "updated_at": updated_at,
        });

        if db_client
            .select_one(EMBEDDING_STATE_RECORD_ID, namespace)
            .await?
            .is_some()
        {
            db_client
                .update(EMBEDDING_STATE_RECORD_ID, payload, namespace)
                .await?;
        } else {
            db_client
                .create(EMBEDDING_STATE_RECORD_ID, payload, namespace)
                .await?;
        }
    }

    Ok(())
}

pub(crate) fn decide_embedding_startup(
    configured_namespaces: &[String],
    namespace_states: &std::collections::HashMap<String, Option<serde_json::Value>>,
    target_signature: &str,
    sample_dimensions: &std::collections::HashMap<String, Vec<usize>>,
    fact_counts: &std::collections::HashMap<String, usize>,
    target_dimension: usize,
) -> EmbeddingStartupDecision {
    let mut namespaces_to_bootstrap = Vec::new();

    for namespace in configured_namespaces {
        match namespace_states
            .get(namespace)
            .and_then(|value| value.as_ref())
        {
            Some(state)
                if state.get("status").and_then(serde_json::Value::as_str)
                    == Some("rebuilding")
                    || state.get("status").and_then(serde_json::Value::as_str)
                        == Some("failed") =>
            {
                return EmbeddingStartupDecision::DisableSemantic {
                    reason: format!(
                        "embedding maintenance is incomplete in namespace `{namespace}`"
                    ),
                };
            }
            Some(state)
                if state.get("status").and_then(serde_json::Value::as_str) == Some("ready")
                    && state
                        .get("active_signature")
                        .and_then(serde_json::Value::as_str)
                        == Some(target_signature) => {}
            Some(state)
                if state.get("status").and_then(serde_json::Value::as_str) == Some("ready") =>
            {
                return EmbeddingStartupDecision::DisableSemantic {
                    reason: format!(
                        "configured embedding signature differs from persisted state in namespace `{namespace}`"
                    ),
                };
            }
            None => {
                let fact_count = *fact_counts.get(namespace).unwrap_or(&0);
                if fact_count == 0 {
                    namespaces_to_bootstrap.push(namespace.clone());
                    continue;
                }

                let sampled = sample_dimensions
                    .get(namespace)
                    .cloned()
                    .unwrap_or_default();
                if !sampled.is_empty()
                    && sampled
                        .iter()
                        .all(|dimension| *dimension == target_dimension)
                {
                    namespaces_to_bootstrap.push(namespace.clone());
                    continue;
                }

                return EmbeddingStartupDecision::DisableSemantic {
                    reason: format!(
                        "legacy embeddings in namespace `{namespace}` require reembed before semantic search can resume"
                    ),
                };
            }
            Some(_) => {
                return EmbeddingStartupDecision::DisableSemantic {
                    reason: format!(
                        "embedding state in namespace `{namespace}` is invalid or incomplete"
                    ),
                };
            }
        }
    }

    if namespaces_to_bootstrap.is_empty() {
        EmbeddingStartupDecision::UseConfiguredProvider
    } else {
        EmbeddingStartupDecision::BootstrapReadyNamespaces {
            namespaces: namespaces_to_bootstrap,
            active_signature: target_signature.to_string(),
        }
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
    namespaces: &[String],
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
                startup_logger.log(event, crate::logging::LogLevel::Warn);
                None
            }
        }
    } else {
        None
    };

    let decision = if let Some(target) = target.as_ref() {
        let namespace_states = load_embedding_states(db_client, namespaces).await?;
        let fact_counts = count_facts_per_namespace(db_client, namespaces).await?;
        let sample_dimensions =
            sample_stored_embedding_dimensions(db_client, namespaces, STORED_EMBEDDING_SAMPLE_SIZE)
                .await?;

        let mut event = std::collections::HashMap::new();
        event.insert(
            "op".to_string(),
            serde_json::json!("embedding.startup_state_loaded"),
        );
        event.insert("namespaces".to_string(), serde_json::json!(namespaces));
        event.insert(
            "state_count".to_string(),
            serde_json::json!(namespace_states.len()),
        );
        event.insert(
            "fact_counts".to_string(),
            serde_json::json!(fact_counts.clone()),
        );
        startup_logger.log(event, crate::logging::LogLevel::Debug);

        decide_embedding_startup(
            namespaces,
            &namespace_states,
            &target.signature,
            &sample_dimensions,
            &fact_counts,
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
    decision_event.insert("namespaces".to_string(), serde_json::json!(namespaces));
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
    async fn apply_startup_migrations_runs_for_every_namespace() {
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
        let namespaces = vec![
            "org".to_string(),
            "personal".to_string(),
            "private".to_string(),
        ];

        apply_startup_migrations(&db_client_dyn, &namespaces)
            .await
            .expect("startup migrations");

        assert_eq!(db_client.apply_count.load(Ordering::SeqCst), 3);
        assert_eq!(
            db_client.calls.safe_lock().clone(),
            vec![
                "org".to_string(),
                "personal".to_string(),
                "private".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn resolve_embedding_startup_does_not_activate_unprobed_remote_target() {
        let namespace = "org".to_string();
        let db_client = Arc::new(
            crate::storage::SurrealDbClient::connect_in_memory("memory", &namespace, "warn")
                .await
                .expect("in-memory database should connect"),
        ) as Arc<dyn DbClient>;
        apply_startup_migrations(&db_client, std::slice::from_ref(&namespace))
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

        let (decision, target) = resolve_embedding_startup(
            &config,
            &db_client,
            std::slice::from_ref(&namespace),
            "/tmp",
            &logger,
        )
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

    #[test]
    fn decide_embedding_startup_disables_semantic_when_any_namespace_is_rebuilding() {
        let states = std::collections::HashMap::from([
            (
                "org".to_string(),
                Some(serde_json::json!({"status": "ready", "active_signature": "embsig:ok"})),
            ),
            (
                "personal".to_string(),
                Some(serde_json::json!({"status": "rebuilding", "active_signature": "embsig:ok"})),
            ),
        ]);

        let decision = decide_embedding_startup(
            &["org".to_string(), "personal".to_string()],
            &states,
            "embsig:ok",
            &std::collections::HashMap::new(),
            &std::collections::HashMap::new(),
            384,
        );

        assert!(matches!(
            decision,
            EmbeddingStartupDecision::DisableSemantic { .. }
        ));
    }

    #[test]
    fn decide_embedding_startup_bootstraps_legacy_ready_when_dimensions_match() {
        let states = std::collections::HashMap::from([("org".to_string(), None)]);
        let decision = decide_embedding_startup(
            &["org".to_string()],
            &states,
            "embsig:new",
            &std::collections::HashMap::from([("org".to_string(), vec![384usize])]),
            &std::collections::HashMap::from([("org".to_string(), 12usize)]),
            384,
        );

        assert!(matches!(
            decision,
            EmbeddingStartupDecision::BootstrapReadyNamespaces { .. }
        ));
    }

    #[test]
    fn decide_embedding_startup_bootstraps_missing_namespace_without_ignoring_existing_ready_state()
    {
        let states = std::collections::HashMap::from([
            (
                "org".to_string(),
                Some(serde_json::json!({"status": "ready", "active_signature": "embsig:new"})),
            ),
            ("personal".to_string(), None),
        ]);

        let decision = decide_embedding_startup(
            &["org".to_string(), "personal".to_string()],
            &states,
            "embsig:new",
            &std::collections::HashMap::from([("personal".to_string(), vec![384usize])]),
            &std::collections::HashMap::from([
                ("org".to_string(), 10usize),
                ("personal".to_string(), 2usize),
            ]),
            384,
        );

        assert!(matches!(
            decision,
            EmbeddingStartupDecision::BootstrapReadyNamespaces { ref namespaces, .. }
                if namespaces == &vec!["personal".to_string()]
        ));
    }
}
