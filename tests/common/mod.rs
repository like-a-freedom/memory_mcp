use std::sync::Arc;

use chrono::{DateTime, Utc};
use memory_mcp::models::IngestRequest;
use memory_mcp::service::{AddFactRequest, MemoryError, MemoryService};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::{Value, json};

/// Creates a temporary directory for file-based SurrealDB (RocksDB).
/// The directory is automatically deleted when the returned guard is dropped.
#[allow(dead_code)]
pub struct TempDbDir {
    pub path: std::path::PathBuf,
}

impl Drop for TempDbDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Creates a MemoryService backed by file-based SurrealDB (RocksDB) in a temp directory.
/// The directory is automatically cleaned up when the returned guard is dropped.
/// Use this for eval tests that need stability beyond what in-memory DB provides.
#[allow(dead_code)]
pub async fn make_file_service() -> (MemoryService, TempDbDir) {
    let namespaces = vec![
        "org".to_string(),
        "personal".to_string(),
        "private".to_string(),
    ];

    // Create temp directory for RocksDB — unique per call to avoid lock conflicts
    static FILE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = FILE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let temp_dir =
        std::env::temp_dir().join(format!("memory_mcp_eval_{}_{}", std::process::id(), id));
    let _ = std::fs::remove_dir_all(&temp_dir);
    std::fs::create_dir_all(&temp_dir).expect("create temp dir");

    let db_client = SurrealDbClient::connect_embedded_with_namespaces(
        &temp_dir.to_string_lossy(),
        &namespaces,
        "warn",
    )
    .await
    .expect("connect embedded service");
    for namespace in &namespaces {
        db_client
            .apply_migrations(namespace)
            .await
            .expect("apply migrations");
    }

    let service = MemoryService::new(Arc::new(db_client), namespaces, "warn".to_string(), 50, 100)
        .expect("service init");

    (service, TempDbDir { path: temp_dir })
}

#[allow(dead_code, clippy::too_many_arguments)]
pub async fn add_fact(
    service: &MemoryService,
    fact_type: &str,
    content: &str,
    quote: &str,
    source_episode: &str,
    t_valid: DateTime<Utc>,
    scope: &str,
    confidence: f64,
    entity_links: Vec<String>,
    policy_tags: Vec<String>,
    provenance: Value,
) -> Result<String, MemoryError> {
    MemoryService::add_fact(
        service,
        AddFactRequest {
            fact_type,
            content,
            quote,
            source_episode,
            t_valid,
            scope,
            confidence,
            entity_links,
            policy_tags,
            provenance,
        },
    )
    .await
}

#[allow(dead_code)]
pub async fn make_service() -> MemoryService {
    let namespaces = vec![
        "org".to_string(),
        "personal".to_string(),
        "private".to_string(),
    ];
    // Use a unique DB name per call to avoid embedded SurrealDB session conflicts
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let db_name = format!("memory_test_{id}");
    let db_client =
        SurrealDbClient::connect_in_memory_with_namespaces(&db_name, &namespaces, "warn")
            .await
            .expect("connect in memory service");
    for namespace in &namespaces {
        db_client
            .apply_migrations(namespace)
            .await
            .expect("apply in-memory migrations");
    }

    MemoryService::new(Arc::new(db_client), namespaces, "warn".to_string(), 50, 100)
        .expect("service init")
}

#[allow(dead_code)]
pub async fn make_service_with_client() -> (MemoryService, Arc<SurrealDbClient>) {
    let namespaces = vec![
        "org".to_string(),
        "personal".to_string(),
        "private".to_string(),
    ];
    // Use a unique DB name to avoid embedded SurrealDB session conflicts
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let db_name = format!("memory_test_{id}");
    let db_client = Arc::new(
        SurrealDbClient::connect_in_memory_with_namespaces(&db_name, &namespaces, "warn")
            .await
            .expect("connect in memory service"),
    );
    for namespace in &namespaces {
        db_client
            .apply_migrations(namespace)
            .await
            .expect("apply in-memory migrations");
    }

    let service = MemoryService::new(db_client.clone(), namespaces, "warn".to_string(), 50, 100)
        .expect("service init");

    (service, db_client)
}

