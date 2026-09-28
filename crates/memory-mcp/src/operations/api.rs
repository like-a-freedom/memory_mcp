use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::MemoryError;
use crate::storage::client::BoundDbClient;

pub const DELETION_TYPED_PHRASE: &str = "DELETE my account";

/// Identity owns the recent-authentication window, so this context
/// reads that policy rather than defining a second copy of it. A
/// security constant with two owners is two values waiting to drift.
pub use crate::identity::api::RECENT_AUTH_MAX_AGE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BeginAccountDeletionCommand {
    pub account_id: String,
    pub session_id: String,
    pub authenticated_at: DateTime<Utc>,
    pub typed_phrase: String,
    pub challenge_verifier: String,
}

#[async_trait::async_trait]
pub trait AccountDeletionPort: Send + Sync {
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;
}

#[derive(Debug)]
pub enum OperationsError {
    ReauthenticationRequired,
    ConfirmationPhraseRejected,
    Persistence(MemoryError),
}

impl std::fmt::Display for OperationsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReauthenticationRequired => f.write_str("recent authentication required"),
            Self::ConfirmationPhraseRejected => {
                f.write_str("deletion confirmation phrase rejected")
            }
            Self::Persistence(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for OperationsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Persistence(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryOutcome {
    Finalized,
    Purged,
}

impl RecoveryOutcome {
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Purged)
    }
}

/// A tenant held for deletion recovery, as far as this module needs to know.
///
/// Deliberately smaller than the registry's `Tenant`: the recovery workflow
/// uses the id and the namespace binding and nothing else, so it asks for only
/// those. `status` is carried because "already purged" is a terminal outcome
/// the workflow must recognise before it acquires anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantUnderDeletion {
    pub tenant_id: String,
    pub namespace: String,
    pub database: String,
    pub purged: bool,
}

/// A deletion lease, fenced by generation.
#[derive(Debug, Clone)]
pub struct DeletionLease {
    pub owner_id: String,
    pub lease_id: String,
    pub fencing_generation: u64,
}

/// The tenant-namespace work that must be swept before a tenant's namespace
/// can be considered empty.
///
/// Owned by provisioning, which owns the `app_session` and `tenant_task`
/// tables. The operations workflow cannot state these deletes itself without
/// duplicating the two stores' own retention logic — which is what it used to
/// do, and how the copies drifted.
#[async_trait::async_trait]
pub trait RetainedTenantWork: Send + Sync {
    /// Delete expired App Sessions and retained Tenant Tasks in this tenant's
    /// namespace. Must tolerate a table that does not exist: a tenant that
    /// never opened an App Session has no `app_session` table.
    ///
    /// The tenant id is passed explicitly rather than read off the handle.
    /// A `BoundDbClient` pins the namespace but does not know which tenant
    /// owns it, and the `tenant_task` delete is scoped by tenant — so
    /// supplying it here keeps the sweep correct rather than namespace-wide.
    async fn purge_retained_work(
        &self,
        tenant_id: &str,
        bound_db: Arc<BoundDbClient>,
    ) -> Result<(), MemoryError>;
}

/// Persistence for the recovery workflow.
///
/// Each method is one durable step. The sequence, the fencing rules and the
/// failure policy live in [`recover_tenant`], not here, so the workflow can be
/// read and tested without a registry.
#[async_trait::async_trait]
pub trait DeletionRecoveryPort: Send + Sync {
    async fn list_deleting_tenants(
        &self,
        limit: usize,
        now: DateTime<Utc>,
    ) -> Result<Vec<String>, MemoryError>;

    /// Read the tenant under deletion, or `None` if it is gone entirely.
    async fn find_tenant_for_recovery(
        &self,
        tenant_id: &str,
    ) -> Result<Option<TenantUnderDeletion>, MemoryError>;

    /// Claim the deletion lease. `None` means another replica already holds a
    /// live lease, or the tenant reached a terminal state.
    async fn claim_deletion_lease(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        ttl_secs: i64,
    ) -> Result<Option<DeletionLease>, MemoryError>;

    /// Bind the tenant's namespace so its data can be swept. This is the only
    /// place a namespace enters the workflow, and it enters as a handle that
    /// cannot be re-pointed at another namespace.
    async fn bind_tenant_namespace(
        &self,
        tenant: &TenantUnderDeletion,
    ) -> Result<Arc<BoundDbClient>, MemoryError>;

