use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use memory_mcp::MemoryError;
use memory_mcp::embedding::providers::EmbeddingProvider;
use memory_mcp::eval_support::retrieval::{EvalRetrievalConfig, new_eval_retrieval_service};
use memory_mcp::models::Provenance;
use memory_mcp::service::MemoryService;
use memory_mcp::storage::table_scope::{KnowledgeTables, OwnedTable};
use memory_mcp::storage::{DbClient, ReleaseOwnedTable, SurrealDbClient};
use serde_json::{Value, json};
use tokio::sync::Mutex;

const EMBEDDING_DIMENSION: usize = 1536;
const FACT_CONTENT: &str = "HNSW vectors retrieve stored facts by cosine similarity.";
const QUERY: &str = "How does the service find conceptually related recollections?";

static NEXT_DATABASE_ID: AtomicUsize = AtomicUsize::new(1);

type ObservedAnnRows = Arc<Mutex<Vec<Value>>>;
type ObservedQuerySql = Arc<Mutex<Vec<String>>>;

#[derive(Clone, Debug)]
pub struct ReembedBatchRow {
    pub fact_id: Option<String>,
    pub fact_type: Option<String>,
    pub content: Option<String>,
    pub quote: Option<String>,
    pub columns: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct ReembedBatchObservation {
    pub sql: String,
    pub rows: Vec<ReembedBatchRow>,
}

#[derive(Clone)]
pub struct ReembedBatchObserverHandle {
    observations: Arc<Mutex<Vec<ReembedBatchObservation>>>,
}

pub struct ReembedBatchObserverDb {
    inner: Arc<dyn DbClient>,
    observations: Arc<Mutex<Vec<ReembedBatchObservation>>>,
}

impl ReembedBatchObserverDb {
    pub fn new(inner: Arc<dyn DbClient>) -> (Self, ReembedBatchObserverHandle) {
        let observations = Arc::new(Mutex::new(Vec::new()));
        let db = Self {
            inner,
            observations: observations.clone(),
        };
        let observer = ReembedBatchObserverHandle { observations };
        (db, observer)
    }
}

impl ReembedBatchObserverHandle {
    pub async fn batches(&self) -> Vec<ReembedBatchObservation> {
        self.observations.lock().await.clone()
    }
}

#[async_trait]
impl DbClient for ReembedBatchObserverDb {
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
        let result = self.inner.query(sql, vars, namespace).await?;
        if sql.contains("embedding_signature") && sql.contains("ORDER BY fact_id ASC LIMIT $limit")
        {
            let rows = result.as_array().ok_or_else(|| {
                MemoryError::Storage("reembed observer expected an array result".to_string())
            })?;
            let observations = rows
                .iter()
                .map(|row| {
                    let object = row.as_object();
                    ReembedBatchRow {
                        fact_id: object
                            .and_then(|map| map.get("fact_id"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        fact_type: object
                            .and_then(|map| map.get("fact_type"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        content: object
                            .and_then(|map| map.get("content"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        quote: object
                            .and_then(|map| map.get("quote"))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        columns: object
                            .map(|map| map.keys().cloned().collect())
                            .unwrap_or_default(),
                    }
                })
                .collect();
            self.observations
                .lock()
                .await
                .push(ReembedBatchObservation {
                    sql: sql.to_string(),
                    rows: observations,
                });
        }
        Ok(result)
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        self.inner.apply_migrations(namespace).await
    }
}

#[derive(Clone, Copy)]
pub enum AnnEvidence {
    Missing,
    MissingScoreWithStoredVector,
    WrongWidthFallback,
    MalformedScore,
    ExplicitScore(f64),
}

struct FixedEmbeddingProvider {
    vector: Vec<f64>,
}

#[async_trait]
impl EmbeddingProvider for FixedEmbeddingProvider {
    fn is_enabled(&self) -> bool {
        true
    }

    fn provider_name(&self) -> &'static str {
        "memory-mode-semantic-fixture"
    }

    fn dimension(&self) -> usize {
        self.vector.len()
    }

    async fn embed(&self, _input: &str) -> Result<Vec<f64>, MemoryError> {
        Ok(self.vector.clone())
    }
}

pub fn fixed_embedding_provider(dimension: usize) -> Arc<dyn EmbeddingProvider> {
    let mut vector = vec![0.0; dimension];
    vector[0] = 1.0;
    Arc::new(FixedEmbeddingProvider { vector })
}

pub struct AnnEvidenceDb {
    inner: Arc<dyn DbClient>,
    evidence: AnnEvidence,
    observed_ann_rows: Arc<Mutex<Vec<Value>>>,
    observed_query_sql: Arc<Mutex<Vec<String>>>,
}

impl AnnEvidenceDb {
    fn new(
        inner: Arc<dyn DbClient>,
        evidence: AnnEvidence,
    ) -> (Self, ObservedAnnRows, ObservedQuerySql) {
        let observed_ann_rows = Arc::new(Mutex::new(Vec::new()));
        let observed_query_sql = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                inner,
                evidence,
                observed_ann_rows: observed_ann_rows.clone(),
                observed_query_sql: observed_query_sql.clone(),
            },
            observed_ann_rows,
            observed_query_sql,
        )
    }

    fn apply_evidence(&self, row: &mut Value) -> Result<(), MemoryError> {
        match self.evidence {
            AnnEvidence::Missing => {
                let object = row.as_object_mut().ok_or_else(|| {
                    MemoryError::Storage("ANN fixture received a non-object row".to_string())
                })?;
                object.remove("sem_score");
                object.remove("embedding");
            }
            AnnEvidence::MissingScoreWithStoredVector => {
                let object = row.as_object_mut().ok_or_else(|| {
                    MemoryError::Storage("ANN fixture received a non-object row".to_string())
                })?;
                object.remove("sem_score");
                if !object.contains_key("embedding") {
                    return Err(MemoryError::Storage(
                        "ANN fixture record has no stored vector".to_string(),
                    ));
                }
            }
            AnnEvidence::WrongWidthFallback => {
                let object = row.as_object_mut().ok_or_else(|| {
                    MemoryError::Storage("ANN fixture received a non-object row".to_string())
                })?;
                object.remove("sem_score");
                object.insert("embedding".to_string(), json!([1.0]));
            }
            AnnEvidence::MalformedScore => {
                let object = row.as_object_mut().ok_or_else(|| {
                    MemoryError::Storage("ANN fixture received a non-object row".to_string())
                })?;
                object.remove("embedding");
                object.insert("sem_score".to_string(), json!("not-a-number"));
            }
            AnnEvidence::ExplicitScore(score) => {
                let object = row.as_object_mut().ok_or_else(|| {
                    MemoryError::Storage("ANN fixture received a non-object row".to_string())
                })?;
                object.remove("embedding");
                object.insert("sem_score".to_string(), json!(score));
            }
        }
        Ok(())
    }
}

#[async_trait]
impl DbClient for AnnEvidenceDb {
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
        self.observed_query_sql.lock().await.push(sql.to_string());
        if sql.contains("vector::similarity::cosine") {
            let facts = self
                .inner
                .select_table(KnowledgeTables::table("fact"), namespace)
                .await?;
            let mut rows = facts
                .into_iter()
                .filter(|row| row["content"] == FACT_CONTENT)
                .collect::<Vec<_>>();
            for row in &mut rows {
                self.apply_evidence(row)?;
            }
            self.observed_ann_rows
                .lock()
                .await
                .extend(rows.iter().cloned());
            return Ok(Value::Array(rows));
        }
        self.inner.query(sql, vars, namespace).await
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        self.inner.apply_migrations(namespace).await
    }
}

pub struct SemanticFixture {
    pub service: MemoryService,
    pub fact_id: String,
    pub observed_ann_rows: Arc<Mutex<Vec<Value>>>,
    pub observed_query_sql: Arc<Mutex<Vec<String>>>,
}

impl SemanticFixture {
    pub async fn observed_ann_rows(&self) -> Vec<Value> {
        self.observed_ann_rows.lock().await.clone()
    }