/// Creates a MemoryService with GLiNER NER and LocalCandle embeddings.
///
/// Uses local model fixtures:
/// - GLiNER: `tests/fixtures/gliner_multi_v2.1/`
/// - Embeddings: `tests/fixtures/multilingual-e5-small/`
///
/// Panics if model files are missing — eval tests require the full ML stack.
#[allow(dead_code)]
pub async fn make_service_with_gliner_and_embeddings() -> MemoryService {
    use memory_mcp::service::{
        EmbeddingProvider, EntityExtractor, GlinerEntityExtractor, LocalCandleEmbeddingProvider,
    };
    use std::path::Path;

    let namespaces = vec![
        "org".to_string(),
        "personal".to_string(),
        "private".to_string(),
    ];
    // Use a unique DB name to avoid embedded SurrealDB session conflicts
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let db_name = format!("memory_test_{id}");
    let db_client =
        SurrealDbClient::connect_in_memory_with_namespaces(&db_name, &namespaces, "warn")
            .await
            .expect("connect in memory service");
    for namespace in &namespaces {
        db_client
            .apply_migrations(namespace)
            .await
            .expect("apply in-memory migrations");
    }

    // GLiNER entity extractor — use local fixtures, no fallback
    let gliner_model_dir = Path::new("tests/fixtures/gliner_multi_v2.1");
    assert!(
        gliner_model_dir.join("tokenizer.json").exists(),
        "GLiNER model not found at {:?}. Run: huggingface-cli download urchade/gliner_multi-v2.1 --local-dir tests/fixtures/gliner_multi_v2.1",
        gliner_model_dir
    );
    let labels = vec![
        "person".to_string(),
        "organization".to_string(),
        "location".to_string(),
    ];
    let extractor = GlinerEntityExtractor::new(gliner_model_dir, labels, 0.1)
        .expect("failed to create GLiNER extractor");
    let entity_extractor: Arc<dyn EntityExtractor> = Arc::new(extractor);

    // LocalCandle embedding provider — use local fixtures, no fallback
    let embedding_model_dir = Path::new("tests/fixtures/multilingual-e5-small");
    assert!(
        embedding_model_dir.join("tokenizer.json").exists(),
        "Embedding model not found at {:?}. Run: huggingface-cli download intfloat/multilingual-e5-small --local-dir tests/fixtures/multilingual-e5-small",
        embedding_model_dir
    );
    let embedding_provider: Arc<dyn EmbeddingProvider> = {
        let dimension = 384;
        let max_tokens = 512;
        let provider = LocalCandleEmbeddingProvider::new(
            "intfloat/multilingual-e5-small",
            dimension,
            max_tokens,
            embedding_model_dir,
        )
        .expect("failed to create LocalCandle embedding provider");
        Arc::new(provider)
    };

    MemoryService::new_with_providers(
        Arc::new(db_client),
        namespaces,
        "warn".to_string(),
        50,
        100,
        embedding_provider,
        0.7,
        entity_extractor,
    )
    .expect("service init")
}

/// Creates a MemoryService with LocalCandle embeddings only (no GLiNER NER).
///
/// Uses local fixture: `tests/fixtures/multilingual-e5-small/`
/// Falls back to AnnoEntityExtractor when GLiNER is not needed.
#[allow(dead_code)]
pub async fn make_service_with_local_embeddings() -> MemoryService {
    use memory_mcp::service::{EmbeddingProvider, LocalCandleEmbeddingProvider};
    use std::path::Path;

    let namespaces = vec![
        "org".to_string(),
        "personal".to_string(),
        "private".to_string(),
    ];
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = COUNTER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let db_name = format!("memory_test_{id}");
    let db_client =
        SurrealDbClient::connect_in_memory_with_namespaces(&db_name, &namespaces, "warn")
            .await
            .expect("connect in memory service");
    for namespace in &namespaces {
        db_client
            .apply_migrations(namespace)
            .await
            .expect("apply in-memory migrations");
    }

    let embedding_model_dir = Path::new("tests/fixtures/multilingual-e5-small");
    assert!(
        embedding_model_dir.join("tokenizer.json").exists(),
        "Embedding model not found at {:?}. Run: huggingface-cli download intfloat/multilingual-e5-small --local-dir tests/fixtures/multilingual-e5-small",
        embedding_model_dir
    );
    let embedding_provider: Arc<dyn EmbeddingProvider> = {
        let dimension = 384;
        let max_tokens = 512;
        let provider = LocalCandleEmbeddingProvider::new(
            "intfloat/multilingual-e5-small",
            dimension,
            max_tokens,
            embedding_model_dir,
        )
        .expect("failed to create LocalCandle embedding provider");
        Arc::new(provider)
    };

    MemoryService::new_with_providers(
        Arc::new(db_client),
        namespaces,
        "warn".to_string(),
        50,
        100,
        embedding_provider,
        0.7,
        Arc::new(memory_mcp::service::AnnoEntityExtractor::new().expect("anno extractor")),
    )
    .expect("service init")
}

#[allow(dead_code)]
pub async fn ingest_episode(service: &MemoryService, source_id: &str, content: &str) -> String {
    let request = IngestRequest {
        source_type: "chat".to_string(),
        source_id: source_id.to_string(),
        content: content.to_string(),
        t_ref: "2026-03-01T10:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("static timestamp should parse"),
        scope: "personal".to_string(),
        t_ingested: None,
        visibility_scope: None,
        policy_tags: vec![],
    };
    let episode_id = service
        .ingest(request, None)
        .await
        .expect("ingest should succeed");
    service
        .extract(&episode_id, None)
        .await
        .expect("extract should succeed");
    episode_id
}

#[allow(dead_code)]
pub async fn seed_fact_at(
    service: &MemoryService,
    scope: &str,
    content: &str,
    t_valid: DateTime<Utc>,
) -> String {
    add_fact(
        service,
        "note",
        content,
        content,
        "episode:seed",
        t_valid,
        scope,
        0.9,
        vec![],
        vec![],
        json!({"source_episode": "episode:seed"}),
    )
    .await
    .expect("seed fact should succeed")
}
