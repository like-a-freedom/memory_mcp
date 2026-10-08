//! HTTP composition of memory-owned lifecycle maintenance.
//!
//! This module translates deployment inputs into the narrow handles that the
//! memory use cases consume. It deliberately does not construct a
//! `MemoryService` or any extraction, embedding, or retrieval capabilities.

use std::sync::Arc;

use crate::config::LifecycleConfig;
use crate::knowledge::claims::ClaimStore;
use crate::logging::StdoutLogger;
use crate::memory::lifecycle_workers::{LifecycleHandles, LifecyclePolicy};
use crate::storage::DbClient;

/// Build the owner-scoped dependencies for one HTTP lifecycle pass.
pub(crate) fn lifecycle_handles<'a>(
    db: Arc<dyn DbClient>,
    namespace: &'a str,
    logger: &'a StdoutLogger,
    config: &LifecycleConfig,
    claim_store: Arc<dyn ClaimStore>,
) -> LifecycleHandles<'a> {
    LifecycleHandles {
        db_client: db,
        active_namespace: namespace,
        logger,
        policy: LifecyclePolicy::from(config),
        claim_store: Some(claim_store),
    }
}

/// Build lifecycle dependencies for passes that never retract claims.
pub(crate) fn lifecycle_handles_without_claim_store<'a>(
    db: Arc<dyn DbClient>,
    namespace: &'a str,
    logger: &'a StdoutLogger,
    config: &LifecycleConfig,
) -> LifecycleHandles<'a> {
    LifecycleHandles {
        db_client: db,
        active_namespace: namespace,
        logger,
        policy: LifecyclePolicy::from(config),
        claim_store: None,
    }
}
