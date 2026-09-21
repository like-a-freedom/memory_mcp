//! Control-namespace storage trait.
//!
//! The trait surface ships the abstract API the control plane and
//! the provisioning worker depend on. Two implementations are
//! available:
//!
//! - `SurrealRegistryStore` is the production adapter; it issues
//!   SurrealQL against the privileged control namespace. The
//!   schema lives in `migrations.rs` and is applied through
//!   `SurrealTenantMigrations`.
//! - `InMemoryStore` is the test backend, gated on
//!   `test-fixtures`. Production startup never selects it; the
//!   `memory_mcp_http` composition root builds
//!   `SurrealRegistryStore` from validated environment
//!   configuration.
//!
//! Required methods (every capability) have no default body; the
//! four atomic operations (`create_account_bundle`,
//! `begin_account_deletion`, `begin_operator_deletion`,
//! `finalize_account_deletion`) remain single methods so the
//! application layer cannot reconstruct multi-row writes as
//! sequences.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "test-fixtures"))]
use std::sync::Mutex;

#[cfg(feature = "control-plane")]
use super::models::BrowserPolicyFence;
use super::models::*;
use crate::error::MemoryError;
#[cfg(feature = "control-plane")]
use crate::http::config::BrowserAuthMethod;
#[cfg(feature = "control-plane")]
use crate::service::local_admin::contracts::LocalKeyFingerprints;

/// Compact view of the lease fields the registry uses for
/// fenced CAS predicates. The `&str` borrows let callers pass
/// `&ProvisioningLease` without copying; the struct stays
/// `Copy` so the trait method can take it by value.
#[derive(Debug, Clone, Copy)]
pub struct LeaseFence<'a> {
    pub owner_id: &'a str,
    pub lease_id: &'a str,
    pub fencing_generation: u64,
}

impl<'a> LeaseFence<'a> {
    pub fn from_lease(lease: &'a crate::http::leases::ProvisioningLease) -> Self {
        Self {
            owner_id: &lease.owner_id,
            lease_id: &lease.lease_id,
            fencing_generation: lease.fencing_generation,
        }
    }
}

// ─── ensure_namespace ─────────────────────────────────────

/// Idempotent DDL: create the namespace and database if they
/// do not exist. Operates on a privileged `Surreal<C>` handle
/// held by the provisioning worker; never callable from an
/// ordinary tenant-bound credential.
pub async fn ensure_namespace<C>(
    privileged: &surrealdb::Surreal<C>,
    namespace: &str,
    database: &str,
) -> Result<(), MemoryError>
where
    C: surrealdb::Connection,
{
    if !is_safe_identifier(namespace) || !is_safe_identifier(database) {
        return Err(MemoryError::Validation(
            "namespace/database name must be server-generated tns_/db identifier".into(),
        ));
    }
    privileged
        .query(format!("DEFINE NAMESPACE IF NOT EXISTS `{namespace}`;"))
        .await
        .map_err(|err| MemoryError::Storage(format!("define namespace failed: {err}")))?;
    let bound = privileged.clone();
    bound
        .use_ns(namespace)
        .use_db(database)
        .await
        .map_err(|err| MemoryError::Storage(format!("bind for define database failed: {err}")))?;
    bound
        .query(format!("DEFINE DATABASE IF NOT EXISTS `{database}`;"))
        .await
        .map_err(|err| MemoryError::Storage(format!("define database failed: {err}")))?;
    Ok(())
}

