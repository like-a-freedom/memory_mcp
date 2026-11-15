use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use memory_mcp::error::MemoryError;
use memory_mcp::storage::{DbClient, table_scope::OwnedTable};
use serde_json::{Map, Value, json};

pub const FIXED_CUTOFF: &str = "2026-11-13T15:52:35.269Z";
pub const FIXTURE_NAMESPACE: &str = "org";

pub fn corrected_unguarded_triple_sql() -> &'static str {
    "SELECT fact_id, fact_type, content, quote, source_episode, t_valid, t_ingested, \
     t_invalid, t_invalid_ingested, confidence, index_keys, access_count, last_accessed, \
     entity_links, scope, policy_tags, provenance FROM fact \
     WHERE fact_id IN ( \
       SELECT VALUE source_fact_id FROM triple \
       WHERE (predicate CONTAINS $query OR object CONTAINS $query OR subject CONTAINS $query) \
     ) \
       AND t_valid <= type::datetime($cutoff) \
       AND (t_ingested IS NONE OR t_ingested <= type::datetime($cutoff)) \
       AND (t_invalid IS NONE OR t_invalid > type::datetime($cutoff) \
            OR t_invalid_ingested > type::datetime($cutoff)) \
     LIMIT $limit"
}

pub fn corrected_triple_vars(query: &str, cutoff: &str, limit: usize) -> Value {
    json!({
        "query": query,
        "cutoff": cutoff,
        "limit": limit,
    })
}

pub async fn seed_triple_fixture(
    db: &dyn DbClient,
    namespace: &str,
    fact_count: usize,
    dimension: usize,
) -> Result<(), MemoryError> {
    if fact_count < 3 || dimension == 0 {
        return Err(MemoryError::Validation(
            "triple fixture requires at least three facts and a positive embedding dimension"
                .to_string(),
        ));
    }

    let facts = (0..fact_count)
        .map(|index| {
            let suffix = format!("{index:04}");
            let embedding = (0..dimension)
                .map(|axis| (((index * 31 + axis * 17) % 101) as f32) / 100.0)
                .collect::<Vec<_>>();
            json!({
                "record_key": format!("triple-fixture-{suffix}"),
                "fact_id": format!("fact:triple-fixture-{suffix}"),
                "content": format!("synthetic triple fixture fact {suffix}"),
                "embedding": embedding,
            })
        })
        .collect::<Vec<_>>();

    db.query(
        "FOR $fact IN $facts {\n\
             CREATE type::record(\"fact\", $fact.record_key) CONTENT {
                 fact_id: $fact.fact_id,\n\
                 fact_type: \"note\",\n\
                 content: $fact.content,\n\
                 quote: $fact.content,\n\
                 source_episode: \"episode:triple-fixture\",\n\
                 t_valid: type::datetime(\"2026-11-13T14:00:00Z\"),\n\
                 t_ingested: type::datetime(\"2026-11-13T14:00:00Z\"),\n\
                 confidence: 0.9,\n\
                 entity_links: [],\n\
                 embedding: $fact.embedding,\n\
                 policy_tags: [],\n\
                 provenance: { ingestion_method: \"manual\" }\n\
             };\n\
         };",
        Some(json!({ "facts": facts })),
        namespace,
    )
    .await?;

    let triples = [
        (
            "triple-fixture-subject",
            "subject-match",
            "predicate-subject",
            "object-subject",
            "fact:triple-fixture-0000",
        ),
        (
            "triple-fixture-predicate",
            "subject-predicate",
            "predicate-match",
            "object-predicate",
            "fact:triple-fixture-0001",
        ),
        (
            "triple-fixture-object",
            "subject-object",
            "predicate-object",
            "object-match",
            "fact:triple-fixture-0002",
        ),
        (
            "triple-fixture-duplicate",
            "duplicate-match",
            "predicate-duplicate",
            "object-duplicate",
            "fact:triple-fixture-0000",
        ),
        (
            "triple-fixture-dangling",
            "quote 'snow' 日本語",
            "predicate-dangling",
            "object-dangling",
            "fact:triple-fixture-missing",
        ),
    ];

    for (record_key, subject, predicate, object, source_fact_id) in triples {
        db.create(
            &format!("triple:{record_key}"),
            json!({
                "subject": subject,
                "predicate": predicate,
                "object": object,
                "confidence": 0.9,
                "source_fact_id": source_fact_id,
            }),
            namespace,
            memory_mcp::knowledge::queries::TRIPLE_TEMPORAL_FIELDS,
        )
        .await?;
    }

    Ok(())
}

pub fn normalized_rows(rows: &[Value]) -> Vec<String> {
    let mut serialized = rows
        .iter()
        .map(|row| {
            serde_json::to_string(&canonicalize(row))
                .expect("JSON values must serialize during triple row normalization")
        })
        .collect::<Vec<_>>();
    serialized.sort();
    serialized
}

fn canonicalize(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(canonicalize).collect()),
        Value::Object(object) => {
            let mut entries = object
                .iter()
                .map(|(key, value)| (key.clone(), canonicalize(value)))
                .collect::<Vec<_>>();
            entries.sort_by(|left, right| left.0.cmp(&right.0));
            let mut canonical = Map::new();
            for (key, value) in entries {
                canonical.insert(key, value);
            }
            Value::Object(canonical)
        }
        scalar => scalar.clone(),
    }
}

#[derive(Clone)]
pub struct SqlOverrideDbClient {
    inner: Arc<dyn DbClient>,
    query_override: Option<String>,
    query_calls: Arc<AtomicUsize>,
}

impl SqlOverrideDbClient {
    pub fn new(inner: Arc<dyn DbClient>, query_override: Option<&str>) -> Self {
        Self {
            inner,
            query_override: query_override.map(str::to_owned),
            query_calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[allow(dead_code)]
    pub fn query_counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.query_calls)
    }
}

#[async_trait]
impl DbClient for SqlOverrideDbClient {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, MemoryError> {
        self.inner.select_one(record_id, namespace).await
    }

    async fn select_table(
        &self,
        table: OwnedTable,
        namespace: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        self.inner.select_table(table, namespace).await
    }

    async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<Value, MemoryError> {
        self.inner
            .create(record_id, content, namespace, temporal_fields)
            .await
    }

    async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<Value, MemoryError> {
        self.inner
            .update(record_id, content, namespace, temporal_fields)
            .await
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, MemoryError> {
        self.query_calls.fetch_add(1, Ordering::Relaxed);
        self.inner
            .query(
                self.query_override.as_deref().unwrap_or(sql),
                vars,
                namespace,
            )
            .await
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        self.inner.apply_migrations(namespace).await
    }
}
