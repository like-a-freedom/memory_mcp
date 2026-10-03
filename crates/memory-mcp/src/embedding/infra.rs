//! Infra adapter binding the owner-approved canonical vector
//! port to the knowledge-owned fact record.
//!
//! Expiry removal: Phase 5, when the fact vector write
//! becomes knowledge-owned storage rather than a bootstrap
//! adapter over the legacy fact store.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::MemoryError;
use crate::embedding::api::{
    CanonicalVectorPort, StoredVector, VectorApplication, VectorIdentity, VectorWritePolicy,
};
use crate::shared::temporal::normalize_dt;
use crate::storage::BoundDbClient;

/// Applies validated vectors to canonical fact records.
///
/// The stored field set mirrors the historical background
/// retry write: vector, provider, signature and update time.
/// Dimension and model are written only when the resolved
/// target reports them, so a runtime without that knowledge
/// stores exactly what it stored before.
pub struct FactVectorAdapter {
    db: BoundDbClient,
    backfill: crate::embedding::backfill_store::EmbeddingBackfillStoreClient,
    model: Option<String>,
    dimension: Option<usize>,
}

impl FactVectorAdapter {
    pub fn new(
        db: Arc<dyn crate::storage::DbClient>,
        namespace: impl Into<String>,
        model: Option<String>,
        dimension: Option<usize>,
    ) -> Self {
        let namespace = namespace.into();
        Self {
            db: BoundDbClient::new(Arc::clone(&db), namespace.clone()),
            backfill: crate::embedding::backfill_store::EmbeddingBackfillStoreClient::new(
                db, namespace,
            ),
            model,
            dimension,
        }
    }

    /// The embedding-only field set for this write.
    ///
    /// Built from scratch rather than cloned from the stored record: a snapshot
    /// of the record is what let a concurrent writer's field change be
    /// overwritten by the vector write.
    fn embedding_fields(
        &self,
        vector: Vec<f64>,
        identity: &VectorIdentity,
        at: DateTime<Utc>,
    ) -> Value {
        let mut fields = serde_json::Map::new();
        fields.insert("embedding".to_string(), Value::from(vector));
        fields.insert(
            "embedding_provider".to_string(),
            Value::from(identity.provider.clone()),
        );
        // Written explicitly even when absent, so a provider change cannot leave
        // the previous provider's model and dimension attached to this vector.
        fields.insert(
            "embedding_model".to_string(),
            match self.model.as_deref().or(identity.model.as_deref()) {
                Some(model) => Value::from(model),
                None => Value::Null,
            },
        );
        fields.insert(
            "embedding_dimension".to_string(),
            Value::from(self.dimension.unwrap_or(identity.dimension)),
        );
        fields.insert(
            "embedding_signature".to_string(),
            Value::from(identity.signature.clone()),
        );
        fields.insert(
            "embedding_updated_at".to_string(),
            Value::from(normalize_dt(at)),
        );
        Value::Object(fields)
    }
}

#[async_trait::async_trait]
impl CanonicalVectorPort for FactVectorAdapter {
    async fn stored_fact_vector(&self, fact_id: &str) -> Result<StoredVector, MemoryError> {
        let Some(Value::Object(record)) = self.db.select_one(fact_id).await? else {
            return Err(MemoryError::NotFound(format!(
                "fact not found for canonical vector read: {fact_id}"
            )));
        };
        if record.get("embedding").is_none_or(|value| value.is_null()) {
            return Ok(StoredVector::Absent);
        }
        Ok(StoredVector::Present {
            signature: record
                .get("embedding_signature")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        })
    }

    async fn apply_fact_vector(
        &self,
        fact_id: &str,
        vector: Vec<f64>,
        identity: VectorIdentity,
        at: DateTime<Utc>,
        policy: VectorWritePolicy,
    ) -> Result<VectorApplication, MemoryError> {
        // Which rows the predicate admits is the policy. The statement, not a
        // prior read, decides: a gap fill keeps any vector that already exists,
        // while a stale replacement also accepts a matching signature as done.
        let predicate = match policy {
            VectorWritePolicy::FillMissing => "embedding IS NONE",
            VectorWritePolicy::ReplaceStale => {
                "embedding IS NONE OR embedding_signature IS NONE \
                 OR embedding_signature != $embedding_signature"
            }
        };
        let fields = self.embedding_fields(vector, &identity, at);
        if self
            .backfill
            .apply_embedding_fields(fact_id, fields, predicate)
            .await?
        {
            return Ok(VectorApplication::Applied);
        }

        // The predicate refused. Distinguish "already has a vector" from "no
        // such fact", which is the one answer the predicate cannot express.
        if self.db.select_one(fact_id).await?.is_none() {
            return Err(MemoryError::NotFound(format!(
                "fact_id not found for canonical vector update: {fact_id}"
            )));
        }
        Ok(VectorApplication::AlreadyCurrent)
    }
}