/// Server-generated identifiers only: ascii alphanumerics and
/// underscore. Backtick-quoting alone prevents SQL injection,
/// but rejecting non-conforming names here is a defense in
/// depth.
pub fn is_safe_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Abstract control store. Backed by a privileged SurrealDB
/// credential in production; the `InMemoryStore` test backend is
/// the only non-test impl.
///
/// Methods are named after the records they touch; the SQL
/// implementation does not use the `DbClient` trait because the
/// `DbClient` trait is per-namespace and the registry is
/// multi-record across many tables.
#[async_trait]
pub trait RegistryStore: Send + Sync + 'static {
    async fn ping(&self) -> bool;

    async fn find_account_by_id(&self, account_id: &str) -> Result<Option<Account>, MemoryError>;
    /// `subject_verifier` is a keyed blind index; raw OIDC `sub`
    /// is never persisted.
    async fn find_account_by_identity(
        &self,
        issuer: &str,
        subject_verifier: &SubjectVerifier,
    ) -> Result<Option<Account>, MemoryError>;

    /// Atomically insert the Account, Tenant, and (optional)
    /// ExternalIdentity records that constitute a new tenant
    /// bundle. The implementation must enforce that the
    /// Account is unique, that the Tenant belongs to the
    /// Account, and that the identity tuple is unique when
    /// provided. `identity = None` is only valid for an
    /// operator-created invite account that will be linked
    /// through the authenticated linking flow.
    async fn create_account_bundle(
        &self,
        account: &Account,
        tenant: &Tenant,
        identity: Option<&ExternalIdentity>,
    ) -> Result<(), MemoryError>;

    async fn find_tenant_by_account(&self, account_id: &str)
    -> Result<Option<Tenant>, MemoryError>;
    async fn find_tenant_by_id(&self, tenant_id: &str) -> Result<Option<Tenant>, MemoryError>;

    /// List all external identities linked to `account_id`.
    /// Returns an empty Vec when the account has no linked
    /// identities (an invite account created without an
    /// identity has zero rows).
    async fn find_external_identities(
        &self,
        account_id: &str,
    ) -> Result<Vec<ExternalIdentity>, MemoryError>;

    /// Add an external identity to an account. Implementations
    /// must enforce that the (issuer, subject_verifier) tuple
    /// is unique and that the account exists, and must append the
    /// `identity_linked` audit row in the same transaction (ADR-0057),
    /// which is why `audit` is required rather than optional.
    async fn link_external_identity(
        &self,
        identity: &ExternalIdentity,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError>;

    /// Remove a linked identity by id, appending the `identity_unlinked`
    /// audit row in the same transaction.
    ///
    /// Refuses to remove an Account's last remaining identity (ADR-0057). The
    /// rule is enforced by the store rather than by the caller so that two
    /// concurrent removals cannot both observe two identities and both delete
    /// one.
    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError>;

    async fn find_api_key(&self, key_id: &str) -> Result<Option<ApiKey>, MemoryError>;
    async fn write_api_key(&self, key: &ApiKey) -> Result<(), MemoryError>;
    async fn list_api_keys(&self, account_id: &str) -> Result<Vec<ApiKeyMeta>, MemoryError>;
    async fn revoke_api_key(&self, account_id: &str, key_id: &str) -> Result<(), MemoryError>;
    async fn touch_api_key(&self, key_id: &str, used_at: DateTime<Utc>) -> Result<(), MemoryError>;

    /// Create an API key only when the account has fewer than
    /// `max_active` currently-active keys. Atomic.
    async fn create_api_key_if_below_limit(
        &self,
        key: &ApiKey,
        max_active: u32,
    ) -> Result<(), MemoryError>;

    /// Revoke every active key for an account; returns the
    /// number of keys revoked.
    async fn revoke_all_api_keys(&self, account_id: &str) -> Result<u64, MemoryError>;

    async fn write_account(&self, account: &Account) -> Result<(), MemoryError>;
    async fn write_tenant(&self, tenant: &Tenant) -> Result<(), MemoryError>;

    /// Transition an Account's status from `from` to `to`. The
    /// transition is conditional on the current status; a
    /// stale read returns `Conflict`.
    async fn transition_account_state(
        &self,
        account_id: &str,
        from: AccountStatus,
        to: AccountStatus,
    ) -> Result<(), MemoryError>;

    /// Atomically consume a valid deletion challenge, fence the account and
    /// tenant into their deleting states, revoke all API keys, and append the
    /// immutable deletion-start audit event. Control-plane sessions are
    /// deliberately retained; their account-status check denies them.
    #[cfg(feature = "control-plane")]
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;

    /// Start operator-initiated deletion without a user confirmation token.
    /// The same control-plane revocation and tombstone invariants apply.
    #[cfg(feature = "control-plane")]
    async fn begin_operator_deletion(
        &self,
        tenant_id: &str,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;

    /// Fenced, idempotent completion of a deletion pass. The account and
    /// tenant tombstones remain durable; only the tenant-local worker removes
    /// expired ephemeral rows before this method is called.
    #[cfg(feature = "control-plane")]
    async fn finalize_account_deletion(
        &self,
        tenant_id: &str,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        completed_at: DateTime<Utc>,
    ) -> Result<(), MemoryError>;

    /// CAS-update the tenant's status. The predicate is
    /// `version = $expected_version AND status = $from`. Returns
    /// the new version on success, `MemoryError::Conflict` on
    /// stale read.
    async fn update_tenant_state(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
    ) -> Result<u64, MemoryError>;

    /// Fenced CAS-update the tenant's status. The predicate
    /// adds `provisioning_lease.owner_id = $owner_id AND
    /// provisioning_lease.lease_id = $lease_id AND
    /// provisioning_lease.fencing_generation = $generation`
    /// to the unfenced CAS, so a stale worker cannot advance
    /// a tenant whose lease has been reassigned.
    async fn update_tenant_state_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
        lease: &LeaseFence<'_>,
    ) -> Result<u64, MemoryError>;

    /// Fenced CAS-update of the schema version. Predicate is
    /// `(version, status, owner, lease, generation)`. Returns
    /// the new version on success.
    async fn update_tenant_schema_version_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        new_schema_version: u32,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<u64, MemoryError>;

    /// Claim a provisioning lease for `tenant_id`. The
    /// implementation is responsible for fencing: if a prior
    /// lease is still active under a different owner the
    /// scheduler extends the generation and returns the new
    /// lease. Returns `None` if the tenant is already in a
    /// terminal state.
    async fn claim_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        lease_ttl_secs: i64,
    ) -> Result<Option<crate::http::leases::ProvisioningLease>, MemoryError>;

    /// Release a previously-claimed lease. CAS-clears the
    /// `provisioning_lease` field; only succeeds when the
    /// stored owner/lease/gen match the caller's.
    async fn release_provisioning_lease(
        &self,
        tenant_id: &str,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<(), MemoryError>;

    /// Heartbeat an active lease: extend `expires_at` and
    /// bump `heartbeat_at` if `(owner_id, lease_id,
    /// fencing_generation)` matches the stored row. Returns
    /// `Err(Conflict)` on a stale or missing lease.
    async fn heartbeat_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        heartbeat_at: chrono::DateTime<chrono::Utc>,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), MemoryError>;

    /// List tenants that are due for (re)provisioning. A
    /// tenant is due when it is in `Reserved`, `Migrating`,
    /// or `Suspended` AND its stored lease (if any) is
    /// expired. Limit caps the page size; the scheduler
    /// walks pages until the result is empty.
    async fn list_due_provisioning(
        &self,
        limit: usize,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::http::registry::models::Tenant>, MemoryError>;

    /// List tenants currently in `Ready` state, paginated by
    /// an opaque cursor (the tenant id of the last item in the
    /// previous page, or `None` for the first page).
    async fn list_ready_tenants(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::http::registry::models::Tenant>, MemoryError>;

    /// List tenants currently in `Deleting` state that are
    /// eligible for the deletion worker.
    async fn list_deleting_tenants(
        &self,
        limit: usize,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::http::registry::models::Tenant>, MemoryError>;

    /// Return a bounded page of every durable Tenant binding for reconciliation.
    async fn list_tenants(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::http::registry::models::Tenant>, MemoryError>;

    /// Append a provisioning event (durable seam consumed by the
    /// scheduler; written by `enqueue_provisioning`).
    async fn append_provisioning_event(
        &self,
        tenant_id: &str,
        stage: &str,
    ) -> Result<(), MemoryError>;

    /// Load the named Plan version. The durable default is the
    /// `Plan::default()` if no rows exist.
    async fn load_plan(&self, version: u32) -> Result<Plan, MemoryError>;

    /// Create the version-1 signup plan when it is absent. Existing durable
    /// plan rows are authoritative and must not be overwritten at startup.
    async fn ensure_plan(&self, plan: &Plan) -> Result<(), MemoryError>;

    /// Load the durable usage snapshot for a tenant. Returns
    /// an empty `UsageSnapshot` when no row exists.
    async fn load_usage(
        &self,
        tenant_id: &str,
    ) -> Result<crate::http::registry::plan::UsageCounter, MemoryError>;

    /// Reserve ingest usage against the tenant's plan. Returns
    /// `Allow`/`Deny` and atomically increments the counter
    /// when allowed.
    async fn reserve_ingest_usage(
        &self,
        tenant_id: &str,
        source_bytes: u64,
        plan: &crate::http::registry::plan::Plan,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::http::registry::plan::QuotaDecision, MemoryError>;

    /// Reconcile usage counters after drift detection.
    async fn reconcile_usage(
        &self,
        tenant_id: &str,
        expected: crate::http::registry::plan::UsageCounter,
    ) -> Result<(), MemoryError>;

    /// Store OIDC request sealed payload with explicit expiry
    /// and AEAD nonce. `policy` is the durable OIDC fence; the write is
    /// rejected unless the singleton mode is OIDC and the epoch matches.
    #[cfg(feature = "control-plane")]
    async fn store_oidc_request(
        &self,
        policy: &BrowserPolicyFence,
        state_hash: &str,
        sealed_payload: &[u8],
        aead_nonce: &[u8; 12],
    ) -> Result<(), MemoryError>;

    /// Atomically consume an OIDC request by state hash.
    /// Returns `None` if the state was already consumed or expired.
    /// Guarded by the OIDC policy mode/epoch.
    #[cfg(feature = "control-plane")]
    async fn take_oidc_request(
        &self,
        policy: &BrowserPolicyFence,
        state_hash: &str,
    ) -> Result<Option<(Vec<u8>, [u8; 12])>, MemoryError>;

    /// Store a control-plane session under the current OIDC policy epoch.
    #[cfg(feature = "control-plane")]
    async fn store_session(
        &self,
        policy: &BrowserPolicyFence,
        session: &crate::control::session::ControlPlaneSession,
    ) -> Result<(), MemoryError>;

    /// Find a session by keyed cookie hash. Excludes expired
    /// (idle/absolute) sessions and sessions from a different policy
    /// epoch (a missing or stale `browser_policy_epoch` never resolves).
    #[cfg(feature = "control-plane")]
    async fn find_session(
        &self,
        policy: &BrowserPolicyFence,
        cookie_hash: &str,
    ) -> Result<Option<crate::control::session::ControlPlaneSession>, MemoryError>;

    /// Refresh a session's idle deadline. The cookie hash binds the
    /// presented session and the idle expiry is computed from database
    /// time; the row is never recreated when missing or expired.
    #[cfg(feature = "control-plane")]
    async fn touch_session(
        &self,
        policy: &BrowserPolicyFence,
        session_id: &str,
        cookie_hash: &str,
    ) -> Result<(), MemoryError>;

    /// Delete one browser session by its keyed cookie hash under the
    /// current OIDC policy epoch.
    #[cfg(feature = "control-plane")]
    async fn delete_session(
        &self,
        policy: &BrowserPolicyFence,
        cookie_hash: &str,
    ) -> Result<(), MemoryError>;

    /// Persist a one-use deletion challenge keyed by a
    /// verifier; the raw token is never stored.
    #[cfg(feature = "control-plane")]
    async fn create_deletion_challenge(
        &self,
        challenge: &crate::http::registry::models::DeletionChallengeRecord,
    ) -> Result<(), MemoryError>;

    /// Create the named local plan when absent; if present, verify every
    /// limit field matches and fail with `'plan_limit_mismatch'` otherwise.
    /// Returns the durable plan.
    async fn ensure_local_plan(&self, plan: &Plan) -> Result<Plan, MemoryError>;

    /// Reconcile the durable browser-auth policy with the configured method
    /// set and return the resulting fence (ADR-0057).
    ///
    /// This is the single writer of `browser_auth_policy`. Creating the
    /// singleton is the only write an empty registry needs; reconciling an
    /// existing row is the *union* of what it holds and `desired`, because a
    /// row holding a method that `desired` omits is refused with
    /// [`MemoryError::Conflict`] rather than narrowed. Narrowing is the
    /// guarded removal operation, not a startup reconciliation.
    ///
    /// `local` carries the `local` method's key fingerprints and is `Some`
    /// exactly when `desired` enables `local`: a row that already enables it
    /// has its fingerprints verified, and adding it writes them.
    ///
    /// The stored set is written in canonical order
    /// ([`BrowserAuthMethod::canonical_set`]) so the row is one value however
    /// the caller enumerated the methods.
    #[cfg(feature = "control-plane")]
    async fn reconcile_browser_policy(
        &self,
        desired: &[BrowserAuthMethod],
        local: Option<LocalKeyFingerprints>,
    ) -> Result<BrowserPolicyFence, MemoryError>;

    /// Remove one method from the durable browser-auth policy and return the
    /// resulting fence (ADR-0057).
    ///
    /// This is the guarded removal operation, and the only path that can narrow
    /// the durable set: [`RegistryStore::reconcile_browser_policy`] adds methods
    /// and refuses to drop one, so a deployment reaches "SSO only" by calling
    /// this explicitly, never by editing the environment.
    ///
    /// The set left behind is the stored set minus `method`, computed here so a
    /// caller cannot name a result that disagrees with the row. It is refused
    /// when `method` is not enabled, and when removing it would leave no browser
    /// method at all — a control plane that authenticates nobody is not a
    /// deployment anyone can administer.
    ///
    /// The epoch advances, so every browser session of either method stops
    /// resolving: a change to the deployment's authentication surface is not
    /// something a session is kept across. An operator-action audit row naming
    /// the method is written in the same transaction.
    #[cfg(feature = "control-plane")]
    async fn remove_browser_auth_method(
        &self,
        method: BrowserAuthMethod,
    ) -> Result<BrowserPolicyFence, MemoryError>;

    /// Atomically create the account, tenant, and external identity under a
    /// matching browser auth policy fence. The policy mode/epoch are checked
    /// before any record is written.
    #[cfg(feature = "control-plane")]
    async fn create_oidc_account_bundle(
        &self,
        policy: &BrowserPolicyFence,
        account: &Account,
        tenant: &Tenant,
        identity: &ExternalIdentity,
    ) -> Result<(), MemoryError>;

    /// Atomically consume a deletion challenge by verifier,
    /// ensuring the same Account + session tuple match.
    /// Returns `Conflict` when the challenge is missing,
    /// expired, or already consumed.
    #[cfg(feature = "control-plane")]
    async fn consume_deletion_challenge(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), MemoryError>;
}

/// Canonical durable implementation. Kept available through this
/// module path for callers that imported the original storage seam.
pub use super::surreal_store::SurrealRegistryStore;

/// In-memory `RegistryStore` for unit tests. The fields are
/// behind a single `Mutex`; the contention is acceptable for
/// unit-test traffic. The struct is feature-gated on
/// `test-fixtures` so a production build cannot accidentally
/// swap it in.
#[cfg(any(test, feature = "test-fixtures"))]
pub struct InMemoryStore {
    accounts: std::sync::Mutex<Vec<Account>>,
    tenants: std::sync::Mutex<Vec<Tenant>>,
    api_keys: std::sync::Mutex<Vec<ApiKey>>,
    identities: std::sync::Mutex<Vec<ExternalIdentity>>,
    events: std::sync::Mutex<Vec<(String, String)>>,
    audit_events: std::sync::Mutex<Vec<ControlAuditEvent>>,
    usage: std::sync::Mutex<
        std::collections::HashMap<String, crate::http::registry::plan::UsageCounter>,
    >,
    plans: std::sync::Mutex<std::collections::HashMap<u32, Plan>>,
    #[cfg(feature = "control-plane")]
    oidc_requests: std::sync::Mutex<std::collections::HashMap<String, SealedOidcPayload>>,
    #[cfg(feature = "control-plane")]
    sessions: std::sync::Mutex<
        std::collections::HashMap<String, crate::control::session::ControlPlaneSession>,
    >,
    #[cfg(feature = "control-plane")]
    deletion_challenges: std::sync::Mutex<Vec<DeletionChallengeRecord>>,
    #[cfg(feature = "control-plane")]
    oidc_conflict: std::sync::Mutex<Option<AccountBundleConflict>>,
    #[cfg(feature = "control-plane")]
    browser_auth_policy: std::sync::Mutex<Option<BrowserPolicyFence>>,
}

/// Sealed OIDC payload: ciphertext + AEAD nonce.
#[cfg(all(feature = "control-plane", any(test, feature = "test-fixtures")))]
type SealedOidcPayload = (Vec<u8>, [u8; 12]);

#[cfg(all(feature = "control-plane", any(test, feature = "test-fixtures")))]
enum AccountBundleConflict {
    WithWinner(Box<(Account, Tenant, ExternalIdentity)>),
    WithoutWinner,
}

#[cfg(any(test, feature = "test-fixtures"))]
impl Default for InMemoryStore {
    fn default() -> Self {
        Self {
            accounts: Mutex::new(Vec::new()),
            tenants: Mutex::new(Vec::new()),
            api_keys: Mutex::new(Vec::new()),
            identities: Mutex::new(Vec::new()),
            events: Mutex::new(Vec::new()),
            audit_events: Mutex::new(Vec::new()),
            usage: Mutex::new(std::collections::HashMap::new()),
            plans: Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "control-plane")]
            oidc_requests: Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "control-plane")]
            sessions: Mutex::new(std::collections::HashMap::new()),
            #[cfg(feature = "control-plane")]
            deletion_challenges: Mutex::new(Vec::new()),
            #[cfg(feature = "control-plane")]
            oidc_conflict: Mutex::new(None),
            #[cfg(feature = "control-plane")]
            browser_auth_policy: Mutex::new(None),
        }
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl InMemoryStore {
    /// Inherent lockers. They centralize the "poisoned" recovery so
    /// the test backend can never silently panic on a stuck lock; the
    /// single `expect` text is also what the support tooling searches
    /// for when triaging a regression.
    fn lock_accounts(&self) -> std::sync::MutexGuard<'_, Vec<Account>> {
        self.accounts.lock().expect("poisoned")
    }
    fn lock_tenants(&self) -> std::sync::MutexGuard<'_, Vec<Tenant>> {
        self.tenants.lock().expect("poisoned")
    }
    fn lock_api_keys(&self) -> std::sync::MutexGuard<'_, Vec<ApiKey>> {
        self.api_keys.lock().expect("poisoned")
    }
    fn lock_identities(&self) -> std::sync::MutexGuard<'_, Vec<ExternalIdentity>> {
        self.identities.lock().expect("poisoned")
    }
    fn lock_audit_events(&self) -> std::sync::MutexGuard<'_, Vec<ControlAuditEvent>> {
        self.audit_events.lock().expect("poisoned")
    }
    fn lock_usage(
        &self,
    ) -> std::sync::MutexGuard<
        '_,
        std::collections::HashMap<String, crate::http::registry::plan::UsageCounter>,
    > {
        self.usage.lock().expect("poisoned")
    }
    #[cfg(feature = "control-plane")]
    fn lock_oidc_requests(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::HashMap<String, SealedOidcPayload>> {
        self.oidc_requests.lock().expect("poisoned")
    }
    #[cfg(feature = "control-plane")]
    fn lock_sessions(
        &self,
    ) -> std::sync::MutexGuard<
        '_,
        std::collections::HashMap<String, crate::control::session::ControlPlaneSession>,
    > {
        self.sessions.lock().expect("poisoned")
    }
    #[cfg(feature = "control-plane")]
    fn lock_deletion_challenges(&self) -> std::sync::MutexGuard<'_, Vec<DeletionChallengeRecord>> {
        self.deletion_challenges.lock().expect("poisoned")
    }
    #[cfg(feature = "control-plane")]
    fn lock_oidc_conflict(&self) -> std::sync::MutexGuard<'_, Option<AccountBundleConflict>> {
        self.oidc_conflict.lock().expect("poisoned")
    }

    #[cfg(feature = "control-plane")]
    fn lock_browser_policy(
        &self,
    ) -> std::sync::MutexGuard<'_, Option<super::models::BrowserPolicyFence>> {
        self.browser_auth_policy.lock().expect("poisoned")
    }

    /// Require the durable policy singleton to be OIDC and to match the
    /// caller's fence before an OIDC write or consumption proceeds.
    #[cfg(feature = "control-plane")]
    fn require_oidc_policy(
        &self,
        policy: &super::models::BrowserPolicyFence,
    ) -> Result<(), MemoryError> {
        let stored = self.lock_browser_policy();
        match stored.as_ref() {
            Some(existing)
                if existing.methods == policy.methods && existing.epoch == policy.epoch =>
            {
                Ok(())
            }
            _ => Err(MemoryError::Conflict("policy method/epoch mismatch".into())),
        }
    }

    #[cfg(feature = "control-plane")]
    pub fn inject_oidc_conflict(&self, winner: Option<(Account, Tenant, ExternalIdentity)>) {
        *self.lock_oidc_conflict() = Some(match winner {
            Some(bundle) => AccountBundleConflict::WithWinner(Box::new(bundle)),
            None => AccountBundleConflict::WithoutWinner,
        });
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
#[async_trait]
impl RegistryStore for InMemoryStore {
    async fn ping(&self) -> bool {
        true
    }
    async fn find_account_by_id(&self, id: &str) -> Result<Option<Account>, MemoryError> {
        Ok(self
            .accounts
            .lock()
            .expect("in-memory store poisoned")
            .iter()
            .find(|a| a.id == id)
            .cloned())
    }
    async fn find_account_by_identity(
        &self,
        issuer: &str,
        subject_verifier: &SubjectVerifier,
    ) -> Result<Option<Account>, MemoryError> {
        let account_id = {
            let identities = self.lock_identities();
            identities
                .iter()
                .find(|i| i.issuer == issuer && i.subject_verifier.0 == subject_verifier.0)
                .map(|i| i.account_id.clone())
        };
        let Some(account_id) = account_id else {
            return Ok(None);
        };
        self.find_account_by_id(&account_id).await
    }

    async fn create_account_bundle(
        &self,
        account: &Account,
        tenant: &Tenant,
        identity: Option<&ExternalIdentity>,
    ) -> Result<(), MemoryError> {
        if account.tenant_id != tenant.id {
            return Err(MemoryError::Validation(
                "account.tenant_id must equal tenant.id".into(),
            ));
        }
        if let Some(identity) = identity
            && identity.account_id != account.id
        {
            return Err(MemoryError::Validation(
                "identity.account_id must equal account.id".into(),
            ));
        }
        #[cfg(feature = "control-plane")]
        if let Some(conflict) = self.lock_oidc_conflict().take() {
            if let AccountBundleConflict::WithWinner(bundle) = conflict {
                let (account, tenant, identity) = *bundle;
                self.lock_accounts().push(account);
                self.lock_tenants().push(tenant);
                self.lock_identities().push(identity);
            }
            return Err(MemoryError::Conflict("injected OIDC create race".into()));
        }
        let mut accounts = self.lock_accounts();
        let mut tenants = self.lock_tenants();
        let mut identities = self.lock_identities();
        if accounts.iter().any(|a| a.id == account.id) {
            return Err(MemoryError::Conflict(format!(
                "account {} already exists",
                account.id
            )));
        }
        if tenants.iter().any(|t| t.id == tenant.id) {
            return Err(MemoryError::Conflict(format!(
                "tenant {} already exists",
                tenant.id
            )));
        }
        if tenants
            .iter()
            .any(|t| t.namespace_binding.namespace == tenant.namespace_binding.namespace)
        {
            return Err(MemoryError::Conflict(format!(
                "namespace {} is already bound",
                tenant.namespace_binding.namespace
            )));
        }
        if let Some(identity) = identity {
            if identities.iter().any(|item| item.id == identity.id)
                || identities.iter().any(|item| {
                    item.issuer == identity.issuer
                        && item.subject_verifier.0 == identity.subject_verifier.0
                })
            {
                return Err(MemoryError::Conflict("identity is already linked".into()));
            }
            identities.push(identity.clone());
        }
        accounts.push(account.clone());
        tenants.push(tenant.clone());
        Ok(())
    }

    async fn find_external_identities(
        &self,
        account_id: &str,
    ) -> Result<Vec<ExternalIdentity>, MemoryError> {
        Ok(self
            .identities
            .lock()
            .expect("poisoned")
            .iter()
            .filter(|i| i.account_id == account_id)
            .cloned()
            .collect())
    }

    async fn link_external_identity(
        &self,
        identity: &ExternalIdentity,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        let account_exists = self
            .accounts
            .lock()
            .expect("poisoned")
            .iter()
            .any(|account| account.id == identity.account_id);
        if !account_exists {
            return Err(MemoryError::NotFound(format!(
                "account {}",
                identity.account_id
            )));
        }
        let mut identities = self.lock_identities();
        if identities.iter().any(|i| i.id == identity.id) {
            return Err(MemoryError::Conflict(format!(
                "identity {} already exists",
                identity.id
            )));
        }
        if identities.iter().any(|i| {
            i.issuer == identity.issuer && i.subject_verifier.0 == identity.subject_verifier.0
        }) {
            return Err(MemoryError::Conflict(format!(
                "identity tuple ({}, *) already linked",
                identity.issuer
            )));
        }
        identities.push(identity.clone());
        drop(identities);
        self.lock_audit_events()
            .push(ControlAuditEvent::identity_change(
                IdentityAuditAction::Linked,
                &identity.id,
                &identity.account_id,
                audit,
            ));
        Ok(())
    }

    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
        audit: &IdentityAudit,
    ) -> Result<(), MemoryError> {
        let mut identities = self.lock_identities();
        let held = identities
            .iter()
            .filter(|i| i.account_id == account_id)
            .count();
        if held < 2 {
            return Err(MemoryError::Conflict(
                "the account's last identity cannot be unlinked".into(),
            ));
        }
        let before = identities.len();
        identities.retain(|i| !(i.account_id == account_id && i.id == identity_id));
        if identities.len() == before {
            return Err(MemoryError::NotFound(format!("identity {identity_id}")));
        }
        drop(identities);
        self.lock_audit_events()
            .push(ControlAuditEvent::identity_change(
                IdentityAuditAction::Unlinked,
                identity_id,
                account_id,
                audit,
            ));
        Ok(())
    }

    async fn create_api_key_if_below_limit(
        &self,
        key: &ApiKey,
        max_active: u32,
    ) -> Result<(), MemoryError> {
        let mut keys = self.lock_api_keys();
        let now = chrono::Utc::now();
        let active = keys
            .iter()
            .filter(|k| {
                k.account_id == key.account_id
                    && matches!(k.status, ApiKeyStatus::Active)
                    && k.expires_at.is_none_or(|expires_at| expires_at > now)
            })
            .count() as u32;
        if active >= max_active {
            return Err(MemoryError::Conflict(format!(
                "account {} reached max active api keys {max_active}",
                key.account_id
            )));
        }
        if keys.iter().any(|k| k.id == key.id) {
            return Err(MemoryError::Conflict(format!(
                "api key {} already exists",
                key.id
            )));
        }
        keys.push(key.clone());
        Ok(())
    }

    async fn revoke_all_api_keys(&self, account_id: &str) -> Result<u64, MemoryError> {
        let mut keys = self.lock_api_keys();
        let mut count = 0u64;
        for k in keys.iter_mut() {
            if k.account_id == account_id && matches!(k.status, ApiKeyStatus::Active) {
                k.status = ApiKeyStatus::Revoked;
                count += 1;
            }
        }
        Ok(count)
    }

    async fn transition_account_state(
        &self,
        account_id: &str,
        from: AccountStatus,
        to: AccountStatus,
    ) -> Result<(), MemoryError> {
        let mut accounts = self.lock_accounts();
        let a = accounts
            .iter_mut()
            .find(|a| a.id == account_id)
            .ok_or_else(|| MemoryError::NotFound(format!("account {account_id}")))?;
        if a.status != from {
            return Err(MemoryError::Conflict(format!(
                "account {account_id} state transition failed: {:?} (expected {:?})",
                a.status, from
            )));
        }
        if a.status == AccountStatus::Deleting && to != AccountStatus::Deleting {
            return Err(MemoryError::Conflict(format!(
                "account {account_id} deletion tombstone is immutable"
            )));
        }
        a.status = to;
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let mut challenges = self.lock_deletion_challenges();
        let challenge_index = challenges
            .iter()
            .position(|challenge| challenge.verifier == verifier)
            .ok_or_else(|| MemoryError::Conflict("deletion challenge is invalid".into()))?;
        let challenge = &challenges[challenge_index];
        if challenge.account_id != account_id || challenge.session_id != session_id {
            return Err(MemoryError::Conflict(
                "deletion challenge tuple mismatch".into(),
            ));
        }
        if challenge.consumed_at.is_some() || challenge.expires_at <= now {
            return Err(MemoryError::Conflict(
                "deletion challenge is invalid or expired".into(),
            ));
        }

        let mut accounts = self.lock_accounts();
        let account_index = accounts
            .iter()
            .position(|account| account.id == account_id)
            .ok_or_else(|| MemoryError::NotFound(format!("account {account_id}")))?;
        if accounts[account_index].status != AccountStatus::Active {
            return Err(MemoryError::Conflict("account is not active".into()));
        }
        let tenant_id = accounts[account_index].tenant_id.clone();

        let mut tenants = self.lock_tenants();
        let tenant_index = tenants
            .iter()
            .position(|tenant| tenant.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        if tenants[tenant_index].status == TenantStatus::Purged {
            return Err(MemoryError::Conflict(
                "tenant deletion tombstone is purged".into(),
            ));
        }

        let mut keys = self.lock_api_keys();
        let mut audit_events = self.lock_audit_events();
        let next_tenant_version = tenants[tenant_index]
            .version
            .checked_add(1)
            .ok_or_else(|| MemoryError::Conflict("tenant version overflow".into()))?;
        accounts[account_index].status = AccountStatus::Deleting;
        tenants[tenant_index].status = TenantStatus::Deleting;
        tenants[tenant_index].provisioning_lease = None;
        tenants[tenant_index].version = next_tenant_version;
        for key in keys.iter_mut() {
            if key.account_id == account_id && key.status == ApiKeyStatus::Active {
                key.status = ApiKeyStatus::Revoked;
            }
        }
        self.sessions
            .lock()
            .expect("poisoned")
            .retain(|_, session| session.account_id != account_id);
        challenges[challenge_index].consumed_at = Some(now);
        audit_events.push(ControlAuditEvent::for_account(
            account_id,
            AuditActorKind::Account,
            account_id,
            "account_deletion_started",
            now,
        ));
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn begin_operator_deletion(
        &self,
        tenant_id: &str,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let mut accounts = self.lock_accounts();
        let account_index = accounts
            .iter()
            .position(|account| account.tenant_id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("account for tenant {tenant_id}")))?;
        let account_id = accounts[account_index].id.clone();
        if accounts[account_index].status == AccountStatus::Deleting {
            return Ok(());
        }
        if accounts[account_index].status != AccountStatus::Active {
            return Err(MemoryError::Conflict("account is not active".into()));
        }
        let mut tenants = self.lock_tenants();
        let tenant_index = tenants
            .iter()
            .position(|tenant| tenant.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        if tenants[tenant_index].status == TenantStatus::Purged {
            return Err(MemoryError::Conflict(
                "tenant deletion tombstone is purged".into(),
            ));
        }
        let next_version = tenants[tenant_index]
            .version
            .checked_add(1)
            .ok_or_else(|| MemoryError::Conflict("tenant version overflow".into()))?;
        accounts[account_index].status = AccountStatus::Deleting;
        tenants[tenant_index].status = TenantStatus::Deleting;
        tenants[tenant_index].provisioning_lease = None;
        tenants[tenant_index].version = next_version;
        for key in self.lock_api_keys().iter_mut() {
            if key.account_id == account_id {
                key.status = ApiKeyStatus::Revoked;
            }
        }
        self.sessions
            .lock()
            .expect("poisoned")
            .retain(|_, session| session.account_id != account_id);
        let mut audit_events = self.lock_audit_events();
        if !audit_events.iter().any(|event| {
            event.account_id == account_id && event.action == "account_deletion_started_operator"
        }) {
            audit_events.push(ControlAuditEvent::for_account(
                &account_id,
                AuditActorKind::Operator,
                actor,
                "account_deletion_started_operator",
                now,
            ));
        }
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn finalize_account_deletion(
        &self,
        tenant_id: &str,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        completed_at: DateTime<Utc>,
    ) -> Result<(), MemoryError> {
        let mut tenants = self.lock_tenants();
        let tenant_index = tenants
            .iter()
            .position(|tenant| tenant.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        if tenants[tenant_index].status == TenantStatus::Purged {
            return Ok(());
        }
        if tenants[tenant_index].status != TenantStatus::Deleting {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} is not deleting"
            )));
        }
        let lease_matches = tenants[tenant_index]
            .provisioning_lease
            .as_ref()
            .is_some_and(|lease| {
                lease.owner_id == lease_owner_id
                    && lease.lease_id == lease_id
                    && lease.fencing_generation == fencing_generation
                    && lease.expires_at > Utc::now()
            });
        if !lease_matches {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} deletion lease is stale"
            )));
        }
        let accounts = self.lock_accounts();
        let account_id = accounts
            .iter()
            .find(|account| account.tenant_id == tenant_id)
            .map(|account| account.id.clone())
            .ok_or_else(|| MemoryError::NotFound(format!("account for tenant {tenant_id}")))?;
        let account = accounts
            .iter()
            .find(|account| account.id == account_id)
            .ok_or_else(|| MemoryError::NotFound(format!("account {account_id}")))?;
        if account.status != AccountStatus::Deleting {
            return Err(MemoryError::Conflict(format!(
                "account {account_id} is not deleting"
            )));
        }
        tenants[tenant_index].status = TenantStatus::Purged;
        tenants[tenant_index].provisioning_lease = None;
        tenants[tenant_index].version = tenants[tenant_index]
            .version
            .checked_add(1)
            .ok_or_else(|| MemoryError::Conflict("tenant version overflow".into()))?;
        let mut audit_events = self.lock_audit_events();
        audit_events.push(ControlAuditEvent::for_account(
            &account_id,
            AuditActorKind::System,
            lease_owner_id,
            "account_deletion_completed",
            completed_at,
        ));
        Ok(())
    }

    async fn find_tenant_by_account(
        &self,
        account_id: &str,
    ) -> Result<Option<Tenant>, MemoryError> {
        let account = self.find_account_by_id(account_id).await?;
        let Some(account) = account else {
            return Ok(None);
        };
        Ok(self
            .tenants
            .lock()
            .expect("in-memory store poisoned")
            .iter()
            .find(|t| t.id == account.tenant_id)
            .cloned())
    }
    async fn find_tenant_by_id(&self, id: &str) -> Result<Option<Tenant>, MemoryError> {
        Ok(self
            .tenants
            .lock()
            .expect("in-memory store poisoned")
            .iter()
            .find(|t| t.id == id)
            .cloned())
    }
    async fn find_api_key(&self, id: &str) -> Result<Option<ApiKey>, MemoryError> {
        Ok(self
            .api_keys
            .lock()
            .expect("in-memory store poisoned")
            .iter()
            .find(|k| k.id == id)
            .cloned())
    }
    async fn write_api_key(&self, key: &ApiKey) -> Result<(), MemoryError> {
        let mut keys = self.lock_api_keys();
        if keys.iter().any(|stored| stored.id == key.id) {
            return Err(MemoryError::Conflict(format!(
                "api key {} already exists",
                key.id
            )));
        }
        keys.push(key.clone());
        Ok(())
    }
    async fn list_api_keys(&self, account_id: &str) -> Result<Vec<ApiKeyMeta>, MemoryError> {
        Ok(self
            .api_keys
            .lock()
            .expect("in-memory store poisoned")
            .iter()
            .filter(|k| k.account_id == account_id)
            .map(|k| ApiKeyMeta {
                id: k.id.clone(),
                name: k.name.clone(),
                status: k.status,
                created_at: k.created_at,
                expires_at: k.expires_at,
                last_used_at: k.last_used_at,
            })
            .collect())
    }
    async fn revoke_api_key(&self, account_id: &str, key_id: &str) -> Result<(), MemoryError> {
        let mut keys = self.lock_api_keys();
        let k = keys
            .iter_mut()
            .find(|k| k.id == key_id && k.account_id == account_id)
            .ok_or_else(|| MemoryError::NotFound(format!("api key {key_id}")))?;
        if k.status == ApiKeyStatus::Revoked {
            return Err(MemoryError::Conflict(format!(
                "api key {key_id} is already revoked"
            )));
        }
        k.status = ApiKeyStatus::Revoked;
        k.version = k
            .version
            .checked_add(1)
            .ok_or_else(|| MemoryError::Conflict(format!("api key {key_id} version overflow")))?;
        Ok(())
    }
    async fn touch_api_key(&self, key_id: &str, used_at: DateTime<Utc>) -> Result<(), MemoryError> {
        let mut keys = self.lock_api_keys();
        if let Some(k) = keys.iter_mut().find(|k| {
            k.id == key_id
                && k.status == ApiKeyStatus::Active
                && k.expires_at.is_none_or(|expires_at| expires_at > used_at)
        }) {
            k.last_used_at = Some(used_at);
        }
        Ok(())
    }
    async fn write_account(&self, account: &Account) -> Result<(), MemoryError> {
        let mut accounts = self.lock_accounts();
        if let Some(slot) = accounts.iter_mut().find(|a| a.id == account.id) {
            if slot.status == AccountStatus::Deleting && account.status != AccountStatus::Deleting {
                return Err(MemoryError::Conflict(
                    "account deletion tombstone is immutable".into(),
                ));
            }
            *slot = account.clone();
        } else {
            accounts.push(account.clone());
        }
        Ok(())
    }
    async fn write_tenant(&self, tenant: &Tenant) -> Result<(), MemoryError> {
        let mut tenants = self.lock_tenants();
        if let Some(slot) = tenants.iter_mut().find(|t| t.id == tenant.id) {
            if slot.namespace_binding.namespace != tenant.namespace_binding.namespace
                || slot.namespace_binding.database != tenant.namespace_binding.database
            {
                return Err(MemoryError::Conflict(
                    "tenant namespace binding is immutable".into(),
                ));
            }
            if slot.status == TenantStatus::Purged && tenant.status != TenantStatus::Purged {
                return Err(MemoryError::Conflict(
                    "purged tenant tombstone is immutable".into(),
                ));
            }
            *slot = tenant.clone();
        } else if tenants.iter().any(|existing| {
            existing.namespace_binding.namespace == tenant.namespace_binding.namespace
        }) {
            return Err(MemoryError::Conflict(format!(
                "namespace {} is already bound",
                tenant.namespace_binding.namespace
            )));
        } else {
            tenants.push(tenant.clone());
        }
        Ok(())
    }
    async fn update_tenant_state(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
    ) -> Result<u64, MemoryError> {
        let mut tenants = self.lock_tenants();
        let t = tenants
            .iter_mut()
            .find(|t| t.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        if t.version != expected_version || t.status != from {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} CAS failed: version {} (expected {}) status {:?} (expected {:?})",
                t.version, expected_version, t.status, from
            )));
        }
        t.status = to;
        t.version = t
            .version
            .checked_add(1)
            .ok_or_else(|| MemoryError::Conflict(format!("tenant {tenant_id} version overflow")))?;
        Ok(t.version)
    }
    async fn update_tenant_state_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
        lease: &LeaseFence<'_>,
    ) -> Result<u64, MemoryError> {
        let mut tenants = self.lock_tenants();
        let t = tenants
            .iter_mut()
            .find(|t| t.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        if t.version != expected_version || t.status != from {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} CAS failed: version {} (expected {}) status {:?} (expected {:?})",
                t.version, expected_version, t.status, from
            )));
        }
        match &t.provisioning_lease {
            Some(stored)
                if stored.owner_id == lease.owner_id
                    && stored.lease_id == lease.lease_id
                    && stored.fencing_generation == lease.fencing_generation
                    && stored.expires_at > chrono::Utc::now() => {}
            Some(stored) => {
                return Err(MemoryError::Conflict(format!(
                    "tenant {tenant_id} fenced CAS failed: lease mismatch (got owner={} lease={} gen={}; expected owner={} lease={} gen={})",
                    stored.owner_id,
                    stored.lease_id,
                    stored.fencing_generation,
                    lease.owner_id,
                    lease.lease_id,
                    lease.fencing_generation,
                )));
            }
            None => {
                return Err(MemoryError::Conflict(format!(
                    "tenant {tenant_id} fenced CAS failed: no active lease"
                )));
            }
        }
        t.status = to;
        t.version = t
            .version
            .checked_add(1)
            .ok_or_else(|| MemoryError::Conflict(format!("tenant {tenant_id} version overflow")))?;
        Ok(t.version)
    }
    async fn update_tenant_schema_version_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        new_schema_version: u32,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<u64, MemoryError> {
        let mut tenants = self.lock_tenants();
        let t = tenants
            .iter_mut()
            .find(|t| t.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        if t.version != expected_version {
            return Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} schema-version CAS failed: version {} (expected {})",
                t.version, expected_version
            )));
        }
        match &t.provisioning_lease {
            Some(stored)
                if stored.owner_id == lease_owner_id
                    && stored.lease_id == lease_id
                    && stored.fencing_generation == fencing_generation
                    && stored.expires_at > chrono::Utc::now() => {}
            Some(stored) => {
                return Err(MemoryError::Conflict(format!(
                    "tenant {tenant_id} schema-version fenced CAS failed: lease mismatch (got owner={} lease={} gen={}; expected owner={} lease={} gen={})",
                    stored.owner_id,
                    stored.lease_id,
                    stored.fencing_generation,
                    lease_owner_id,
                    lease_id,
                    fencing_generation,
                )));
            }
            None => {
                return Err(MemoryError::Conflict(format!(
                    "tenant {tenant_id} schema-version fenced CAS failed: no active lease"
                )));
            }
        }
        t.schema_version = new_schema_version;
        t.version = t
            .version
            .checked_add(1)
            .ok_or_else(|| MemoryError::Conflict(format!("tenant {tenant_id} version overflow")))?;
        Ok(t.version)
    }
    async fn claim_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        lease_ttl_secs: i64,
    ) -> Result<Option<crate::http::leases::ProvisioningLease>, MemoryError> {
        use crate::http::registry::models::ProvisioningLeaseState;
        use crate::http::registry::models::TenantStatus as S;
        if lease_ttl_secs <= 0 {
            return Err(MemoryError::Validation(
                "provisioning lease TTL must be positive".into(),
            ));
        }
        let mut tenants = self.lock_tenants();
        let t = tenants
            .iter_mut()
            .find(|t| t.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        if matches!(t.status, S::Ready | S::Purged) {
            return Ok(None);
        }
        let now = chrono::Utc::now();
        let new_generation = match &t.provisioning_lease {
            // An active lease cannot be stolen before expiry.
            // Takeover is safe only after the datastore-time
            // lease expires, at which point the generation is
            // advanced so the prior holder becomes stale.
            Some(existing) if existing.expires_at > now => {
                return Err(MemoryError::Conflict(format!(
                    "tenant {tenant_id} provisioning lease is still active"
                )));
            }
            Some(existing) => existing.fencing_generation.checked_add(1).ok_or_else(|| {
                MemoryError::Conflict(format!(
                    "tenant {tenant_id} provisioning fence generation overflow"
                ))
            })?,
            // No lease: generation 1 is the initial fence.
            None => 1u64,
        };
        let lease = ProvisioningLeaseState {
            owner_id: owner_id.to_string(),
            lease_id: lease_id.to_string(),
            expires_at: now + chrono::Duration::seconds(lease_ttl_secs),
            fencing_generation: new_generation,
            heartbeat_at: now,
        };
        t.provisioning_lease = Some(lease.clone());
        t.version = t
            .version
            .checked_add(1)
            .ok_or_else(|| MemoryError::Conflict(format!("tenant {tenant_id} version overflow")))?;
        Ok(Some(crate::http::leases::ProvisioningLease {
            owner_id: lease.owner_id,
            lease_id: lease.lease_id,
            fencing_generation: lease.fencing_generation,
            expires_at: lease.expires_at,
            heartbeat_at: lease.heartbeat_at,
        }))
    }
    async fn release_provisioning_lease(
        &self,
        tenant_id: &str,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
    ) -> Result<(), MemoryError> {
        let mut tenants = self.lock_tenants();
        let t = tenants
            .iter_mut()
            .find(|t| t.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        match &t.provisioning_lease {
            Some(stored)
                if stored.owner_id == lease_owner_id
                    && stored.lease_id == lease_id
                    && stored.fencing_generation == fencing_generation =>
            {
                t.provisioning_lease = None;
                t.version = t.version.checked_add(1).ok_or_else(|| {
                    MemoryError::Conflict(format!("tenant {tenant_id} version overflow"))
                })?;
                Ok(())
            }
            _ => Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} release failed: lease mismatch"
            ))),
        }
    }
    async fn heartbeat_provisioning(
        &self,
        tenant_id: &str,
        owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        heartbeat_at: chrono::DateTime<chrono::Utc>,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), MemoryError> {
        let mut tenants = self.lock_tenants();
        let t = tenants
            .iter_mut()
            .find(|t| t.id == tenant_id)
            .ok_or_else(|| MemoryError::NotFound(format!("tenant {tenant_id}")))?;
        let now = chrono::Utc::now();
        let stored_matches = t
            .provisioning_lease
            .as_ref()
            .map(|stored| {
                stored.owner_id == owner_id
                    && stored.lease_id == lease_id
                    && stored.fencing_generation == fencing_generation
                    && stored.expires_at > now
            })
            .unwrap_or(false);
        if stored_matches {
            let stored = t.provisioning_lease.as_mut().expect("checked above");
            stored.heartbeat_at = heartbeat_at;
            stored.expires_at = expires_at;
            t.version = t.version.checked_add(1).ok_or_else(|| {
                MemoryError::Conflict(format!("tenant {tenant_id} version overflow"))
            })?;
            Ok(())
        } else {
            Err(MemoryError::Conflict(format!(
                "tenant {tenant_id} heartbeat failed: lease mismatch"
            )))
        }
    }
    async fn list_due_provisioning(
        &self,
        limit: usize,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::http::registry::models::Tenant>, MemoryError> {
        let tenants = self.lock_tenants();
        let mut out = Vec::new();
        for t in tenants.iter() {
            // Mirrors the durable query: `Suspended` is a terminal,
            // operator-chosen state and is not provisioning work, while
            // `NamespaceCreating` must stay resumable after a crash between
            // the two provisioning transitions.
            if !matches!(
                t.status,
                TenantStatus::Reserved
                    | TenantStatus::NamespaceCreating
                    | TenantStatus::Migrating
                    | TenantStatus::Failed
            ) {
                continue;
            }
            let lease_active = t
                .provisioning_lease
                .as_ref()
                .map(|l| l.expires_at > now)
                .unwrap_or(false);
            if lease_active {
                continue;
            }
            out.push(t.clone());
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    async fn list_ready_tenants(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<Vec<crate::http::registry::models::Tenant>, MemoryError> {
        let tenants = self.lock_tenants();
        let mut out = Vec::new();
        let mut started = cursor.is_none();
        for t in tenants.iter() {
            if !matches!(t.status, TenantStatus::Ready) {
                continue;
            }
            if !started {
                if Some(t.id.as_str()) == cursor {
                    started = true;
                }
                continue;
            }
            out.push(t.clone());
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    async fn list_deleting_tenants(
        &self,
        limit: usize,
        now: chrono::DateTime<Utc>,
    ) -> Result<Vec<crate::http::registry::models::Tenant>, MemoryError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let tenants = self.lock_tenants();
        let mut out = Vec::new();
        for t in tenants.iter() {
            if matches!(t.status, TenantStatus::Deleting)
                && t.provisioning_lease
                    .as_ref()
                    .is_none_or(|lease| lease.expires_at <= now)
            {
                out.push(t.clone());
            }
            if out.len() >= limit {
                break;
            }
        }
        Ok(out)
    }

    async fn list_tenants(
        &self,
        limit: usize,
    ) -> Result<Vec<crate::http::registry::models::Tenant>, MemoryError> {
        let tenants = self.lock_tenants();
        Ok(tenants.iter().take(limit).cloned().collect())
    }

    async fn load_plan(&self, version: u32) -> Result<Plan, MemoryError> {
        Ok(self
            .plans
            .lock()
            .expect("poisoned")
            .get(&version)
            .cloned()
            .unwrap_or_else(|| Plan {
                version,
                ..Plan::default()
            }))
    }

    async fn ensure_plan(&self, plan: &Plan) -> Result<(), MemoryError> {
        self.plans
            .lock()
            .expect("poisoned")
            .entry(plan.version)
            .or_insert_with(|| plan.clone());
        Ok(())
    }

    async fn load_usage(
        &self,
        tenant_id: &str,
    ) -> Result<crate::http::registry::plan::UsageCounter, MemoryError> {
        Ok(self
            .usage
            .lock()
            .expect("poisoned")
            .get(tenant_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn reserve_ingest_usage(
        &self,
        tenant_id: &str,
        source_bytes: u64,
        plan: &crate::http::registry::plan::Plan,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::http::registry::plan::QuotaDecision, MemoryError> {
        let mut usage = self.lock_usage();
        let counter = usage.entry(tenant_id.to_owned()).or_default();
        Ok(crate::http::registry::plan::enforce_ingest(
            plan,
            counter,
            source_bytes,
            now,
        ))
    }

    async fn reconcile_usage(
        &self,
        tenant_id: &str,
        expected: crate::http::registry::plan::UsageCounter,
    ) -> Result<(), MemoryError> {
        self.usage
            .lock()
            .expect("poisoned")
            .insert(tenant_id.to_owned(), expected);
        Ok(())
    }
    async fn append_provisioning_event(
        &self,
        tenant_id: &str,
        stage: &str,
    ) -> Result<(), MemoryError> {
        self.events
            .lock()
            .expect("in-memory store poisoned")
            .push((tenant_id.to_string(), stage.to_string()));
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn store_oidc_request(
        &self,
        policy: &BrowserPolicyFence,
        state_hash: &str,
        sealed_payload: &[u8],
        aead_nonce: &[u8; 12],
    ) -> Result<(), MemoryError> {
        self.require_oidc_policy(policy)?;
        let mut requests = self.lock_oidc_requests();
        if requests.contains_key(state_hash) {
            return Err(MemoryError::Conflict(
                "OIDC request state already exists".into(),
            ));
        }
        requests.insert(
            state_hash.to_string(),
            (sealed_payload.to_vec(), *aead_nonce),
        );
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn take_oidc_request(
        &self,
        policy: &BrowserPolicyFence,
        state_hash: &str,
    ) -> Result<Option<(Vec<u8>, [u8; 12])>, MemoryError> {
        self.require_oidc_policy(policy)?;
        Ok(self
            .oidc_requests
            .lock()
            .expect("poisoned")
            .remove(state_hash))
    }

    #[cfg(feature = "control-plane")]
    async fn store_session(
        &self,
        policy: &BrowserPolicyFence,
        session: &crate::control::session::ControlPlaneSession,
    ) -> Result<(), MemoryError> {
        self.require_oidc_policy(policy)?;
        if session.browser_policy_epoch != Some(policy.epoch) {
            return Err(MemoryError::Conflict(
                "session epoch does not match the durable policy".into(),
            ));
        }
        let mut sessions = self.lock_sessions();
        if sessions.contains_key(&session.cookie_hash)
            || sessions.values().any(|stored| stored.id == session.id)
        {
            return Err(MemoryError::Conflict("session already exists".into()));
        }
        sessions.insert(session.cookie_hash.clone(), session.clone());
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn find_session(
        &self,
        policy: &BrowserPolicyFence,
        cookie_hash: &str,
    ) -> Result<Option<crate::control::session::ControlPlaneSession>, MemoryError> {
        self.require_oidc_policy(policy)?;
        let now = chrono::Utc::now();
        Ok(self
            .sessions
            .lock()
            .expect("in-memory store poisoned")
            .get(cookie_hash)
            .filter(|session| {
                session.idle_expiry > now
                    && session.absolute_expiry > now
                    && session.browser_policy_epoch == Some(policy.epoch)
            })
            .cloned())
    }

    #[cfg(feature = "control-plane")]
    async fn touch_session(
        &self,
        policy: &BrowserPolicyFence,
        session_id: &str,
        cookie_hash: &str,
    ) -> Result<(), MemoryError> {
        self.require_oidc_policy(policy)?;
        let now = chrono::Utc::now();
        let mut sessions = self.lock_sessions();
        let updated = sessions
            .get_mut(cookie_hash)
            .filter(|s| s.id == session_id && s.browser_policy_epoch == Some(policy.epoch))
            .ok_or_else(|| MemoryError::NotFound(format!("session {session_id}")))?;
        if updated.absolute_expiry <= now || updated.idle_expiry <= now {
            return Err(MemoryError::Conflict(format!(
                "session {session_id} has expired"
            )));
        }
        let next_idle = (now + chrono::Duration::minutes(30)).min(updated.absolute_expiry);
        updated.idle_expiry = next_idle;
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn delete_session(
        &self,
        policy: &BrowserPolicyFence,
        cookie_hash: &str,
    ) -> Result<(), MemoryError> {
        self.require_oidc_policy(policy)?;
        let mut sessions = self.lock_sessions();
        let matches_epoch = sessions
            .get(cookie_hash)
            .is_some_and(|session| session.browser_policy_epoch == Some(policy.epoch));
        if !matches_epoch {
            return Err(MemoryError::NotFound("session not found".into()));
        }
        sessions
            .remove(cookie_hash)
            .map(|_| ())
            .ok_or_else(|| MemoryError::NotFound("session not found".into()))
    }

    #[cfg(feature = "control-plane")]
    async fn create_deletion_challenge(
        &self,
        challenge: &DeletionChallengeRecord,
    ) -> Result<(), MemoryError> {
        let mut challenges = self.lock_deletion_challenges();
        if challenges.iter().any(|c| c.verifier == challenge.verifier) {
            return Err(MemoryError::Conflict(
                "deletion challenge already exists".into(),
            ));
        }
        challenges.push(challenge.clone());
        Ok(())
    }

    #[cfg(feature = "control-plane")]
    async fn consume_deletion_challenge(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), MemoryError> {
        let mut challenges = self.lock_deletion_challenges();
        let c = challenges
            .iter_mut()
            .find(|c| c.verifier == verifier)
            .ok_or_else(|| MemoryError::NotFound("deletion challenge".into()))?;
        if c.account_id != account_id || c.session_id != session_id {
            return Err(MemoryError::Conflict(
                "deletion challenge tuple mismatch".into(),
            ));
        }
        if c.consumed_at.is_some() {
            return Err(MemoryError::Conflict(
                "deletion challenge already consumed".into(),
            ));
        }
        if c.expires_at <= now {
            return Err(MemoryError::Conflict("deletion challenge expired".into()));
        }
        c.consumed_at = Some(now);
        Ok(())
    }

    async fn ensure_local_plan(&self, plan: &Plan) -> Result<Plan, MemoryError> {
        let mut plans = self.plans.lock().expect("poisoned");
        let plan_id = format!("local_plan_v{}", plan.version);
        if let Some(existing) = plans.get(&plan.version).filter(|p| p.id == plan_id) {
            if existing.limits != plan.limits {
                return Err(MemoryError::Conflict("plan limit mismatch".into()));
            }
            Ok(existing.clone())
        } else {
            plans.insert(plan.version, plan.clone());
            Ok(plan.clone())
        }
    }

    #[cfg(feature = "control-plane")]
    async fn reconcile_browser_policy(
        &self,
        desired: &[crate::http::config::BrowserAuthMethod],
        local: Option<LocalKeyFingerprints>,
    ) -> Result<super::models::BrowserPolicyFence, MemoryError> {
        use crate::http::config::BrowserAuthMethod;
        // The in-memory backend stores no fingerprints: it has no `local`
        // surface, so `local` may only ever be absent here.
        if local.is_some() {
            return Err(MemoryError::Conflict(
                "the in-memory registry does not implement local fingerprints".into(),
            ));
        }
        let mut policy = self.lock_browser_policy();
        let desired = BrowserAuthMethod::canonical_set(desired);
        let fence = match policy.as_ref() {
            None => super::models::BrowserPolicyFence {
                methods: desired.clone(),
                epoch: 1,
            },
            Some(existing) => {
                if let Some(held) = existing.methods.iter().find(|m| !desired.contains(m)) {
                    return Err(MemoryError::Conflict(format!(
                        "browser authentication method '{}' is enabled durably but not in this \
                         configuration",
                        held.as_str()
                    )));
                }
                super::models::BrowserPolicyFence {
                    methods: desired.clone(),
                    epoch: existing.epoch,
                }
            }
        };
        *policy = Some(fence.clone());
        Ok(fence)
    }

    #[cfg(feature = "control-plane")]
    async fn remove_browser_auth_method(
        &self,
        method: crate::http::config::BrowserAuthMethod,
    ) -> Result<super::models::BrowserPolicyFence, MemoryError> {
        use crate::http::config::BrowserAuthMethod;
        let mut policy = self.lock_browser_policy();
        let Some(existing) = policy.as_mut() else {
            return Err(MemoryError::Conflict(
                "no durable browser-auth policy exists yet".into(),
            ));
        };
        if !existing.methods.contains(&method) {
            return Err(MemoryError::Conflict(format!(
                "browser authentication method '{}' is not enabled",
                method.as_str()
            )));
        }
        let remaining: Vec<BrowserAuthMethod> = existing
            .methods
            .iter()
            .copied()
            .filter(|held| *held != method)
            .collect();
        if remaining.is_empty() {
            return Err(MemoryError::Conflict(
                "removing this method would leave no browser authentication method".into(),
            ));
        }
        // The durable backend appends an operator-action audit row in
        // `local_admin_audit`, which the in-memory double does not model at all
        // (it holds no local-admin table), so only the policy change is mirrored.
        existing.methods = remaining;
        existing.epoch += 1;
        Ok(existing.clone())
    }

    #[cfg(feature = "control-plane")]
    async fn create_oidc_account_bundle(
        &self,
        policy: &super::models::BrowserPolicyFence,
        account: &Account,
        tenant: &Tenant,
        identity: &ExternalIdentity,
    ) -> Result<(), MemoryError> {
        {
            let stored = self.lock_browser_policy();
            match stored.as_ref() {
                Some(existing)
                    if existing.methods == policy.methods && existing.epoch == policy.epoch => {}
                _ => return Err(MemoryError::Conflict("policy method/epoch mismatch".into())),
            }
        }
        self.create_account_bundle(account, tenant, Some(identity))
            .await
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl InMemoryStore {
    pub fn provisioning_events(&self) -> Vec<(String, String)> {
        self.events
            .lock()
            .expect("in-memory store poisoned")
            .clone()
    }

    pub fn audit_events(&self) -> Vec<ControlAuditEvent> {
        self.audit_events
            .lock()
            .expect("in-memory store poisoned")
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trait_object_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<dyn RegistryStore>>();
    }

    #[tokio::test]
    async fn in_memory_store_round_trips_account_and_tenant() {
        use super::super::models::{AccountStatus, NamespaceBinding, TenantStatus};
        let s = InMemoryStore::default();
        let account = Account {
            id: "acct_1".into(),
            status: AccountStatus::Active,
            tenant_id: "ten_1".into(),
            created_at: chrono::Utc::now(),
        };
        let tenant = Tenant {
            id: "ten_1".into(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: "tns_x".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: chrono::Utc::now(),
            version: 0,
        };
        s.write_account(&account).await.unwrap();
        s.write_tenant(&tenant).await.unwrap();
        let got = s.find_account_by_id("acct_1").await.unwrap().unwrap();
        assert_eq!(got.id, "acct_1");
        let t = s.find_tenant_by_account("acct_1").await.unwrap().unwrap();
        assert_eq!(t.id, "ten_1");
    }

    #[tokio::test]
    async fn create_account_bundle_persists_all_three_records() {
        use super::super::models::{AccountStatus, NamespaceBinding, TenantStatus};
        let s = InMemoryStore::default();
        let account = Account {
            id: "acct_bundle_1".into(),
            status: AccountStatus::Active,
            tenant_id: "ten_bundle_1".into(),
            created_at: chrono::Utc::now(),
        };
        let tenant = Tenant {
            id: "ten_bundle_1".into(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: "tns_bundle".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: chrono::Utc::now(),
            version: 0,
        };
        let identity = ExternalIdentity {
            id: "idn_x".into(),
            issuer: "https://issuer".into(),
            subject_verifier: SubjectVerifier([0xAAu8; 32]),
            account_id: "acct_bundle_1".into(),
            created_at: chrono::Utc::now(),
        };
        s.create_account_bundle(&account, &tenant, Some(&identity))
            .await
            .unwrap();
        let found = s
            .find_account_by_identity("https://issuer", &SubjectVerifier([0xAAu8; 32]))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.id, "acct_bundle_1");
        let ids = s.find_external_identities("acct_bundle_1").await.unwrap();
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].issuer, "https://issuer");
    }

    #[tokio::test]
    async fn create_account_bundle_rejects_tenant_account_mismatch() {
        use super::super::models::{AccountStatus, NamespaceBinding, TenantStatus};
        let s = InMemoryStore::default();
        let account = Account {
            id: "acct_2".into(),
            status: AccountStatus::Active,
            tenant_id: "ten_other".into(),
            created_at: chrono::Utc::now(),
        };
        let tenant = Tenant {
            id: "ten_2".into(),
            status: TenantStatus::Reserved,
            namespace_binding: NamespaceBinding {
                namespace: "tns_x".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: chrono::Utc::now(),
            version: 0,
        };
        let res = s.create_account_bundle(&account, &tenant, None).await;
        assert!(matches!(res, Err(MemoryError::Validation(_))));
    }

    #[tokio::test]
    async fn link_external_identity_rejects_duplicate_tuple() {
        let s = InMemoryStore::default();
        for (id, tenant_id) in [("acct_a", "ten_a"), ("acct_b", "ten_b")] {
            s.write_account(&Account {
                id: id.into(),
                status: AccountStatus::Active,
                tenant_id: tenant_id.into(),
                created_at: chrono::Utc::now(),
            })
            .await
            .unwrap();
        }
        let sv = SubjectVerifier([0x42u8; 32]);
        let i1 = ExternalIdentity {
            id: "idn_a".into(),
            issuer: "https://issuer".into(),
            subject_verifier: sv.clone(),
            account_id: "acct_a".into(),
            created_at: chrono::Utc::now(),
        };
        let i2 = ExternalIdentity {
            id: "idn_b".into(),
            issuer: "https://issuer".into(),
            subject_verifier: sv,
            account_id: "acct_b".into(),
            created_at: chrono::Utc::now(),
        };
        s.link_external_identity(
            &i1,
            &IdentityAudit::by_account("acct_a", chrono::Utc::now()),
        )
        .await
        .unwrap();
        let res = s
            .link_external_identity(
                &i2,
                &IdentityAudit::by_account("acct_b", chrono::Utc::now()),
            )
            .await;
        assert!(matches!(res, Err(MemoryError::Conflict(_))));
    }

    /// ADR-0057: the in-memory double mirrors the guarded removal — the set
    /// narrows, the epoch advances, and the last method stays.
    #[tokio::test]
    #[cfg(feature = "control-plane")]
    async fn in_memory_removal_narrows_the_policy_and_advances_the_epoch() {
        use crate::http::config::BrowserAuthMethod;
        let s = InMemoryStore::default();
        s.reconcile_browser_policy(&[BrowserAuthMethod::Local, BrowserAuthMethod::Oidc], None)
            .await
            .expect("reconcile a dual-method policy");

        let fence = s
            .remove_browser_auth_method(BrowserAuthMethod::Local)
            .await
            .expect("remove the local method");
        assert_eq!(fence.methods, vec![BrowserAuthMethod::Oidc]);
        assert_eq!(fence.epoch, 2, "a policy change advances the epoch");

        // The whole point of the removal: the narrowed configuration now
        // reconciles instead of being refused.
        s.reconcile_browser_policy(&[BrowserAuthMethod::Oidc], None)
            .await
            .expect("an SSO-only configuration must be accepted after the removal");

        let refused = s.remove_browser_auth_method(BrowserAuthMethod::Oidc).await;
        assert!(
            matches!(refused, Err(MemoryError::Conflict(_))),
            "the last method must not be removable, got {refused:?}"
        );
        let absent = s.remove_browser_auth_method(BrowserAuthMethod::Local).await;
        assert!(
            matches!(absent, Err(MemoryError::Conflict(_))),
            "an absent method must be refused, got {absent:?}"
        );
    }

    #[tokio::test]
    async fn in_memory_plan_default_preserves_requested_version() {
        let s = InMemoryStore::default();
        let plan = s.load_plan(17).await.unwrap();
        assert_eq!(plan.id, "free");
        assert_eq!(plan.version, 17);
    }

    #[tokio::test]
    async fn api_key_ids_are_not_reusable() {
        let s = InMemoryStore::default();
        let key = ApiKey {
            id: "ak_reusable".into(),
            account_id: "acct_keys".into(),
            name: "first".into(),
            verifier: KeyedVerifier([1; 32]),
            status: ApiKeyStatus::Active,
            created_at: chrono::Utc::now(),
            expires_at: None,
            last_used_at: None,
            version: 0,
        };
        s.write_api_key(&key).await.unwrap();
        let duplicate = ApiKey {
            name: "replacement".into(),
            verifier: KeyedVerifier([2; 32]),
            ..key.clone()
        };
        let result = s.write_api_key(&duplicate).await;
        assert!(matches!(result, Err(MemoryError::Conflict(_))));
        assert_eq!(
            s.find_api_key(&key.id).await.unwrap().unwrap().name,
            "first"
        );
    }

    #[tokio::test]
    async fn create_api_key_if_below_limit_enforces_cap() {
        let s = InMemoryStore::default();
        let mut k1 = ApiKey {
            id: "ak_1".into(),
            account_id: "acct_x".into(),
            name: "k1".into(),
            verifier: KeyedVerifier([0u8; 32]),
            status: ApiKeyStatus::Active,
            created_at: chrono::Utc::now(),
            expires_at: None,
            last_used_at: None,
            version: 0,
        };
        s.create_api_key_if_below_limit(&k1, 1).await.unwrap();
        k1.id = "ak_2".into();
        let res = s.create_api_key_if_below_limit(&k1, 1).await;
        assert!(matches!(res, Err(MemoryError::Conflict(_))));
    }

    #[tokio::test]
    async fn transition_account_state_rejects_wrong_from() {
        use super::super::models::AccountStatus;
        let s = InMemoryStore::default();
        let account = Account {
            id: "acct_t".into(),
            status: AccountStatus::Active,
            tenant_id: "ten_t".into(),
            created_at: chrono::Utc::now(),
        };
        s.write_account(&account).await.unwrap();
        let res = s
            .transition_account_state("acct_t", AccountStatus::Suspended, AccountStatus::Deleting)
            .await;
        assert!(matches!(res, Err(MemoryError::Conflict(_))));
        let res = s
            .transition_account_state("acct_t", AccountStatus::Active, AccountStatus::Deleting)
            .await;
        assert!(res.is_ok());
        let again = s
            .transition_account_state("acct_t", AccountStatus::Active, AccountStatus::Deleting)
            .await;
        assert!(matches!(again, Err(MemoryError::Conflict(_))));
    }

    #[tokio::test]
    async fn revoke_all_api_keys_only_touches_active() {
        let s = InMemoryStore::default();
        let active = ApiKey {
            id: "ak_a".into(),
            account_id: "acct_k".into(),
            name: "active".into(),
            verifier: KeyedVerifier([0u8; 32]),
            status: ApiKeyStatus::Active,
            created_at: chrono::Utc::now(),
            expires_at: None,
            last_used_at: None,
            version: 0,
        };
        let revoked = ApiKey {
            id: "ak_r".into(),
            account_id: "acct_k".into(),
            name: "revoked".into(),
            verifier: KeyedVerifier([0u8; 32]),
            status: ApiKeyStatus::Revoked,
            created_at: chrono::Utc::now(),
            expires_at: None,
            last_used_at: None,
            version: 0,
        };
        s.write_api_key(&active).await.unwrap();
        s.write_api_key(&revoked).await.unwrap();
        let n = s.revoke_all_api_keys("acct_k").await.unwrap();
        assert_eq!(n, 1);
        let keys = s.list_api_keys("acct_k").await.unwrap();
        assert_eq!(keys.len(), 2);
        assert!(keys.iter().all(|k| k.status == ApiKeyStatus::Revoked));
    }

    #[tokio::test]
    async fn list_ready_tenants_pages_with_cursor() {
        use super::super::models::{AccountStatus, NamespaceBinding, TenantStatus};
        let s = InMemoryStore::default();
        for i in 0..5 {
            let t = Tenant {
                id: format!("ten_{i}"),
                status: TenantStatus::Ready,
                namespace_binding: NamespaceBinding {
                    namespace: format!("tns_{i}"),
                    database: "memory".into(),
                },
                plan_version: 1,
                schema_version: 0,
                retry_stage: None,
                provisioning_lease: None,
                created_at: chrono::Utc::now(),
                version: 0,
            };
            let a = Account {
                id: format!("acct_{i}"),
                status: AccountStatus::Active,
                tenant_id: t.id.clone(),
                created_at: chrono::Utc::now(),
            };
            s.write_account(&a).await.unwrap();
            s.write_tenant(&t).await.unwrap();
        }
        let first = s.list_ready_tenants(None, 2).await.unwrap();
        assert_eq!(first.len(), 2);
        let second = s
            .list_ready_tenants(Some(&first.last().unwrap().id), 2)
            .await
            .unwrap();
        assert_eq!(second.len(), 2);
        assert_ne!(first[0].id, second[0].id);
    }

    #[tokio::test]
    #[cfg(feature = "control-plane")]
    async fn deletion_challenge_consume_is_one_use() {
        let s = InMemoryStore::default();
        let now = chrono::Utc::now();
        let record = DeletionChallengeRecord {
            id: "del_1".into(),
            verifier: "verifier_x".into(),
            account_id: "acct_d".into(),
            session_id: "ses_d".into(),
            expires_at: now + chrono::Duration::seconds(60),
            consumed_at: None,
        };
        s.create_deletion_challenge(&record).await.unwrap();
        s.consume_deletion_challenge("verifier_x", "acct_d", "ses_d", now)
            .await
            .unwrap();
        let replay = s
            .consume_deletion_challenge("verifier_x", "acct_d", "ses_d", now)
            .await;
        assert!(matches!(replay, Err(MemoryError::Conflict(_))));
    }

    #[tokio::test]
    #[cfg(feature = "control-plane")]
    async fn deletion_challenge_rejects_expired() {
        let s = InMemoryStore::default();
        let past = chrono::Utc::now() - chrono::Duration::seconds(60);
        let record = DeletionChallengeRecord {
            id: "del_2".into(),
            verifier: "verifier_y".into(),
            account_id: "acct_d".into(),
            session_id: "ses_d".into(),
            expires_at: past,
            consumed_at: None,
        };
        s.create_deletion_challenge(&record).await.unwrap();
        let res = s
            .consume_deletion_challenge("verifier_y", "acct_d", "ses_d", chrono::Utc::now())
            .await;
        assert!(matches!(res, Err(MemoryError::Conflict(_))));
    }

    #[tokio::test]
    #[cfg(feature = "control-plane")]
    async fn deletion_challenge_rejects_account_mismatch() {
        let s = InMemoryStore::default();
        let now = chrono::Utc::now();
        let record = DeletionChallengeRecord {
            id: "del_3".into(),
            verifier: "verifier_z".into(),
            account_id: "acct_d".into(),
            session_id: "ses_d".into(),
            expires_at: now + chrono::Duration::seconds(60),
            consumed_at: None,
        };
        s.create_deletion_challenge(&record).await.unwrap();
        let res = s
            .consume_deletion_challenge("verifier_z", "acct_other", "ses_d", now)
            .await;
        assert!(matches!(res, Err(MemoryError::Conflict(_))));
    }

    #[tokio::test]
    #[cfg(feature = "control-plane")]
    async fn deletion_start_is_atomic_and_retains_control_records() {
        let s = InMemoryStore::default();
        let now = Utc::now();
        let policy = s
            .reconcile_browser_policy(&[BrowserAuthMethod::Oidc], None)
            .await
            .expect("reconcile browser policy");
        let account = Account {
            id: "acct_delete".into(),
            status: AccountStatus::Active,
            tenant_id: "ten_delete".into(),
            created_at: now,
        };
        let tenant = Tenant {
            id: "ten_delete".into(),
            status: TenantStatus::Ready,
            namespace_binding: NamespaceBinding {
                namespace: "tns_delete".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 1,
            retry_stage: None,
            provisioning_lease: None,
            created_at: now,
            version: 0,
        };
        s.write_account(&account).await.unwrap();
        s.write_tenant(&tenant).await.unwrap();
        s.write_api_key(&ApiKey {
            id: "ak_delete".into(),
            account_id: account.id.clone(),
            name: "key".into(),
            verifier: KeyedVerifier([1; 32]),
            status: ApiKeyStatus::Active,
            created_at: now,
            expires_at: None,
            last_used_at: None,
            version: 0,
        })
        .await
        .unwrap();
        s.link_external_identity(
            &ExternalIdentity {
                id: "idn_delete".into(),
                issuer: "https://issuer".into(),
                subject_verifier: SubjectVerifier([2; 32]),
                account_id: account.id.clone(),
                created_at: now,
            },
            &IdentityAudit::by_account(&account.id, now),
        )
        .await
        .unwrap();
        s.store_session(
            &policy,
            &crate::control::session::ControlPlaneSession {
                id: "ses_delete".into(),
                cookie_hash: "cookie_delete".into(),
                account_id: account.id.clone(),
                browser_policy_epoch: Some(policy.epoch),
                auth_time: now,
                idle_expiry: now + chrono::Duration::minutes(5),
                absolute_expiry: now + chrono::Duration::hours(1),
            },
        )
        .await
        .unwrap();
        s.create_deletion_challenge(&DeletionChallengeRecord {
            id: "del_atomic".into(),
            verifier: "verifier_atomic".into(),
            account_id: account.id.clone(),
            session_id: "ses_delete".into(),
            expires_at: now + chrono::Duration::minutes(5),
            consumed_at: None,
        })
        .await
        .unwrap();

        let invalid = s
            .begin_account_deletion("verifier_atomic", &account.id, "wrong_session", now)
            .await;
        assert!(matches!(invalid, Err(MemoryError::Conflict(_))));
        assert_eq!(
            s.find_account_by_id(&account.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            AccountStatus::Active
        );
        assert_eq!(
            s.find_tenant_by_id(&tenant.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            TenantStatus::Ready
        );
        assert_eq!(
            s.find_api_key("ak_delete").await.unwrap().unwrap().status,
            ApiKeyStatus::Active
        );

        s.begin_account_deletion("verifier_atomic", &account.id, "ses_delete", now)
            .await
            .unwrap();
        assert_eq!(
            s.find_account_by_id(&account.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            AccountStatus::Deleting
        );
        assert_eq!(
            s.find_tenant_by_id(&tenant.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            TenantStatus::Deleting
        );
        assert_eq!(
            s.find_api_key("ak_delete").await.unwrap().unwrap().status,
            ApiKeyStatus::Revoked
        );
        assert!(
            s.find_session(&policy, "cookie_delete")
                .await
                .expect("session lookup")
                .is_none(),
            "account deletion must revoke every browser session"
        );
        assert_eq!(
            s.find_external_identities(&account.id).await.unwrap().len(),
            1
        );
        let events = s.audit_events();
        assert_eq!(
            events
                .iter()
                .filter(|event| event.action == "account_deletion_started")
                .count(),
            1,
            "the deletion start must append exactly one row"
        );
        assert_eq!(events.len(), 2, "the earlier link keeps its own row");

        let replay = s
            .begin_account_deletion("verifier_atomic", &account.id, "ses_delete", now)
            .await;
        assert!(matches!(replay, Err(MemoryError::Conflict(_))));
    }

    #[tokio::test]
    #[cfg(feature = "control-plane")]
    async fn deletion_finalization_is_fenced_and_idempotent() {
        let s = InMemoryStore::default();
        let now = Utc::now();
        s.write_account(&Account {
            id: "acct_finalize".into(),
            status: AccountStatus::Deleting,
            tenant_id: "ten_finalize".into(),
            created_at: now,
        })
        .await
        .unwrap();
        s.write_tenant(&Tenant {
            id: "ten_finalize".into(),
            status: TenantStatus::Deleting,
            namespace_binding: NamespaceBinding {
                namespace: "tns_finalize".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 1,
            retry_stage: None,
            provisioning_lease: None,
            created_at: now,
            version: 0,
        })
        .await
        .unwrap();
        let lease = s
            .claim_provisioning("ten_finalize", "deletion-scheduler", "lease_finalize", 60)
            .await
            .unwrap()
            .unwrap();
        s.finalize_account_deletion(
            "ten_finalize",
            &lease.owner_id,
            &lease.lease_id,
            lease.fencing_generation,
            now,
        )
        .await
        .unwrap();
        assert_eq!(
            s.find_tenant_by_id("ten_finalize")
                .await
                .unwrap()
                .unwrap()
                .status,
            TenantStatus::Purged
        );
        assert_eq!(
            s.find_account_by_id("acct_finalize")
                .await
                .unwrap()
                .unwrap()
                .status,
            AccountStatus::Deleting
        );
        assert_eq!(s.audit_events().len(), 1);
        s.finalize_account_deletion(
            "ten_finalize",
            &lease.owner_id,
            &lease.lease_id,
            lease.fencing_generation,
            now,
        )
        .await
        .unwrap();
        assert_eq!(s.audit_events().len(), 1);
    }
}

// ─── ensure_namespace tests ───────────────────────────────

#[cfg(test)]
mod ensure_namespace_tests {
    use super::*;
    use surrealdb::Surreal;
    use surrealdb::engine::local::Mem;

    #[tokio::test]
    async fn ensure_namespace_is_idempotent() {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        ensure_namespace(&db, "ns_a", "db_a").await.unwrap();
        ensure_namespace(&db, "ns_a", "db_a").await.unwrap();
    }

    #[tokio::test]
    async fn ensure_namespace_rejects_non_server_generated_names() {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        let r = ensure_namespace(&db, "ns;drop", "db_a").await;
        assert!(matches!(r, Err(MemoryError::Validation(_))));
    }

    #[test]
    fn is_safe_identifier_accepts_tns_prefix() {
        assert!(is_safe_identifier("tns_abc123"));
        assert!(is_safe_identifier("memory"));
    }

    #[test]
    fn is_safe_identifier_rejects_injection_chars() {
        assert!(!is_safe_identifier("ns;drop"));
        assert!(!is_safe_identifier(""));
        assert!(!is_safe_identifier("with space"));
        assert!(!is_safe_identifier("with`backtick"));
    }
}
