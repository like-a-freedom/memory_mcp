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
use crate::embedding::api::{CanonicalVectorPort, StoredVector, VectorIdentity, VectorWritePolicy};
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
}

#[async_trait::async_trait]
impl CanonicalVectorPort for FactVectorAdapter {
    async fn stored_fact_vector(&self, fact_id: &str) -> Result<StoredVector, MemoryError> {
        let Some(Value::Object(record)) = self.db.select_one(fact_id).await? else {
            return Ok(StoredVector::Absent);
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
    ) -> Result<(), MemoryError> {
        let Some(Value::Object(mut record)) = self.db.select_one(fact_id).await? else {
            return Err(MemoryError::NotFound(format!(
                "fact_id not found for canonical vector update: {fact_id}"
            )));
        };
        record.insert("embedding".to_string(), Value::from(vector));
        record.insert(
            "embedding_provider".to_string(),
            Value::from(identity.provider.clone()),
        );
        if let Some(model) = self.model.as_deref().or(identity.model.as_deref()) {
            record.insert("embedding_model".to_string(), Value::from(model));
        }
        if let Some(dimension) = self.dimension.or(Some(identity.dimension)) {
            record.insert("embedding_dimension".to_string(), Value::from(dimension));
        }
        record.insert(
            "embedding_signature".to_string(),
            Value::from(identity.signature.clone()),
        );
        record.insert(
            "embedding_updated_at".to_string(),
            Value::from(normalize_dt(at)),
        );
        let payload = Value::Object(record);
        match policy {
            // A gap fill stays a compare-and-set in storage, so a
            // concurrent backfill cannot overwrite the winner.
            // The backfill store owns that conditional SQL.
            VectorWritePolicy::FillMissing => {
                let mut fields = payload.as_object().cloned().unwrap_or_default();
                fields.remove("fact_id");
                self.backfill
                    .update_embedding_fields(fact_id, Value::Object(fields))
                    .await
            }
            VectorWritePolicy::ReplaceStale => self
                .db
                .update(
                    fact_id,
                    payload,
                    crate::knowledge::queries::FACT_TEMPORAL_FIELDS,
                )
                .await
                .map(|_| ()),
        }
    }
}