    pub async fn observed_query_sql(&self) -> Vec<String> {
        self.observed_query_sql.lock().await.clone()
    }
}

pub async fn semantic_fixture(evidence: AnnEvidence, threshold: f64) -> SemanticFixture {
    let sequence = NEXT_DATABASE_ID.fetch_add(1, Ordering::Relaxed);
    let database = format!("embedding_modes_semantic_{sequence}");
    let db = Arc::new(
        SurrealDbClient::connect_in_memory(&database, "main", "warn")
            .await
            .expect("isolated semantic database connected"),
    );
    db.apply_migrations("main")
        .await
        .expect("semantic fixture migrations applied");

    let mut vector = vec![0.0; EMBEDDING_DIMENSION];
    vector[0] = 1.0;
    let (ann_db, observed_ann_rows, observed_query_sql) = AnnEvidenceDb::new(db.clone(), evidence);
    let service = new_eval_retrieval_service(
        Arc::new(ann_db),
        "main".to_string(),
        EvalRetrievalConfig {
            embedding_provider: Arc::new(FixedEmbeddingProvider { vector }),
            semantic_threshold: threshold,
            claim_stage: "evidence".to_string(),
        },
    )
    .await
    .expect("controlled semantic service created");
    let fact_id = service
        .add_fact(
            "note",
            FACT_CONTENT,
            FACT_CONTENT,
            "episode:ann-evidence",
            chrono::Utc::now(),
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:ann-evidence"),
        )
        .await
        .expect("semantic fact written through owner API");

    SemanticFixture {
        service,
        fact_id,
        observed_ann_rows,
        observed_query_sql,
    }
}

pub fn semantic_query() -> &'static str {
    QUERY
}
