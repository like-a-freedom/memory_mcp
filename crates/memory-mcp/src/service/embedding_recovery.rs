//! In-process recovery of remote embeddings after a degraded startup.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::config::{
    DEFAULT_EMBEDDING_RECOVERY_BACKOFF_SECS, DEFAULT_EMBEDDING_RECOVERY_MAX_BACKOFF_SECS,
    EmbeddingConfig, build_embedding_signature,
};
use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::service::cache::invalidate_cache;
use crate::service::embedding::{
    EmbeddingProvider, ResolvedEmbeddingTarget, create_embedding_provider_with_dimension,
    probe_remote_embedding_dimension,
};
use crate::service::fact::FactService;
use crate::service::is_remote_embedding_provider;
use crate::service::startup::{
    EmbeddingActivationMode, EmbeddingStartupDecision, load_embedding_state,
    write_bootstrap_ready_state,
};
use crate::storage::{BoundDbClient, embedding_backfill_store::EmbeddingBackfillStoreClient};
use tokio_util::sync::CancellationToken;

#[async_trait]
pub(crate) trait EmbeddingRecoveryBackend: Send + Sync {
    async fn probe_dimension(&self) -> Result<usize, MemoryError>;

    async fn create_provider(
        &self,
        dimension: usize,
    ) -> Result<Arc<dyn EmbeddingProvider>, MemoryError>;
}

pub(crate) struct ConfiguredEmbeddingRecoveryBackend {
    config: EmbeddingConfig,
    data_dir: String,
}

pub(crate) fn should_spawn_embedding_recovery(
    mode: EmbeddingActivationMode,
    decision: &EmbeddingStartupDecision,
    config: &EmbeddingConfig,
) -> bool {
    matches!(mode, EmbeddingActivationMode::Standard)
        && config.auto_recovery
        && is_remote_embedding_provider(config.provider_label())
        && (matches!(
            decision,
            EmbeddingStartupDecision::DisableSemantic { reason }
                if reason == "embedding target preflight failed"
        ) || matches!(
            decision,
            EmbeddingStartupDecision::ResumePendingBackfill { .. }
                | EmbeddingStartupDecision::RecoverMissingEmbeddings { .. }
        ))
}

impl ConfiguredEmbeddingRecoveryBackend {
    pub(crate) fn new(config: EmbeddingConfig, data_dir: String) -> Self {
        Self { config, data_dir }
    }
}

#[async_trait]
impl EmbeddingRecoveryBackend for ConfiguredEmbeddingRecoveryBackend {
    async fn probe_dimension(&self) -> Result<usize, MemoryError> {
        probe_remote_embedding_dimension(&self.config).await
    }

