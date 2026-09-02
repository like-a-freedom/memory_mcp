//! Account deletion flow.
//!
//! 1. Recent OIDC reauthentication.
//! 2. Display notice: no export/recovery.
//! 3. Typed-phrase confirmation by the user.
//! 4. Server-issued short-lived one-use confirmation token.
//! 5. Durable credential/session revocation.
//! 6. Idempotent logical deletion job.

use std::sync::Arc;

use chrono::Utc;
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

use super::error::ApiError;
use super::recent_auth;
use super::session::ControlPlaneSession;
use crate::error::MemoryError;
use crate::http::registry::RegistryHandle;
use crate::http::registry::storage::RegistryStore;

/// The typed phrase the user must type to confirm deletion.
pub const DELETION_TYPED_PHRASE: &str = "DELETE my account";

/// Short-lived confirmation token for the deletion flow.
#[derive(Debug, Clone)]
pub struct DeletionConfirmationToken {
    pub account_id: String,
    pub session_id: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub used: bool,
}

impl DeletionConfirmationToken {
    pub fn new(account_id: &str, session_id: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            session_id: session_id.to_string(),
            expires_at: Utc::now() + chrono::Duration::minutes(5),
            used: false,
        }
    }

    pub fn is_valid(&self) -> bool {
        !self.used && self.expires_at > Utc::now()
    }
}

/// Validate that the typed phrase matches the expected deletion phrase.
pub fn validate_typed_phrase(phrase: &str) -> bool {
    phrase.trim() == DELETION_TYPED_PHRASE
}

/// Derive the durable verifier for a one-use confirmation token. The raw token
/// is returned only to the browser in the start response and is never persisted.
pub fn token_verifier(key: &[u8; 32], token: &str) -> Result<String, crate::error::MemoryError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|_| crate::error::MemoryError::ConfigInvalid("deletion token key".into()))?;
    mac.update(token.as_bytes());
    Ok(hex::encode(mac.finalize().into_bytes()))
}

/// Execute the safe deletion-start path. The caller supplies only the durable
/// challenge verifier; the store consumes the challenge and performs every
/// control-plane mutation in one transaction/critical section.
pub async fn execute_deletion(
    session: &ControlPlaneSession,
    typed_phrase: &str,
    challenge_verifier: &str,
    store: &dyn RegistryStore,
) -> Result<(), ApiError> {
    recent_auth::require_recent_auth(session, recent_auth::DEFAULT_REAUTH_MAX_AGE)?;
    if !validate_typed_phrase(typed_phrase) {
        return Err(ApiError::Forbidden);
    }
    store
        .begin_account_deletion(
            challenge_verifier,
            &session.account_id,
            &session.id,
            Utc::now(),
        )
        .await?;
    Ok(())
}

const DELETION_LEASE_TTL_SECS: i64 = 60;
const DELETION_BATCH_SIZE: usize = 64;
const APP_SESSION_CLEANUP_SQL: &str =
    "DELETE FROM app_session WHERE idle_expiry <= time::now() OR absolute_expiry <= time::now();";
const TASK_CLEANUP_SQL: &str = "DELETE FROM tenant_task WHERE retention_expiry <= time::now() AND state IN ['completed', 'completed_before_cancel', 'cancelled', 'cancelled_before_commit', 'failed'];";

/// Run one crash-safe deletion pass. The registry lease is the only worker
/// lease: it fences both tenant-local cleanup and the final tombstone update.
pub async fn run_deletion_worker(registry: RegistryHandle) -> Result<(), MemoryError> {
    let store = registry.store_clone();
    let tenants = store
        .list_deleting_tenants(DELETION_BATCH_SIZE, Utc::now())
        .await?;
    if tenants.is_empty() {
        return Ok(());
    }
    let engine = registry.tenant_engine()?;
    let mut first_error = None;
    let owner_id = crate::http::leases::scheduler::replica_id();

    for tenant in tenants {
        let lease_id = uuid::Uuid::new_v4().to_string();
        let Some(lease) = store
            .claim_provisioning(&tenant.id, &owner_id, &lease_id, DELETION_LEASE_TTL_SECS)
            .await?
        else {
            continue;
        };
        let tenant_id = tenant.id.clone();
        let tenant_id_for_work = tenant_id.clone();
        let namespace = tenant.namespace_binding.namespace.clone();
        let store_for_work = Arc::clone(&store);
        let engine_for_work = engine.clone();
        let lease_for_work = lease.clone();
        let cleanup = lease
            .run_with_heartbeat(registry.clone(), &tenant_id, async move {
                let client = engine_for_work.bind(&tenant).await?;
                match client
                    .execute_migration_script(APP_SESSION_CLEANUP_SQL, &namespace)
                    .await
                {
                    Ok(()) => {}
                    Err(error) if missing_app_session_table(&error) => {}
                    Err(error) => return Err(error),
                }
                match client
                    .execute_migration_script(TASK_CLEANUP_SQL, &namespace)
                    .await
                {
                    Ok(()) => {}
                    Err(error) if missing_task_table(&error) => {}
                    Err(error) => return Err(error),
                }
                store_for_work
                    .finalize_account_deletion(
                        &tenant_id_for_work,
                        &lease_for_work.owner_id,
                        &lease_for_work.lease_id,
                        lease_for_work.fencing_generation,
                        Utc::now(),
                    )
                    .await
            })
            .await;

        if let Err(error) = cleanup {
            let _ = lease.release(store.as_ref(), &tenant_id).await;
            if first_error.is_none() {
                first_error = Some(error);
            }
        }
    }

    first_error.map_or(Ok(()), Err)
}

fn missing_app_session_table(error: &MemoryError) -> bool {
    missing_table(error, "app_session")
}

fn missing_task_table(error: &MemoryError) -> bool {
    missing_table(error, "tenant_task")
}

fn missing_table(error: &MemoryError, table: &str) -> bool {
    let MemoryError::Storage(message) = error else {
        return false;
    };
    let lower = message.to_ascii_lowercase();
    lower.contains(table)
        && ((lower.contains("does not exist") && lower.contains("table"))
            || lower.contains("unknown table")
            || lower.contains("table not found"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_typed_phrase_exact() {
        assert!(validate_typed_phrase("DELETE my account"));
    }

    #[test]
    fn validate_typed_phrase_with_whitespace() {
        assert!(validate_typed_phrase("  DELETE my account  "));
    }

    #[test]
    fn validate_typed_phrase_wrong() {
        assert!(!validate_typed_phrase("delete my account"));
        assert!(!validate_typed_phrase("DELETE"));
        assert!(!validate_typed_phrase(""));
    }

    #[test]
    fn deletion_token_is_valid_initially() {
        let token = DeletionConfirmationToken::new("acc1", "sess1");
        assert!(token.is_valid());
    }

    #[test]
    fn deletion_token_expires() {
        let mut token = DeletionConfirmationToken::new("acc1", "sess1");
        token.expires_at = Utc::now() - chrono::Duration::minutes(1);
        assert!(!token.is_valid());
    }

    #[test]
    fn deletion_token_one_use() {
        let mut token = DeletionConfirmationToken::new("acc1", "sess1");
        assert!(token.is_valid());
        token.used = true;
        assert!(!token.is_valid());
    }
}
