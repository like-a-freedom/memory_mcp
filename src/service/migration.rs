//! Online embedding migration service.

use std::sync::Arc;
use std::time::Duration;

use crate::config::EmbeddingConfig;
use crate::logging::{LogLevel, StdoutLogger};
use crate::service::MemoryError;
use crate::service::embedding::EmbeddingProvider;
use crate::storage::{DbClient, EmbeddingSchema, EmbeddingStatus};

/// Active HNSW index name — must match `__Initial.surql`.
const ACTIVE_INDEX: &str = "fact_embedding_hnsw";

pub async fn run_if_needed(
    db: &dyn DbClient,
    provider: Arc<dyn EmbeddingProvider>,
    config: &EmbeddingConfig,
    ns: &str,
    logger: &StdoutLogger,
) -> Result<(), MemoryError> {
    if !config.is_enabled() {
        return Ok(());
    }

    let stored = db.get_embedding_schema(ns).await?;
    let config_schema = EmbeddingSchema::from_config(config);

    match &stored {
        None => {
            log_info(logger, "First startup - creating embedding schema");
            // Index already exists from __Initial.surql, just record schema
            db.set_embedding_schema(&config_schema, ns).await?;
            log_info(logger, "Embedding schema initialized");
            return Ok(());
        }
        Some(s) if s.active_matches_config(config) && s.status == EmbeddingStatus::Ready => {
            log_info(logger, "Embedding schema matches config, no action needed");
            return Ok(());
        }
        Some(s) if s.status == EmbeddingStatus::Cutover => {
            log_info(logger, "Deterrupted cutover detected, completing...");
            // Fall through — cutover will be completed below
        }
        Some(s) if s.target_matches_config(config) && s.status == EmbeddingStatus::Migrating => {
            log_info(logger, "Resuming interrupted migration");
        }
        Some(s) => {
            log_warn(
                logger,
                &format!(
                    "Embedding schema changed ({} {} dim={}) -> ({} {} dim={}). Starting migration.",
                    s.provider,
                    s.model,
                    s.dimension,
                    config_schema.provider,
                    config_schema.model,
                    config_schema.dimension
                ),
            );
            db.clear_next_embeddings(ns).await?;
            db.set_embedding_schema(&config_schema.as_migration_start(config), ns)
                .await?;
        }
    }

    // Phase 1: Backfill — no offset, same query each iteration
    if stored
        .as_ref()
        .is_none_or(|s| s.status != EmbeddingStatus::Cutover)
    {
        let batch_size = 50;
        let mut processed = 0usize;

        loop {
            let batch = db.get_facts_pending_reembed(batch_size, ns).await?;
            if batch.is_empty() {
                break;
            }

            for (id, content) in &batch {
                match provider.embed(content).await {
                    Ok(vec) => {
                        db.set_fact_next_embedding(id, vec, ns).await?;
                        processed += 1;
                    }
                    Err(e) => {
                        log_warn(logger, &format!("Failed to embed fact {id}: {e}"));
                    }
                }
            }

            if processed.is_multiple_of(500) {
                log_info(
                    logger,
                    &format!("Embedding migration: {processed} facts processed"),
                );
            }

            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        log_info(
            logger,
            &format!("Backfill complete: {processed} facts. Starting cutover..."),
        );
    }

    // Phase 2: Cutover — 3 sequential updates, no mixed embedding spaces
    let target_dim = config_schema.dimension;

    db.set_embedding_schema(&config_schema.with_status(EmbeddingStatus::Cutover), ns)
        .await?;
    db.drop_hnsw_index(ACTIVE_INDEX, ns).await?;
    db.apply_cutover(ns).await?;
    db.create_hnsw_index("embedding", ACTIVE_INDEX, target_dim, ns)
        .await?;
    db.set_embedding_schema(&config_schema.with_status(EmbeddingStatus::Ready), ns)
        .await?;

    log_info(logger, "Cutover complete. Running repair pass...");

    // Phase 3: Repair pass — fill in facts with embedding IS NONE
    let batch_size = 50;
    let mut repaired = 0usize;
    loop {
        let batch = db.get_facts_without_embedding(batch_size, ns).await?;
        if batch.is_empty() {
            break;
        }

        for (id, content) in &batch {
            match provider.embed(content).await {
                Ok(vec) => {
                    db.set_fact_embedding(id, vec, ns).await?;
                    repaired += 1;
                }
                Err(e) => {
                    log_warn(logger, &format!("Repair: failed to embed fact {id}: {e}"));
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    log_info(
        logger,
        &format!("Embedding migration completed. Repaired {repaired} facts."),
    );
    Ok(())
}

fn log_info(logger: &crate::logging::StdoutLogger, msg: &str) {
    logger.log(
        crate::log_event!("embedding.migration", "info", "message" => msg),
        LogLevel::Info,
    );
}

fn log_warn(logger: &crate::logging::StdoutLogger, msg: &str) {
    logger.log(
        crate::log_event!("embedding.migration", "warn", "message" => msg),
        LogLevel::Warn,
    );
}
