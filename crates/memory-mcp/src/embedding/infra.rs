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

/// Applies validated vectors to canonical fact records.
///
/// The stored field set mirrors the historical background
/// retry write: vector, provider, signature and update time.
/// Dimension and model are written only when the resolved
/// target reports them, so a runtime without that knowledge
/// stores exactly what it stored before.
pub struct FactVectorAdapter {
    backfill: crate::embedding::backfill_store::EmbeddingBackfillStoreClient,
}

impl FactVectorAdapter {
    pub fn new(db: Arc<dyn crate::storage::DbClient>, namespace: impl Into<String>) -> Self {
        let namespace = namespace.into();
        Self {
            backfill: crate::embedding::backfill_store::EmbeddingBackfillStoreClient::new(
                db, namespace,
            ),
        }
    }

    /// The embedding-only field set for this write.
    ///
    /// Built from the validated identity rather than constructor parameters.
    /// Duplicating model/dimension on the adapter lets a caller store metadata
    /// that disagrees with the signature and vector it just validated.
    fn embedding_fields(vector: Vec<f64>, identity: &VectorIdentity, at: DateTime<Utc>) -> Value {
        let mut fields = serde_json::Map::new();
        fields.insert("embedding".to_string(), Value::from(vector));
        fields.insert(
            "embedding_provider".to_string(),
            Value::from(identity.provider.clone()),
        );
        // Written explicitly even when absent, so a provider change cannot leave
        // the previous provider's model attached to this vector.
        fields.insert(
            "embedding_model".to_string(),
            match identity.model.as_deref() {
                Some(model) => Value::from(model),
                None => Value::Null,
            },
        );
        fields.insert(
            "embedding_dimension".to_string(),
            Value::from(identity.dimension),
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
        let Some(Value::Object(record)) = self.backfill.select_fact(fact_id).await? else {
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
        let fields = Self::embedding_fields(vector, &identity, at);
        if self
            .backfill
            .apply_embedding_fields(fact_id, fields, predicate)
            .await?
        {
            return Ok(VectorApplication::Applied);
        }

        // The predicate refused. Distinguish "already has a vector" from "no
        // such fact", which is the one answer the predicate cannot express.
        if self.backfill.select_fact(fact_id).await?.is_none() {
            return Err(MemoryError::NotFound(format!(
                "fact_id not found for canonical vector update: {fact_id}"
            )));
        }
        Ok(VectorApplication::AlreadyCurrent)
    }
}