    async fn create_provider(
        &self,
        dimension: usize,
    ) -> Result<Arc<dyn EmbeddingProvider>, MemoryError> {
        create_embedding_provider_with_dimension(&self.config, &self.data_dir, dimension).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryDecision {
    FullRecovery(ResolvedEmbeddingTarget),
    EnableForNewFacts(ResolvedEmbeddingTarget),
    DimensionMismatch {
        index_dimension: usize,
        probed_dimension: usize,
    },
}

pub(crate) fn choose_recovery_decision(
    config: &EmbeddingConfig,
    index_dimension: usize,
    stored_signature: Option<&str>,
    probed_dimension: usize,
) -> RecoveryDecision {
    let target = ResolvedEmbeddingTarget {
        provider_label: config.provider_label(),
        model: config.model.clone(),
        dimension: probed_dimension,
        signature: build_embedding_signature(
            config.provider_label(),
            config.model.as_deref(),
            config.base_url.as_deref(),
            probed_dimension,
        ),
    };

    if probed_dimension != index_dimension {
        RecoveryDecision::DimensionMismatch {
            index_dimension,
            probed_dimension,
        }
    } else if stored_signature.is_some_and(|signature| signature != target.signature) {
        RecoveryDecision::EnableForNewFacts(target)
    } else {
        RecoveryDecision::FullRecovery(target)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct RecoveryWorkerSettings {
    pub(crate) initial_probe_delay: Duration,
    pub(crate) backoff_base: Duration,
    pub(crate) backoff_cap: Duration,
    pub(crate) warn_demote_after: u32,
    pub(crate) batch_size: i32,
}

impl RecoveryWorkerSettings {
    pub(crate) fn production(interval_secs: u64) -> Self {
        Self {
            initial_probe_delay: Duration::from_secs(interval_secs),
            backoff_base: Duration::from_secs(DEFAULT_EMBEDDING_RECOVERY_BACKOFF_SECS),
            backoff_cap: Duration::from_secs(DEFAULT_EMBEDDING_RECOVERY_MAX_BACKOFF_SECS),
            warn_demote_after: 3,
            batch_size: 100,
        }
    }
}

#[must_use]
pub(crate) fn recovery_backoff(failures: u32) -> Duration {
    let multiplier = 1u64 << failures.saturating_sub(1).min(5);
    Duration::from_secs(
        DEFAULT_EMBEDDING_RECOVERY_BACKOFF_SECS
            .saturating_mul(multiplier)
            .min(DEFAULT_EMBEDDING_RECOVERY_MAX_BACKOFF_SECS),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BackfillOutcome {
    Complete { processed: usize },
}

fn required_fact_string(record: &Value, field: &str) -> Result<String, MemoryError> {
    record
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| MemoryError::Validation(format!("missing fact field `{field}`")))
}

fn embedding_fields_for_backfill(
    provider: &dyn EmbeddingProvider,
    embedding: Vec<f64>,
    signature: &str,
    model: Option<&str>,
    dimension: usize,
) -> Result<Value, MemoryError> {
    if embedding.len() != dimension {
        return Err(MemoryError::Validation(format!(
            "embedding dimension mismatch: provider returned {}, expected {dimension}",
            embedding.len()
        )));
    }

    let mut fields = serde_json::Map::from_iter([
        ("embedding".to_string(), json!(embedding)),
        (
            "embedding_provider".to_string(),
            json!(provider.provider_name()),
        ),
        ("embedding_dimension".to_string(), json!(dimension)),
        ("embedding_signature".to_string(), json!(signature)),
        (
            "embedding_updated_at".to_string(),
            json!(crate::service::normalize_dt(crate::service::now())),
        ),
    ]);
    if let Some(model) = model {
        fields.insert("embedding_model".to_string(), json!(model));
    }
    Ok(Value::Object(fields))
}

pub(crate) async fn run_backfill(
    service: &crate::service::MemoryService,
    provider: Arc<dyn EmbeddingProvider>,
    signature: &str,
    model: Option<&str>,
    dimension: usize,
    batch_size: i32,
) -> Result<BackfillOutcome, MemoryError> {
    let store = EmbeddingBackfillStoreClient::new(
        service.db_client.clone(),
        service.active_namespace.clone(),
    );
    let mut cursor: Option<String> = None;
    let mut processed = 0usize;
    let total = store.count_facts_missing_embeddings().await?;
    log_recovery_event(
        &service.logger,
        "embedding.backfill_started",
        LogLevel::Info,
        [("total_missing", json!(total))],
    );

    loop {
        let batch = store
            .select_facts_missing_embeddings(cursor.as_deref(), batch_size)
            .await?;
        if batch.is_empty() {
            return Ok(BackfillOutcome::Complete { processed });
        }

        let batch_size = batch.len();
        for fact in batch {
            let fact_id = required_fact_string(&fact, "fact_id")?;
            let fact_type = required_fact_string(&fact, "fact_type")?;
            let content = required_fact_string(&fact, "content")?;
            let quote = required_fact_string(&fact, "quote")?;
            let input = FactService::build_fact_embedding_input(&fact_type, &content, &quote);
            let embedding = provider.embed(&input).await?;
            let fields = embedding_fields_for_backfill(
                provider.as_ref(),
                embedding,
                signature,
                model,
                dimension,
            )?;
            store.update_embedding_fields(&fact_id, fields).await?;
            crate::service::invalidate_cache(&service.context_cache).await;
            cursor = Some(fact_id);
            processed += 1;
        }
        let remaining = store.count_facts_missing_embeddings().await?;
        log_recovery_event(
            &service.logger,
            "embedding.backfill_progress",
            LogLevel::Info,
            [
                ("batch_size", json!(batch_size)),
                ("processed", json!(processed)),
                ("remaining", json!(remaining)),
            ],
        );
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RecoveryCycleOutcome {
    Completed,
}

async fn install_recovery_provider(
    service: &MemoryService,
    db: &BoundDbClient,
    target: &ResolvedEmbeddingTarget,
    provider: Arc<dyn EmbeddingProvider>,
    persist_backfill_pending: bool,
) -> Result<(), MemoryError> {
    if persist_backfill_pending {
        write_bootstrap_ready_state(
            db,
            &target.signature,
            target.provider_label,
            target.model.as_deref(),
            target.dimension,
            true,
        )
        .await?;
    }
    service.replace_embedding_runtime_state(
        crate::service::embedding_runtime::EmbeddingRuntimeState::new(
            provider,
            Some(target.signature.clone()),
            target.model.clone(),
            Some(target.dimension),
        ),
    );
    invalidate_cache(&service.context_cache).await;
    Ok(())
}

async fn backfill_and_mark_ready(
    service: &MemoryService,
    db: &BoundDbClient,
    provider: Arc<dyn EmbeddingProvider>,
    target: &ResolvedEmbeddingTarget,
    batch_size: i32,
) -> Result<usize, MemoryError> {
    let processed = match run_backfill(
        service,
        provider,
        &target.signature,
        target.model.as_deref(),
        target.dimension,
        batch_size,
    )
    .await?
    {
        BackfillOutcome::Complete { processed } => processed,
    };
    write_bootstrap_ready_state(
        db,
        &target.signature,
        target.provider_label,
        target.model.as_deref(),
        target.dimension,
        false,
    )
    .await?;
    Ok(processed)
}

fn log_recovery_event(
    logger: &StdoutLogger,
    op: &str,
    level: LogLevel,
    fields: impl IntoIterator<Item = (&'static str, serde_json::Value)>,
) {
    let mut event = HashMap::new();
    event.insert("op".to_string(), serde_json::json!(op));
    for (key, value) in fields {
        event.insert(key.to_string(), value);
    }
    logger.log(event, level);
}

fn recovery_backoff_with_settings(failures: u32, base: Duration, cap: Duration) -> Duration {
    let multiplier = 1u32 << failures.saturating_sub(1).min(31);
    base.checked_mul(multiplier).unwrap_or(cap).min(cap)
}

async fn run_recovery_cycle(
    service: &MemoryService,
    config: &EmbeddingConfig,
    backend: &dyn EmbeddingRecoveryBackend,
    settings: &RecoveryWorkerSettings,
) -> Result<RecoveryCycleOutcome, MemoryError> {
    let db = BoundDbClient::new(service.db_client.clone(), service.active_namespace.clone());
    let persisted_state = load_embedding_state(&db).await?;
    let index_dimension = persisted_state
        .as_ref()
        .and_then(|record| record.get("dimension"))
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
        .unwrap_or_else(|| config.fallback_dimension());
    let stored_signature = persisted_state
        .as_ref()
        .and_then(|record| record.get("active_signature"))
        .and_then(Value::as_str);
    let probed_dimension = backend.probe_dimension().await?;

    match choose_recovery_decision(config, index_dimension, stored_signature, probed_dimension) {
        RecoveryDecision::FullRecovery(target) => {
            let provider = backend.create_provider(target.dimension).await?;
            install_recovery_provider(service, &db, &target, provider.clone(), true).await?;
            log_recovery_event(
                &service.logger,
                "embedding.recovered",
                LogLevel::Info,
                [
                    ("provider", serde_json::json!(target.provider_label)),
                    ("dimension", serde_json::json!(target.dimension)),
                    (
                        "target_signature",
                        serde_json::json!(target.signature.clone()),
                    ),
                ],
            );
            let processed =
                backfill_and_mark_ready(service, &db, provider, &target, settings.batch_size)
                    .await?;
            log_recovery_event(
                &service.logger,
                "embedding.backfill_completed",
                LogLevel::Info,
                [("processed", serde_json::json!(processed))],
            );
            Ok(RecoveryCycleOutcome::Completed)
        }
        RecoveryDecision::EnableForNewFacts(target) => {
            let provider = backend.create_provider(target.dimension).await?;
            install_recovery_provider(service, &db, &target, provider.clone(), false).await?;
            log_recovery_event(
                &service.logger,
                "embedding.reembed_required",
                LogLevel::Warn,
                [
                    ("reason", serde_json::json!("embedding signature differs")),
                    ("dimension", serde_json::json!(target.dimension)),
                    ("target_signature", serde_json::json!(target.signature)),
                ],
            );
            let processed = match run_backfill(
                service,
                provider,
                &target.signature,
                target.model.as_deref(),
                target.dimension,
                settings.batch_size,
            )
            .await?
            {
                BackfillOutcome::Complete { processed } => processed,
            };
            log_recovery_event(
                &service.logger,
                "embedding.backfill_completed",
                LogLevel::Info,
                [("processed", serde_json::json!(processed))],
            );
            Ok(RecoveryCycleOutcome::Completed)
        }
        RecoveryDecision::DimensionMismatch {
            index_dimension,
            probed_dimension,
        } => {
            log_recovery_event(
                &service.logger,
                "embedding.reembed_required",
                LogLevel::Warn,
                [
                    ("reason", serde_json::json!("embedding dimension differs")),
                    ("index_dimension", serde_json::json!(index_dimension)),
                    ("probed_dimension", serde_json::json!(probed_dimension)),
                ],
            );
            Err(MemoryError::Validation(format!(
                "embedding recovery dimension mismatch: index={index_dimension}, probed={probed_dimension}"
            )))
        }
    }
}

fn log_recovery_probe_failure(
    logger: &StdoutLogger,
    failures: u32,
    error: &MemoryError,
    level: LogLevel,
) {
    log_recovery_event(
        logger,
        "embedding.recovery_probe_failed",
        level,
        [
            ("consecutive_failures", serde_json::json!(failures)),
            ("error", serde_json::json!(error.to_string())),
            (
                "next_backoff_secs",
                serde_json::json!(recovery_backoff(failures).as_secs()),
            ),
        ],
    );
}

pub(crate) async fn run_recovery_worker(
    service: MemoryService,
    config: EmbeddingConfig,
    backend: Arc<dyn EmbeddingRecoveryBackend>,
    settings: RecoveryWorkerSettings,
    shutdown: CancellationToken,
) {
    let mut consecutive_failures = 0u32;
    let mut delay = settings.initial_probe_delay;
    log_recovery_event(
        &service.logger,
        "embedding.recovery_started",
        LogLevel::Info,
        [("initial_delay_secs", serde_json::json!(delay.as_secs()))],
    );

    loop {
        let waited = tokio::select! {
            _ = shutdown.cancelled() => false,
            _ = tokio::time::sleep(delay) => true,
        };
        if !waited {
            log_recovery_event(
                &service.logger,
                "embedding.recovery_stopped",
                LogLevel::Info,
                [],
            );
            return;
        }

        match run_recovery_cycle(&service, &config, backend.as_ref(), &settings).await {
            Ok(RecoveryCycleOutcome::Completed) => return,
            Err(error) => {
                consecutive_failures = consecutive_failures.saturating_add(1);
                let level = if consecutive_failures >= settings.warn_demote_after {
                    LogLevel::Debug
                } else {
                    LogLevel::Warn
                };
                log_recovery_probe_failure(&service.logger, consecutive_failures, &error, level);
                delay = recovery_backoff_with_settings(
                    consecutive_failures,
                    settings.backoff_base,
                    settings.backoff_cap,
                );
            }
        }
    }
}

#[derive(Clone)]
pub(crate) struct EmbeddingRecoveryRuntime {
    shutdown: CancellationToken,
    handles: Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl EmbeddingRecoveryRuntime {
    pub(crate) fn new() -> Self {
        Self {
            shutdown: CancellationToken::new(),
            handles: Arc::new(tokio::sync::Mutex::new(Vec::new())),
        }
    }

    pub(crate) async fn spawn(
        &self,
        service: MemoryService,
        config: EmbeddingConfig,
        backend: Arc<dyn EmbeddingRecoveryBackend>,
        settings: RecoveryWorkerSettings,
    ) {
        let shutdown = self.shutdown.clone();
        let handle = tokio::spawn(async move {
            run_recovery_worker(service, config, backend, settings, shutdown).await;
        });
        self.handles.lock().await.push(handle);
    }

    pub(crate) async fn shutdown(&self) {
        self.shutdown.cancel();
        let handles = std::mem::take(&mut *self.handles.lock().await);
        for handle in handles {
            let _ = handle.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use async_trait::async_trait;
    use chrono::Utc;
    use serde_json::{Value, json};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    use crate::config::{
        DEFAULT_EMBEDDING_DIMENSION, DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD, EmbeddingConfig,
        EmbeddingProviderKind,
    };

    use crate::service::{
        DisabledEmbeddingProvider, EmbeddingProvider, MemoryService, normalize_dt,
    };
    use crate::storage::{DbClient, SurrealDbClient};

    use super::*;

    fn remote_config() -> EmbeddingConfig {
        EmbeddingConfig {
            provider: EmbeddingProviderKind::OpenAiCompatible,
            base_url: Some("http://127.0.0.1:12345/v1".to_string()),
            model: Some("test-model".to_string()),
            ..EmbeddingConfig::default()
        }
    }

    #[test]
    fn recovery_decision_requires_same_dimension_and_same_or_absent_signature() {
        let config = remote_config();
        let full = choose_recovery_decision(&config, 1536, None, 1536);
        assert!(matches!(full, RecoveryDecision::FullRecovery(target) if target.dimension == 1536));

        let enabled = choose_recovery_decision(&config, 1536, Some("embsig:old"), 1536);
        assert!(
            matches!(enabled, RecoveryDecision::EnableForNewFacts(target) if target.dimension == 1536)
        );

        let incompatible = choose_recovery_decision(&config, 1536, Some("embsig:new"), 768);
        assert!(matches!(
            incompatible,
            RecoveryDecision::DimensionMismatch {
                index_dimension: 1536,
                probed_dimension: 768
            }
        ));
    }

    #[test]
    fn recovery_backoff_is_15_seconds_then_doubles_and_caps() {
        assert_eq!(recovery_backoff(1), Duration::from_secs(15));
        assert_eq!(recovery_backoff(2), Duration::from_secs(30));
        assert_eq!(recovery_backoff(3), Duration::from_secs(60));
        assert_eq!(recovery_backoff(6), Duration::from_secs(300));
        assert_eq!(recovery_backoff(20), Duration::from_secs(300));
    }

    struct FakeEmbeddingProvider {
        remaining_failures: AtomicUsize,
        dimension: usize,
    }

    impl FakeEmbeddingProvider {
        fn new(dimension: usize) -> Self {
            Self {
                remaining_failures: AtomicUsize::new(0),
                dimension,
            }
        }

        fn transient_once(dimension: usize) -> Self {
            Self {
                remaining_failures: AtomicUsize::new(1),
                dimension,
            }
        }
    }

    #[async_trait]
    impl EmbeddingProvider for FakeEmbeddingProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "openai-compatible"
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        async fn embed(&self, _input: &str) -> Result<Vec<f64>, crate::service::MemoryError> {
            let remaining = self.remaining_failures.load(Ordering::SeqCst);
            if remaining > 0 {
                self.remaining_failures.fetch_sub(1, Ordering::SeqCst);
                return Err(crate::service::MemoryError::Transient(
                    "synthetic backfill outage".to_string(),
                ));
            }
            let mut embedding = vec![0.0; self.dimension];
            embedding[0] = 1.0;
            Ok(embedding)
        }
    }

    async fn make_in_memory_db(name: &str) -> Arc<SurrealDbClient> {
        let database = format!(
            "embedding_recovery_{name}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        );
        let db = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                &database,
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory"),
        );
        db.apply_migrations("org").await.expect("migrations");
        db
    }

    async fn seed_missing_fact(db: &Arc<SurrealDbClient>, fact_id: &str, content: &str) {
        let now = normalize_dt(Utc::now());
        db.create(
            fact_id,
            json!({
                "fact_id": fact_id,
                "fact_type": "note",
                "content": content,
                "quote": content,
                "source_episode": "episode:seed",
                "t_valid": now,
                "t_ingested": now,
                "confidence": 0.9,
                "index_keys": [],
                "access_count": 0,
                "entity_links": [],
                "scope": "org",
                "policy_tags": [],
                "provenance": {"source_episode": "episode:seed"}
            }),
            "org",
        )
        .await
        .expect("missing fact");
    }

    async fn seed_fact_with_embedding(db: &Arc<SurrealDbClient>, fact_id: &str) {
        let now = normalize_dt(Utc::now());
        db.create(
            fact_id,
            json!({
                "fact_id": fact_id,
                "fact_type": "note",
                "content": "existing vector",
                "quote": "existing vector",
                "source_episode": "episode:seed",
                "t_valid": now,
                "t_ingested": now,
                "confidence": 0.9,
                "index_keys": [],
                "access_count": 0,
                "entity_links": [],
                "scope": "org",
                "policy_tags": [],
                "provenance": {"source_episode": "episode:seed"},
                "embedding": vec![0.1f64; DEFAULT_EMBEDDING_DIMENSION],
                "embedding_provider": "legacy-test",
                "embedding_dimension": DEFAULT_EMBEDDING_DIMENSION,
                "embedding_signature": "embsig:old",
                "embedding_updated_at": now
            }),
            "org",
        )
        .await
        .expect("existing fact");
    }

    fn make_disabled_service(db: Arc<SurrealDbClient>) -> MemoryService {
        MemoryService::new_with_embedding_provider(
            db,
            "org".to_string(),
            "warn".to_string(),
            50,
            100,
            Arc::new(DisabledEmbeddingProvider::new(DEFAULT_EMBEDDING_DIMENSION)),
            DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            Arc::new(crate::service::AnnoEntityExtractor::new().expect("anno extractor")),
        )
        .expect("service")
    }

    async fn read_http_request(socket: &mut TcpStream) -> std::io::Result<()> {
        let mut request = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            let read = socket.read(&mut byte).await?;
            if read == 0 {
                break;
            }
            request.push(byte[0]);
            if request.ends_with(b"\r\n\r\n") || request.ends_with(b"\n\n") {
                return Ok(());
            }
            if request.len() > 64 * 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "HTTP request headers exceed test limit",
                ));
            }
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "HTTP request ended before headers",
        ))
    }

    async fn write_embedding_response(
        socket: &mut TcpStream,
        status: &str,
        dimension: usize,
    ) -> std::io::Result<()> {
        let mut embedding = vec![0.0f64; dimension];
        embedding[0] = 1.0;
        let body = serde_json::to_vec(&json!({
            "data": [{"embedding": embedding}]
        }))
        .map_err(|error| std::io::Error::other(error.to_string()))?;
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        socket.write_all(headers.as_bytes()).await?;
        socket.write_all(&body).await?;
        socket.shutdown().await
    }

    struct FakeRecoveryBackend {
        remaining_probe_failures: AtomicUsize,
        probe_count: AtomicUsize,
        provider: Arc<dyn EmbeddingProvider>,
    }

    impl FakeRecoveryBackend {
        fn fail_probes_then_succeed(failures: usize, dimension: usize) -> Self {
            Self {
                remaining_probe_failures: AtomicUsize::new(failures),
                probe_count: AtomicUsize::new(0),
                provider: Arc::new(FakeEmbeddingProvider::new(dimension)),
            }
        }

        fn probe_succeeds_with_flaky_provider(dimension: usize, failures: usize) -> Self {
            Self {
                remaining_probe_failures: AtomicUsize::new(0),
                probe_count: AtomicUsize::new(0),
                provider: Arc::new(if failures == 0 {
                    FakeEmbeddingProvider::new(dimension)
                } else {
                    FakeEmbeddingProvider::transient_once(dimension)
                }),
            }
        }

        fn probe_calls(&self) -> usize {
            self.probe_count.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl EmbeddingRecoveryBackend for FakeRecoveryBackend {
        async fn probe_dimension(&self) -> Result<usize, crate::service::MemoryError> {
            self.probe_count.fetch_add(1, Ordering::SeqCst);
            let remaining = self.remaining_probe_failures.load(Ordering::SeqCst);
            if remaining > 0 {
                self.remaining_probe_failures.fetch_sub(1, Ordering::SeqCst);
                return Err(crate::service::MemoryError::Transient(
                    "synthetic probe outage".to_string(),
                ));
            }
            Ok(self.provider.dimension())
        }

        async fn create_provider(
            &self,
            _dimension: usize,
        ) -> Result<Arc<dyn EmbeddingProvider>, crate::service::MemoryError> {
            Ok(self.provider.clone())
        }
    }

    fn test_settings() -> RecoveryWorkerSettings {
        RecoveryWorkerSettings {
            initial_probe_delay: Duration::ZERO,
            backoff_base: Duration::from_millis(1),
            backoff_cap: Duration::from_millis(4),
            warn_demote_after: 3,
            batch_size: 100,
        }
    }

    #[tokio::test]
    async fn worker_retries_probe_then_recovers_and_exits_when_backfill_is_empty() {
        let db = make_in_memory_db("worker_retry").await;
        let service = make_disabled_service(db);
        let backend = Arc::new(FakeRecoveryBackend::fail_probes_then_succeed(2, 1536));
        let config = remote_config();

        run_recovery_worker(
            service.clone(),
            config,
            backend.clone(),
            test_settings(),
            CancellationToken::new(),
        )
        .await;

        assert_eq!(backend.probe_calls(), 3);
        assert!(service.embedding_runtime_snapshot().provider.is_enabled());
    }

    #[tokio::test]
    async fn recovery_clears_durable_backfill_pending_marker_after_resume() {
        let db = make_in_memory_db("backfill_resume").await;
        seed_missing_fact(&db, "fact:offline", "saved while disconnected").await;
        let config = remote_config();
        let signature = crate::config::build_embedding_signature(
            config.provider_label(),
            config.model.as_deref(),
            config.base_url.as_deref(),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        db.create(
            crate::storage::embedding_state_store::EMBEDDING_STATE_RECORD_ID,
            json!({
                "status": "backfill_pending",
                "active_signature": signature,
                "provider": config.provider_label(),
                "model": config.model,
                "dimension": DEFAULT_EMBEDDING_DIMENSION,
                "updated_at": normalize_dt(Utc::now()),
            }),
            "org",
        )
        .await
        .expect("pending embedding state");

        let service = make_disabled_service(db.clone());
        let backend = Arc::new(FakeRecoveryBackend::fail_probes_then_succeed(
            0,
            DEFAULT_EMBEDDING_DIMENSION,
        ));

        run_recovery_worker(
            service,
            config,
            backend,
            test_settings(),
            CancellationToken::new(),
        )
        .await;

        let state = db
            .select_one(
                crate::storage::embedding_state_store::EMBEDDING_STATE_RECORD_ID,
                "org",
            )
            .await
            .expect("read state")
            .expect("state exists");
        assert_eq!(state.get("status").and_then(Value::as_str), Some("ready"));
    }

    #[tokio::test]
    async fn worker_backfills_missing_facts_after_signature_change_without_rewriting_stale_vectors()
    {
        let db = make_in_memory_db("signature_change_backfill").await;
        seed_missing_fact(&db, "fact:offline", "offline fact").await;
        seed_fact_with_embedding(&db, "fact:stale").await;
        let config = remote_config();
        db.create(
            crate::storage::embedding_state_store::EMBEDDING_STATE_RECORD_ID,
            json!({
                "status": "ready",
                "active_signature": "embsig:old",
                "provider": "legacy-test",
                "model": "legacy-model",
                "dimension": DEFAULT_EMBEDDING_DIMENSION,
                "updated_at": normalize_dt(Utc::now()),
            }),
            "org",
        )
        .await
        .expect("legacy embedding state");
        let service = make_disabled_service(db.clone());
        let backend = Arc::new(FakeRecoveryBackend::fail_probes_then_succeed(
            0,
            DEFAULT_EMBEDDING_DIMENSION,
        ));
        let expected_signature = crate::config::build_embedding_signature(
            config.provider_label(),
            config.model.as_deref(),
            config.base_url.as_deref(),
            DEFAULT_EMBEDDING_DIMENSION,
        );

        run_recovery_worker(
            service,
            config,
            backend,
            test_settings(),
            CancellationToken::new(),
        )
        .await;

        let missing = db
            .select_one("fact:offline", "org")
            .await
            .expect("read missing fact")
            .expect("missing fact");
        assert_eq!(
            missing.get("embedding_signature").and_then(Value::as_str),
            Some(expected_signature.as_str())
        );
        let stale = db
            .select_one("fact:stale", "org")
            .await
            .expect("read stale fact")
            .expect("stale fact");
        assert_eq!(
            stale.get("embedding_signature").and_then(Value::as_str),
            Some("embsig:old")
        );
        let state = db
            .select_one(
                crate::storage::embedding_state_store::EMBEDDING_STATE_RECORD_ID,
                "org",
            )
            .await
            .expect("read embedding state")
            .expect("embedding state");
        assert_eq!(
            state.get("active_signature").and_then(Value::as_str),
            Some("embsig:old")
        );
    }

    #[tokio::test]
    async fn worker_returns_to_probe_after_backfill_network_failure() {
        let db = make_in_memory_db("worker_backfill_cycle").await;
        seed_missing_fact(&db, "fact:offline", "offline fact").await;
        let service = make_disabled_service(db.clone());
        let backend = Arc::new(FakeRecoveryBackend::probe_succeeds_with_flaky_provider(
            1536, 1,
        ));
        let config = remote_config();
        let expected_signature = crate::config::build_embedding_signature(
            config.provider_label(),
            config.model.as_deref(),
            config.base_url.as_deref(),
            1536,
        );

        run_recovery_worker(
            service,
            config,
            backend.clone(),
            test_settings(),
            CancellationToken::new(),
        )
        .await;

        assert!(backend.probe_calls() >= 2);
        let fact = db
            .select_one("fact:offline", "org")
            .await
            .expect("read")
            .expect("fact");
        assert_eq!(
            fact.get("embedding_signature").and_then(Value::as_str),
            Some(expected_signature.as_str())
        );
    }

    #[tokio::test]
    async fn tcp_listener_recovery_probe_failure_then_success_backfills_offline_fact() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base_url = format!("http://{}", listener.local_addr().expect("address"));
        let failures = Arc::new(AtomicUsize::new(1));
        let server_failures = failures.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.expect("accept");
                read_http_request(&mut socket).await.expect("request");
                let failed = server_failures
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok();
                write_embedding_response(
                    &mut socket,
                    if failed {
                        "503 Service Unavailable"
                    } else {
                        "200 OK"
                    },
                    DEFAULT_EMBEDDING_DIMENSION,
                )
                .await
                .expect("response");
            }
        });

        let db = make_in_memory_db("tcp_recovery").await;
        seed_missing_fact(&db, "fact:offline", "saved while disconnected").await;
        let service = make_disabled_service(db.clone());
        let config = EmbeddingConfig {
            base_url: Some(base_url.clone()),
            timeout_secs: 1,
            ..remote_config()
        };
        let expected_signature = crate::config::build_embedding_signature(
            config.provider_label(),
            config.model.as_deref(),
            config.base_url.as_deref(),
            DEFAULT_EMBEDDING_DIMENSION,
        );
        let backend = Arc::new(ConfiguredEmbeddingRecoveryBackend::new(
            config.clone(),
            ".".to_string(),
        ));
        let settings = RecoveryWorkerSettings {
            initial_probe_delay: Duration::ZERO,
            backoff_base: Duration::from_millis(1),
            backoff_cap: Duration::from_millis(2),
            warn_demote_after: 3,
            batch_size: 100,
        };

        tokio::time::timeout(
            Duration::from_secs(5),
            run_recovery_worker(
                service.clone(),
                config,
                backend,
                settings,
                CancellationToken::new(),
            ),
        )
        .await
        .expect("worker should recover and finish backfill");
        server.abort();

        assert!(service.embedding_runtime_snapshot().provider.is_enabled());
        let fact = db
            .select_one("fact:offline", "org")
            .await
            .expect("read")
            .expect("fact");
        assert_eq!(
            fact.get("embedding_dimension").and_then(Value::as_u64),
            Some(DEFAULT_EMBEDDING_DIMENSION as u64)
        );
        assert_eq!(
            fact.get("embedding_signature").and_then(Value::as_str),
            Some(expected_signature.as_str())
        );
    }

    #[tokio::test]
    async fn backfill_embeds_missing_facts_but_does_not_touch_existing_vectors() {
        let db = make_in_memory_db("backfill_missing").await;
        seed_missing_fact(&db, "fact:missing", "offline fact").await;
        seed_fact_with_embedding(&db, "fact:stale").await;
        let service = make_disabled_service(db.clone());
        let provider = Arc::new(FakeEmbeddingProvider::new(DEFAULT_EMBEDDING_DIMENSION));

        let outcome = run_backfill(
            &service,
            provider,
            "embsig:target",
            Some("test-model"),
            DEFAULT_EMBEDDING_DIMENSION,
            100,
        )
        .await
        .expect("backfill");
        assert!(matches!(
            outcome,
            BackfillOutcome::Complete { processed: 1 }
        ));

        let missing = db
            .select_one("fact:missing", "org")
            .await
            .expect("read")
            .expect("fact");
        assert_eq!(
            missing.get("embedding_dimension").and_then(Value::as_u64),
            Some(DEFAULT_EMBEDDING_DIMENSION as u64)
        );
        assert_eq!(
            missing.get("embedding_signature").and_then(Value::as_str),
            Some("embsig:target")
        );
        assert_eq!(
            missing.get("embedding_provider").and_then(Value::as_str),
            Some("openai-compatible")
        );

        let stale = db
            .select_one("fact:stale", "org")
            .await
            .expect("read")
            .expect("fact");
        assert_eq!(
            stale.get("embedding_signature").and_then(Value::as_str),
            Some("embsig:old")
        );
    }

    #[test]
    fn recovery_worker_gate_requires_exact_preflight_failure_and_remote_provider() {
        let remote = EmbeddingConfig {
            provider: EmbeddingProviderKind::OpenAiCompatible,
            ..EmbeddingConfig::default()
        };
        let local = EmbeddingConfig {
            provider: EmbeddingProviderKind::LocalCandle,
            ..EmbeddingConfig::default()
        };
        let preflight_failure = EmbeddingStartupDecision::DisableSemantic {
            reason: "embedding target preflight failed".to_string(),
        };
        assert!(should_spawn_embedding_recovery(
            EmbeddingActivationMode::Standard,
            &preflight_failure,
            &remote,
        ));
        assert!(should_spawn_embedding_recovery(
            EmbeddingActivationMode::Standard,
            &EmbeddingStartupDecision::ResumePendingBackfill {
                active_signature: "embsig:ok".to_string(),
            },
            &remote,
        ));
        assert!(should_spawn_embedding_recovery(
            EmbeddingActivationMode::Standard,
            &EmbeddingStartupDecision::RecoverMissingEmbeddings {
                target_signature: "embsig:new".to_string(),
            },
            &remote,
        ));
        assert!(!should_spawn_embedding_recovery(
            EmbeddingActivationMode::ForceEnabledForReembed,
            &preflight_failure,
            &remote,
        ));
        assert!(!should_spawn_embedding_recovery(
            EmbeddingActivationMode::Standard,
            &EmbeddingStartupDecision::DisableSemantic {
                reason: "configured embedding signature differs".to_string(),
            },
            &remote,
        ));
        assert!(!should_spawn_embedding_recovery(
            EmbeddingActivationMode::Standard,
            &preflight_failure,
            &local,
        ));
        let mut opted_out = remote.clone();
        opted_out.auto_recovery = false;
        assert!(!should_spawn_embedding_recovery(
            EmbeddingActivationMode::Standard,
            &preflight_failure,
            &opted_out,
        ));
        assert!(is_remote_embedding_provider("openai-compatible"));
    }

    #[tokio::test]
    async fn backfill_returns_error_when_provider_is_temporarily_unavailable() {
        let db = make_in_memory_db("backfill_retry").await;
        seed_missing_fact(&db, "fact:missing", "offline fact").await;
        let service = make_disabled_service(db);
        let provider = Arc::new(FakeEmbeddingProvider::transient_once(
            DEFAULT_EMBEDDING_DIMENSION,
        ));

        let result = run_backfill(
            &service,
            provider,
            "embsig:target",
            None,
            DEFAULT_EMBEDDING_DIMENSION,
            100,
        )
        .await;
        assert!(matches!(
            result,
            Err(crate::service::MemoryError::Transient(_))
        ));
    }
}
