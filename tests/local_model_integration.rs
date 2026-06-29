use std::collections::BTreeSet;
use std::env;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use chrono::Utc;
use memory_mcp::MemoryService;
use memory_mcp::config::NerConfig;
use memory_mcp::models::{AssembleContextRequest, ExtractedEntity, IngestRequest, Provenance};
use memory_mcp::service::{EntityExtractor, GlinerEntityExtractor};
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

fn supported_gliner_labels() -> Vec<String> {
    NerConfig::default().labels
}

type GlinerExpectedEntities = Vec<(&'static str, Vec<&'static str>)>;
type GlinerCoverageCase = (&'static str, &'static str, GlinerExpectedEntities);

fn gliner_diverse_coverage_cases() -> Vec<GlinerCoverageCase> {
    vec![
        (
            "multilingual launch",
            "Иван Петров from Microsoft unveiled Surface Laptop 6 at Build 2026 in Seattle using Kubernetes.",
            vec![
                ("person", vec!["Иван Петров"]),
                ("company", vec!["Microsoft"]),
                ("product", vec!["Surface Laptop 6"]),
                ("event", vec!["Build 2026"]),
                ("location", vec!["Seattle"]),
                ("technology", vec!["Kubernetes"]),
            ],
        ),
        (
            "multi-actor comparison",
            "At Cloud Summit 2026 in Berlin, Alice Smith and Bob Jones from Google and DeepMind compared Pixel 8 Pro with PostgreSQL.",
            vec![
                ("person", vec!["Alice Smith", "Bob Jones"]),
                ("company", vec!["Google", "DeepMind"]),
                ("product", vec!["Pixel 8 Pro"]),
                ("event", vec!["Cloud Summit 2026"]),
                ("location", vec!["Berlin"]),
                ("technology", vec!["PostgreSQL"]),
            ],
        ),
        (
            "newline demo",
            "During AI Summit 2026 in Madrid,\nMaría García of OpenAI demoed ChatGPT Enterprise built on Rust.",
            vec![
                ("person", vec!["María García"]),
                ("company", vec!["OpenAI"]),
                ("product", vec!["ChatGPT Enterprise"]),
                ("event", vec!["AI Summit 2026"]),
                ("location", vec!["Madrid"]),
                ("technology", vec!["Rust"]),
            ],
        ),
    ]
}

fn assert_gliner_case_matrix_covers_supported_labels(cases: &[GlinerCoverageCase]) {
    let covered_labels = cases
        .iter()
        .flat_map(|(_, _, expected_entities)| expected_entities.iter().map(|(label, _)| *label))
        .map(String::from)
        .collect::<BTreeSet<_>>();
    let supported_labels = supported_gliner_labels()
        .into_iter()
        .collect::<BTreeSet<_>>();

    assert_eq!(
        covered_labels, supported_labels,
        "GLiNER coverage cases should span every default supported label"
    );
}

fn local_gliner_env(temp_dir: &TempDir, labels: Option<&[String]>, threshold: f64) -> EnvGuard {
    let mut pairs = configure_embedded_env(temp_dir);
    pairs.extend([
        ("NER_PROVIDER", Some("local-gliner".to_string())),
        ("NER_MODEL", Some("urchade/gliner_multi-v2.1".to_string())),
        (
            "NER_MODEL_DIR",
            Some(local_gliner_model_dir().display().to_string()),
        ),
        ("NER_LABELS", labels.map(|labels| labels.join(","))),
        ("NER_THRESHOLD", Some(threshold.to_string())),
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
    case_name: &str,
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
        "case `{case_name}` expected entity type `{entity_type}` with one of {:?}, got {:?}",
        expected_names,
        entities
            .iter()
            .map(|entity| format!("{}:{}", entity.entity_type, entity.canonical_name))
            .collect::<Vec<_>>()
    );
}

fn assert_candidate_entity(
    case_name: &str,
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
        "case `{case_name}` expected candidate type `{entity_type}` with one of {:?}, got {:?}",
        expected_names,
        entities
            .iter()
            .map(|entity| format!("{}:{}", entity.entity_type, entity.canonical_name))
            .collect::<Vec<_>>()
    );
}

