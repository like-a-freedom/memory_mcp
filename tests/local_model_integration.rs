use std::env;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::Utc;
use memory_mcp::MemoryService;
use memory_mcp::models::{AssembleContextRequest, ExtractedEntity, IngestRequest};
use memory_mcp::service::{EntityExtractor, GlinerEntityExtractor};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::Mutex;

static ENV_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

const ORG_SCOPE: &str = "org";
const LOCAL_CANDLE_DIMENSION: usize = 384;
const GLINER_REQUIRED_FILES: &[&str] =
    &["gliner_config.json", "model.safetensors", "tokenizer.json"];
const LOCAL_CANDLE_REQUIRED_FILES: &[&str] =
    &["config.json", "model.safetensors", "tokenizer.json"];

struct EnvGuard {
    saved: Vec<(String, Option<String>)>,
}

impl EnvGuard {
    fn apply(pairs: Vec<(&'static str, Option<String>)>) -> Self {
        let mut saved = Vec::with_capacity(pairs.len());
        for (key, value) in pairs {
            saved.push((key.to_string(), env::var(key).ok()));
            unsafe {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
        Self { saved }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (key, value) in self.saved.iter().rev() {
            unsafe {
                match value {
                    Some(value) => env::set_var(key, value),
                    None => env::remove_var(key),
                }
            }
        }
    }
}

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn local_gliner_model_dir() -> PathBuf {
    repo_root()
        .join("tests")
        .join("models")
        .join("ner")
        .join("urchade--gliner_multi-v2.1")
}

fn local_candle_model_dir() -> PathBuf {
    repo_root()
        .join("tests")
        .join("models")
        .join("intfloat")
        .join("multilingual-e5-small")
}

fn assert_required_files(model_dir: &Path, file_names: &[&str]) {
    assert!(
        model_dir.exists(),
        "expected local model directory to exist: {}",
        model_dir.display()
    );

    for file_name in file_names {
        let path = model_dir.join(file_name);
        assert!(
            path.is_file(),
            "expected local model artifact to exist: {}",
            path.display()
        );
    }
}

fn configure_embedded_env(temp_dir: &TempDir) -> Vec<(&'static str, Option<String>)> {
    vec![
        (
            "SURREALDB_DB_NAME",
            Some("memory_local_model_tests".to_string()),
        ),
        ("SURREALDB_EMBEDDED", Some("true".to_string())),
        (
            "SURREALDB_DATA_DIR",
            Some(temp_dir.path().display().to_string()),
        ),
        ("SURREALDB_NAMESPACES", Some(ORG_SCOPE.to_string())),
        ("SURREALDB_USERNAME", Some("root".to_string())),
        ("SURREALDB_PASSWORD", Some("root".to_string())),
        ("SURREALDB_URL", None),
        ("QUERY_LOGGING_ENABLED", Some("false".to_string())),
        ("RUST_LOG", Some("warn".to_string())),
    ]
}

fn local_gliner_env(temp_dir: &TempDir) -> EnvGuard {
    let mut pairs = configure_embedded_env(temp_dir);
    pairs.extend([
        ("NER_PROVIDER", Some("local-gliner".to_string())),
        ("NER_MODEL", Some("urchade/gliner_multi-v2.1".to_string())),
        (
            "NER_MODEL_DIR",
            Some(local_gliner_model_dir().display().to_string()),
        ),
        ("NER_LABELS", Some("person,company,location".to_string())),
        ("NER_THRESHOLD", Some("0.35".to_string())),
        ("NER_BATCH_SIZE", Some("4".to_string())),
        ("EMBEDDINGS_ENABLED", Some("false".to_string())),
        ("EMBEDDINGS_PROVIDER", None),
        ("EMBEDDINGS_MODEL", None),
        ("EMBEDDINGS_MODEL_DIR", None),
        ("EMBEDDINGS_BASE_URL", None),
        ("EMBEDDINGS_API_KEY", None),
        ("EMBEDDINGS_SIMILARITY_THRESHOLD", None),
        ("SURREALDB_EMBEDDING_DIMENSION", None),
    ]);
    EnvGuard::apply(pairs)
}

fn local_candle_env(temp_dir: &TempDir, similarity_threshold: f64) -> EnvGuard {
    let mut pairs = configure_embedded_env(temp_dir);
    pairs.extend([
        ("NER_PROVIDER", Some("regex".to_string())),
        ("NER_MODEL", None),
        ("NER_MODEL_DIR", None),
        ("NER_LABELS", None),
        ("NER_THRESHOLD", None),
        ("NER_BATCH_SIZE", None),
        ("EMBEDDINGS_ENABLED", Some("true".to_string())),
        ("EMBEDDINGS_PROVIDER", Some("local-candle".to_string())),
        (
            "EMBEDDINGS_MODEL",
            Some("intfloat/multilingual-e5-small".to_string()),
        ),
        (
            "EMBEDDINGS_MODEL_DIR",
            Some(local_candle_model_dir().display().to_string()),
        ),
        (
            "EMBEDDINGS_SIMILARITY_THRESHOLD",
            Some(similarity_threshold.to_string()),
        ),
        (
            "SURREALDB_EMBEDDING_DIMENSION",
            Some(LOCAL_CANDLE_DIMENSION.to_string()),
        ),
        ("EMBEDDINGS_BASE_URL", None),
        ("EMBEDDINGS_API_KEY", None),
    ]);
    EnvGuard::apply(pairs)
}

fn entity_name_matches(actual: &str, expected: &str) -> bool {
    let actual = actual.trim().to_lowercase();
    let expected = expected.trim().to_lowercase();
    actual == expected || actual.contains(&expected) || expected.contains(&actual)
}

fn assert_extracted_entity(
    entities: &[ExtractedEntity],
    entity_type: &str,
    expected_names: &[&str],
) {
    assert!(
        entities.iter().any(|entity| {
            entity.entity_type == entity_type
                && expected_names
                    .iter()
                    .any(|name| entity_name_matches(&entity.canonical_name, name))
        }),
        "expected entity type `{entity_type}` with one of {:?}, got {:?}",
        expected_names,
        entities
            .iter()
            .map(|entity| format!("{}:{}", entity.entity_type, entity.canonical_name))
            .collect::<Vec<_>>()
    );
}

fn assert_candidate_entity(
    entities: &[memory_mcp::models::EntityCandidate],
    entity_type: &str,
    expected_names: &[&str],
) {
    assert!(
        entities.iter().any(|entity| {
            entity.entity_type == entity_type
                && expected_names
                    .iter()
                    .any(|name| entity_name_matches(&entity.canonical_name, name))
        }),
        "expected candidate type `{entity_type}` with one of {:?}, got {:?}",
        expected_names,
        entities
            .iter()
            .map(|entity| format!("{}:{}", entity.entity_type, entity.canonical_name))
            .collect::<Vec<_>>()
    );
}

fn content_source_id(content: &str) -> String {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    format!("source-{:016x}", hasher.finish())
}

async fn ingest_episode(service: &MemoryService, content: &str) -> String {
    service
        .ingest(
            IngestRequest {
                source_type: "test".to_string(),
                source_id: content_source_id(content),
                content: content.to_string(),
                t_ref: Utc::now(),
                scope: ORG_SCOPE.to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: Vec::new(),
            },
            None,
        )
        .await
        .expect("ingest should succeed")
}

async fn add_note_fact(service: &MemoryService, source_episode: &str, content: &str) -> String {
    service
        .add_fact(
            "note",
            content,
            content,
            source_episode,
            Utc::now(),
            ORG_SCOPE,
            0.95,
            Vec::new(),
            Vec::new(),
            json!({"source": "local-model-integration-test"}),
        )
        .await
        .expect("fact insertion should succeed")
}

fn extract_embedding(record: &Value) -> Vec<f64> {
    record
        .get("embedding")
        .and_then(Value::as_array)
        .expect("fact record should contain embedding array")
        .iter()
        .map(|value| value.as_f64().expect("embedding element should be numeric"))
        .collect()
}

fn vector_norm(values: &[f64]) -> f64 {
    values.iter().map(|value| value * value).sum::<f64>().sqrt()
}

fn cosine_similarity(left: &[f64], right: &[f64]) -> f64 {
    let dot = left
        .iter()
        .zip(right.iter())
        .map(|(l, r)| l * r)
        .sum::<f64>();
    let norm_product = vector_norm(left) * vector_norm(right);
    dot / norm_product
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires local GLiNER model files under tests/models/ner/urchade--gliner_multi-v2.1"]
async fn local_gliner_extractor_detects_expected_entities() {
    let model_dir = local_gliner_model_dir();
    assert_required_files(&model_dir, GLINER_REQUIRED_FILES);

    let extractor = GlinerEntityExtractor::new(
        &model_dir,
        vec![
            "person".to_string(),
            "company".to_string(),
            "location".to_string(),
        ],
        0.35,
    )
    .expect("GLiNER extractor should load the local model");

    let entities = extractor
        .extract_candidates("Alice Smith joined Microsoft in London on 5 March 2026.")
        .await
        .expect("GLiNER extraction should succeed");

    assert_candidate_entity(&entities, "person", &["Alice Smith", "Alice"]);
    assert_candidate_entity(&entities, "company", &["Microsoft"]);
    assert_candidate_entity(&entities, "location", &["London"]);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires local GLiNER model files under tests/models/ner/urchade--gliner_multi-v2.1"]
async fn memory_service_uses_local_gliner_from_env_for_extract() {
    let _env_lock = ENV_LOCK.lock().await;
    let temp_dir = TempDir::new().expect("temp dir should be created");
    let _env = local_gliner_env(&temp_dir);

    let service = MemoryService::new_from_env()
        .await
        .expect("service should bootstrap with local GLiNER");
    let episode_id = ingest_episode(
        &service,
        "Alice Smith joined Microsoft in London on 5 March 2026.",
    )
    .await;

    let extracted = service
        .extract(&episode_id, None)
        .await
        .expect("extract should succeed with local GLiNER");

    assert_eq!(extracted.episode_id, episode_id);
    assert_extracted_entity(&extracted.entities, "person", &["Alice Smith", "Alice"]);
    assert_extracted_entity(&extracted.entities, "company", &["Microsoft"]);
    assert_extracted_entity(&extracted.entities, "location", &["London"]);
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires local multilingual-e5-small model files under tests/models/intfloat/multilingual-e5-small"]
async fn memory_service_persists_real_local_candle_embeddings() {
    let _env_lock = ENV_LOCK.lock().await;
    let model_dir = local_candle_model_dir();
    assert_required_files(&model_dir, LOCAL_CANDLE_REQUIRED_FILES);

    let temp_dir = TempDir::new().expect("temp dir should be created");
    let _env = local_candle_env(&temp_dir, 0.0);

    let service = MemoryService::new_from_env()
        .await
        .expect("service should bootstrap with local Candle embeddings");
    let source_episode =
        ingest_episode(&service, "The compensation committee finished its review.").await;

    let compensation_fact = add_note_fact(
        &service,
        &source_episode,
        "Compensation increase approved for the engineering team.",
    )
    .await;
    let paraphrase_fact = add_note_fact(
        &service,
        &source_episode,
        "Salary raise approved for the engineering group.",
    )
    .await;
    let unrelated_fact = add_note_fact(
        &service,
        &source_episode,
        "Fresh fruit was delivered to the office kitchen.",
    )
    .await;

    let compensation_record = service
        .db_client
        .select_one(&compensation_fact, ORG_SCOPE)
        .await
        .expect("fact lookup should succeed")
        .expect("fact should exist");
    let paraphrase_record = service
        .db_client
        .select_one(&paraphrase_fact, ORG_SCOPE)
        .await
        .expect("fact lookup should succeed")
        .expect("fact should exist");
    let unrelated_record = service
        .db_client
        .select_one(&unrelated_fact, ORG_SCOPE)
        .await
        .expect("fact lookup should succeed")
        .expect("fact should exist");

    let compensation_embedding = extract_embedding(&compensation_record);
    let paraphrase_embedding = extract_embedding(&paraphrase_record);
    let unrelated_embedding = extract_embedding(&unrelated_record);

    assert_eq!(compensation_embedding.len(), LOCAL_CANDLE_DIMENSION);
    assert_eq!(paraphrase_embedding.len(), LOCAL_CANDLE_DIMENSION);
    assert_eq!(unrelated_embedding.len(), LOCAL_CANDLE_DIMENSION);

    let compensation_norm = vector_norm(&compensation_embedding);
    let paraphrase_norm = vector_norm(&paraphrase_embedding);
    let unrelated_norm = vector_norm(&unrelated_embedding);

    assert!(
        (0.99..=1.01).contains(&compensation_norm),
        "expected normalized compensation embedding, got norm {compensation_norm}"
    );
    assert!(
        (0.99..=1.01).contains(&paraphrase_norm),
        "expected normalized paraphrase embedding, got norm {paraphrase_norm}"
    );
    assert!(
        (0.99..=1.01).contains(&unrelated_norm),
        "expected normalized unrelated embedding, got norm {unrelated_norm}"
    );

    let paraphrase_similarity = cosine_similarity(&compensation_embedding, &paraphrase_embedding);
    let unrelated_similarity = cosine_similarity(&compensation_embedding, &unrelated_embedding);

    assert!(
        paraphrase_similarity > unrelated_similarity,
        "expected semantic paraphrase similarity ({paraphrase_similarity}) to exceed unrelated similarity ({unrelated_similarity})"
    );
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires local multilingual-e5-small model files under tests/models/intfloat/multilingual-e5-small"]
async fn memory_service_assemble_context_uses_real_local_candle_embeddings() {
    let _env_lock = ENV_LOCK.lock().await;
    let model_dir = local_candle_model_dir();
    assert_required_files(&model_dir, LOCAL_CANDLE_REQUIRED_FILES);

    let temp_dir = TempDir::new().expect("temp dir should be created");
    let _env = local_candle_env(&temp_dir, 0.0);

    let service = MemoryService::new_from_env()
        .await
        .expect("service should bootstrap with local Candle embeddings");
    let target_episode = ingest_episode(&service, "Compensation decisions were published.").await;
    let distractor_episode = ingest_episode(&service, "Facilities updates were published.").await;

    let target_fact = add_note_fact(
        &service,
        &target_episode,
        "Compensation increase approved for the engineering team.",
    )
    .await;
    let _distractor_fact = add_note_fact(
        &service,
        &distractor_episode,
        "Fresh fruit was delivered to the office kitchen.",
    )
    .await;

    let context = service
        .assemble_context(AssembleContextRequest {
            query: "salary raise for engineers".to_string(),
            scope: ORG_SCOPE.to_string(),
            project: None,
            fact_types: Vec::new(),
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble_context should succeed with local Candle embeddings");

    assert!(
        !context.is_empty(),
        "expected semantic retrieval to return at least one fact"
    );
    assert_eq!(
        context[0].fact_id,
        target_fact,
        "expected the semantically matching fact to rank first, got {:?}",
        context
            .iter()
            .map(|item| (
                item.fact_id.clone(),
                item.content.clone(),
                item.retrieval_tier.clone()
            ))
            .collect::<Vec<_>>()
    );
    assert!(
        context.iter().any(|item| item.fact_id == target_fact),
        "expected target fact to appear in assembled context, got {:?}",
        context
            .iter()
            .map(|item| (
                item.fact_id.clone(),
                item.content.clone(),
                item.retrieval_tier.clone()
            ))
            .collect::<Vec<_>>()
    );
}