    /// Commit the deletion tombstone under the lease fence.
    async fn write_deletion_tombstone(
        &self,
        tenant_id: &str,
        lease: &DeletionLease,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;

    /// Whether the tenant has reached its terminal purged state.
    async fn is_tenant_purged(&self, tenant_id: &str) -> Result<bool, MemoryError>;

    /// Release a lease this replica still holds. Fenced: a stale call is a
    /// no-op rather than an error, because the holder it was meant for has
    /// already been superseded.
    async fn release_deletion_lease(
        &self,
        tenant_id: &str,
        lease: &DeletionLease,
    ) -> Result<(), MemoryError>;
}

/// How long a deletion lease stays valid without a heartbeat.
pub const DELETION_LEASE_TTL_SECS: i64 = 60;

/// The fault point reached once a tenant's tombstone is durably committed.
///
/// Injected so a test can stop the workflow at the one step where the tenant
/// is already marked deleted but its namespace is not yet swept, and assert
/// that the next replica can still reclaim it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TombstoneCommitted;

/// Recover one tenant: sweep its retained work, then commit the tombstone.
///
/// The order matters. The tombstone is written last, so a replica that dies
/// part-way leaves the tenant still claimable: the work is idempotent, and the
/// lease simply expires. Writing it first would strand a tenant whose namespace
/// was never swept and whose lease nobody would try to reclaim, because the
/// state that would make it visible again no longer exists.
pub async fn recover_tenant(
    port: &(impl DeletionRecoveryPort + ?Sized),
    retained_work: &(impl RetainedTenantWork + ?Sized),
    tenant_id: &str,
    owner_id: &str,
    now: DateTime<Utc>,
) -> Result<RecoveryOutcome, MemoryError> {
    let Some(tenant) = port.find_tenant_for_recovery(tenant_id).await? else {
        return Ok(RecoveryOutcome::Purged);
    };
    if tenant.purged {
        return Ok(RecoveryOutcome::Purged);
    }

    let lease_id = uuid::Uuid::new_v4().to_string();
    let Some(lease) = port
        .claim_deletion_lease(tenant_id, owner_id, &lease_id, DELETION_LEASE_TTL_SECS)
        .await?
    else {
        // Another replica holds a live lease, or the tenant is already in a
        // terminal state. Either way this replica has nothing to do.
        return Ok(RecoveryOutcome::Finalized);
    };

    // Everything after the claim runs inside one guard. The lease is only
    // reclaimable once its TTL lapses, so any early return that skipped the
    // release would leave the tenant locked to this replica for a full minute
    // — after a failure that a retry could have fixed immediately. The old
    // shape leaked exactly that way when the namespace bind failed.
    let attempt = async {
        let bound_db = port.bind_tenant_namespace(&tenant).await?;
        retained_work
            .purge_retained_work(&tenant.tenant_id, bound_db)
            .await?;
        port.write_deletion_tombstone(tenant_id, &lease, now).await
    }
    .await;

    match attempt {
        Ok(()) => Ok(RecoveryOutcome::Finalized),
        Err(error) => {
            // A tombstone that landed before the failure is not a reason to
            // give up: the terminal state is the thing that matters, and
            // reporting it is what stops the next pass redoing the sweep.
            match port.is_tenant_purged(tenant_id).await {
                Ok(true) => Ok(RecoveryOutcome::Purged),
                // Re-checking is best-effort. If it fails there is no durable
                // answer to act on, so the original error is the one worth
                // surfacing rather than the re-check's.
                Ok(false) | Err(_) => {
                    // Fenced no-op when the lease was already superseded.
                    let _ = port.release_deletion_lease(tenant_id, &lease).await;
                    Err(error)
                }
            }
        }
    }
}

pub async fn run_deletion_recovery(
    port: &(impl DeletionRecoveryPort + ?Sized),
    retained_work: &(impl RetainedTenantWork + ?Sized),
    owner_id: &str,
    now: DateTime<Utc>,
) -> Result<(), MemoryError> {
    let tenants = port.list_deleting_tenants(64, now).await?;
    let mut first_error = None;
    for tenant_id in tenants {
        if let Err(error) = recover_tenant(port, retained_work, &tenant_id, owner_id, now).await
            && first_error.is_none()
        {
            first_error = Some(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}

pub async fn begin_account_deletion(
    port: &(impl AccountDeletionPort + ?Sized),
    command: &BeginAccountDeletionCommand,
    now: DateTime<Utc>,
) -> Result<(), OperationsError> {
    if now - command.authenticated_at
        > chrono::Duration::from_std(RECENT_AUTH_MAX_AGE).unwrap_or_default()
    {
        return Err(OperationsError::ReauthenticationRequired);
    }
    if command.typed_phrase.trim() != DELETION_TYPED_PHRASE {
        return Err(OperationsError::ConfirmationPhraseRejected);
    }
    port.begin_account_deletion(
        &command.challenge_verifier,
        &command.account_id,
        &command.session_id,
        now,
    )
    .await
    .map_err(OperationsError::Persistence)
}