fn assert_candidate_case_entities(
    case_name: &str,
    entities: &[memory_mcp::models::EntityCandidate],
    expected_entities: &GlinerExpectedEntities,
) {
    for (entity_type, expected_names) in expected_entities {
        assert_candidate_entity(case_name, entities, entity_type, expected_names);
    }
}

fn assert_extracted_case_entities(
    case_name: &str,
    entities: &[ExtractedEntity],
    expected_entities: &GlinerExpectedEntities,
) {
    for (entity_type, expected_names) in expected_entities {
        assert_extracted_entity(case_name, entities, entity_type, expected_names);
    }
}

fn zero_shot_gliner_labels() -> Vec<String> {
    vec!["project", "deal", "asset"]
        .into_iter()
        .map(String::from)
        .collect()
}

fn zero_shot_gliner_coverage_cases() -> Vec<GlinerCoverageCase> {
    vec![(
        "project deal asset mix",
        "The Apollo project closed a deal to acquire the asset Orion.",
        vec![
            ("project", vec!["Apollo project"]),
            ("deal", vec!["deal", "closed a deal", "Orion deal"]),
            ("asset", vec!["asset Orion"]),
        ],
    )]
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires local GLiNER model files under tests/models/ner/urchade--gliner_multi-v2.1"]
async fn local_gliner_extractor_detects_custom_zero_shot_entities() {
    let model_dir = local_gliner_model_dir();
    assert_required_files(&model_dir, GLINER_REQUIRED_FILES);

    let extractor = GlinerEntityExtractor::new(&model_dir, zero_shot_gliner_labels(), 0.2)
        .expect("GLiNER extractor should load the local model for zero-shot labels");

    for (case_name, text, expected_entities) in zero_shot_gliner_coverage_cases() {
        let entities = extractor
            .extract_candidates(text)
            .await
            .unwrap_or_else(|err| {
                panic!("GLiNER zero-shot extraction should succeed for `{case_name}`: {err}")
            });

        assert_candidate_case_entities(case_name, &entities, &expected_entities);
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires local GLiNER model files under tests/models/ner/urchade--gliner_multi-v2.1"]
async fn local_gliner_extractor_supports_per_call_custom_labels() {
    let model_dir = local_gliner_model_dir();
    assert_required_files(&model_dir, GLINER_REQUIRED_FILES);

    // Load extractor with default labels
    let default_labels = supported_gliner_labels();
    let extractor = GlinerEntityExtractor::new(&model_dir, default_labels, 0.2)
        .expect("GLiNER extractor should load the local model");

    // Test that we can override labels per-call using extract_candidates_with_labels
    let custom_labels = zero_shot_gliner_labels();
    for (case_name, text, expected_entities) in zero_shot_gliner_coverage_cases() {
        let entities = extractor
            .extract_candidates_with_labels(text, &custom_labels)
            .await
            .unwrap_or_else(|err| {
                panic!(
                    "GLiNER per-call custom labels extraction should succeed for `{case_name}`: {err}"
                )
            });

        // Verify extraction returns entities (model accuracy varies by label)
        assert!(
            !entities.is_empty() || expected_entities.is_empty(),
            "per-call extraction should return entities for `{case_name}`, got {:?}",
            entities
        );
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires local GLiNER model files under tests/models/ner/urchade--gliner_multi-v2.1"]
async fn memory_service_uses_local_gliner_zero_shot_labels() {
    let _env_lock = ENV_LOCK.lock().await;
    let temp_dir = TempDir::new().expect("temp dir should be created");
    let labels = zero_shot_gliner_labels();
    let _env = local_gliner_env(&temp_dir, Some(&labels), 0.2);

    let service = MemoryService::new_from_env()
        .await
        .expect("service should bootstrap with local GLiNER zero-shot labels");

    for (case_name, text, expected_entities) in zero_shot_gliner_coverage_cases() {
        let episode_id = ingest_episode(&service, text).await;
        let extracted = service
            .extract(&episode_id, None, None)
            .await
            .unwrap_or_else(|err| {
                panic!("extract should succeed with local GLiNER zero-shot labels for `{case_name}`: {err}")
            });

        assert_eq!(
            extracted.episode_id, episode_id,
            "service extraction should preserve episode id for `{case_name}`"
        );
        assert_extracted_case_entities(case_name, &extracted.entities, &expected_entities);
    }
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
            Provenance::agent_observation(source_episode),
        )
        .await
        .expect("fact insertion should succeed")
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires local GLiNER model files under tests/models/ner/urchade--gliner_multi-v2.1"]
async fn local_gliner_extractor_detects_all_default_supported_entities_across_diverse_texts() {
    let model_dir = local_gliner_model_dir();
    assert_required_files(&model_dir, GLINER_REQUIRED_FILES);
    let cases = gliner_diverse_coverage_cases();
    assert_gliner_case_matrix_covers_supported_labels(&cases);

    let extractor = GlinerEntityExtractor::new(&model_dir, supported_gliner_labels(), 0.35)
        .expect("GLiNER extractor should load the local model");

    for (case_name, text, expected_entities) in cases {
        let entities = extractor
            .extract_candidates(text)
            .await
            .unwrap_or_else(|err| {
                panic!("GLiNER extraction should succeed for `{case_name}`: {err}")
            });

        assert_candidate_case_entities(case_name, &entities, &expected_entities);
    }
}

#[tokio::test(flavor = "current_thread")]
#[ignore = "requires local GLiNER model files under tests/models/ner/urchade--gliner_multi-v2.1"]
async fn memory_service_uses_local_gliner_defaults_across_diverse_texts() {
    let _env_lock = ENV_LOCK.lock().await;
    let temp_dir = TempDir::new().expect("temp dir should be created");
    let _env = local_gliner_env(&temp_dir, None, 0.35);
    let cases = gliner_diverse_coverage_cases();
    assert_gliner_case_matrix_covers_supported_labels(&cases);

    let service = MemoryService::new_from_env()
        .await
        .expect("service should bootstrap with local GLiNER");

    for (case_name, text, expected_entities) in cases {
        let episode_id = ingest_episode(&service, text).await;
        let extracted = service
            .extract(&episode_id, None, None)
            .await
            .unwrap_or_else(|err| {
                panic!("extract should succeed with local GLiNER for `{case_name}`: {err}")
            });

        assert_eq!(
            extracted.episode_id, episode_id,
            "service extraction should preserve episode id for `{case_name}`"
        );
        assert_extracted_case_entities(case_name, &extracted.entities, &expected_entities);
    }
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

    let _compensation_fact = add_note_fact(
        &service,
        &source_episode,
        "Compensation increase approved for the engineering team.",
    )
    .await;
    let _paraphrase_fact = add_note_fact(
        &service,
        &source_episode,
        "Salary raise approved for the engineering group.",
    )
    .await;
    let _unrelated_fact = add_note_fact(
        &service,
        &source_episode,
        "Fresh fruit was delivered to the office kitchen.",
    )
    .await;

    // Verify embeddings were persisted by checking semantic similarity
    let context = service
        .assemble_context(AssembleContextRequest {
            query: "compensation raise engineering".to_string(),
            scope: ORG_SCOPE.to_string(),
            project: None,
            fact_types: vec![],
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble_context should succeed");

    // Both compensation and paraphrase facts should be retrieved (semantic match)
    let content_set: std::collections::HashSet<_> =
        context.iter().map(|i| i.content.as_str()).collect();
    assert!(
        content_set
            .iter()
            .any(|c| c.contains("Compensation") || c.contains("compensation")),
        "semantic retrieval should find compensation fact, got: {:?}",
        content_set
    );
    assert!(
        content_set
            .iter()
            .any(|c| c.contains("Salary") || c.contains("raise")),
        "semantic retrieval should find paraphrase fact, got: {:?}",
        content_set
    );
    // Compensation fact should rank higher than unrelated (semantic similarity)
    let compensation_pos = context
        .iter()
        .position(|i| i.content.contains("Compensation"));
    let unrelated_pos = context.iter().position(|i| i.content.contains("fruit"));
    if let (Some(c_pos), Some(u_pos)) = (compensation_pos, unrelated_pos) {
        assert!(
            c_pos < u_pos,
            "compensation fact (pos {c_pos}) should rank above unrelated (pos {u_pos})"
        );
    }
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
