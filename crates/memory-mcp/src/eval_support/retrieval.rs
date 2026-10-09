use std::sync::Arc;

use crate::embedding::providers::EmbeddingProvider;
use crate::error::MemoryError;
use crate::service::MemoryService;
use crate::storage::DbClient;

/// Production-configured provider and its resolved deployment identity for a
/// retrieval-quality run. The identity contains no credentials.
pub struct EvalEmbeddingProvider {
    pub provider: Arc<dyn EmbeddingProvider>,
    pub provider_label: String,
    pub model: Option<String>,
    pub dimension: usize,
    pub signature: String,
    pub similarity_threshold: f64,
    pub index_signature: String,
    pub extractor_fingerprint: String,
}

/// Builds the same real provider used by service startup from the supplied
/// deployment configuration. A disabled provider is never accepted as a
/// substitute for a missing quality-evaluation model.
pub async fn create_eval_embedding_provider(
    config: &crate::config::EmbeddingConfig,
    data_dir: &str,
) -> Result<EvalEmbeddingProvider, MemoryError> {
    if !config.is_enabled() {
        return Err(MemoryError::ConfigInvalid(
            "retrieval quality evaluation requires a real embedding provider".into(),
        ));
    }

    let target =
        crate::embedding::providers::resolve_embedding_target_identity(config, data_dir).await?;
    let provider = crate::embedding::providers::create_embedding_provider_with_dimension(
        config,
        data_dir,
        target.dimension,
    )
    .await?;

    if !provider.is_enabled()
        || provider.provider_name() != target.provider_label
        || provider.dimension() != target.dimension
    {
        return Err(MemoryError::ConfigInvalid(
            "resolved retrieval quality provider does not match its model identity".into(),
        ));
    }

    Ok(EvalEmbeddingProvider {
        provider,
        provider_label: target.provider_label.to_string(),
        model: target.model,
        dimension: target.dimension,
        signature: target.signature,
        similarity_threshold: config.similarity_threshold,
        index_signature: eval_index_signature(target.dimension),
        extractor_fingerprint: eval_extractor_fingerprint()?,
    })
}

/// Fingerprints the production fact-vector index and query similarity contract.
/// This is evaluation metadata only; the production index remains owned by the
/// embedding context and is created through its normal re-embed lifecycle.
pub fn eval_index_signature(dimension: usize) -> String {
    use sha2::Digest;

    let definition = format!(
        "DEFINE INDEX {} ON TABLE fact FIELDS embedding HNSW DIMENSION {dimension}",
        crate::embedding::reembed_store::EMBEDDING_INDEX_NAME
    );
    let contract = format!("{definition}\nvector::similarity::cosine\nretrieval-query-contract-v1");
    format!(
        "indexsig:{}",
        hex::encode(sha2::Sha256::digest(contract.as_bytes()))
    )
}

/// Returns the identity of the native extractor used by the evaluation service
/// factory, through the extractor's own persisted fingerprint contract.
pub fn eval_extractor_fingerprint() -> Result<String, MemoryError> {
    use crate::knowledge::entity_extraction::EntityExtractor;

    let extractor = crate::knowledge::entity_extraction::AnnoEntityExtractor::new()?;
    Ok(extractor.fingerprint().as_str().to_owned())
}

pub struct EvalRetrievalConfig {
    pub embedding_provider: Arc<dyn EmbeddingProvider>,
    pub semantic_threshold: f64,
    pub claim_stage: String,
}

pub async fn new_eval_retrieval_service(
    db_client: Arc<dyn DbClient>,
    active_namespace: String,
    config: EvalRetrievalConfig,
) -> Result<MemoryService, MemoryError> {
    if !config.semantic_threshold.is_finite() || !(-1.0..=1.0).contains(&config.semantic_threshold)
    {
        return Err(MemoryError::ConfigInvalid(
            "embedding similarity threshold must be finite and within [-1, 1]".to_string(),
        ));
    }

    let active_namespace = crate::config::ActiveNamespace::new(active_namespace)?;
    let entity_extractor = Arc::new(crate::service::AnnoEntityExtractor::new()?);
    MemoryService::new_with_embedding_provider(
        db_client,
        active_namespace.as_str().to_string(),
        "warn".to_string(),
        50,
        100,
        config.embedding_provider,
        config.semantic_threshold,
        entity_extractor,
    )?
    .with_claim_rollout_stage(&config.claim_stage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn quality_provider_factory_rejects_the_disabled_provider() {
        let result = create_eval_embedding_provider(
            &crate::config::EmbeddingConfig::default(),
            "/tmp/eval-models",
        )
        .await;

        assert!(
            matches!(result, Err(MemoryError::ConfigInvalid(message)) if message.contains("real embedding provider"))
        );
    }
}
