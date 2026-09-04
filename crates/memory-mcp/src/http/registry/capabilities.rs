//! Capability-specific control Registry interfaces (ADR-0054).
//!
//! The eight `pub(crate)` traits here refine the omnibus
//! `RegistryStore` into per-concern seams so a consumer that
//! handles a single concern (auth, identity, OIDC, plan usage,
//! session refresh, deletion challenge) can ask for that
//! trait and nothing more. They are crate-private because the
//! public API of `memory_mcp_http` does not expose the
//! Registry; the split is an internal composition refinement.
//!
//! The required operations have no default implementation.
//! Atomic cross-row operations remain named methods and are
//! not reconstructed as application-layer sequences. The four
//! existing atomic operations (`create_account_bundle`,
//! `begin_account_deletion`, `begin_operator_deletion`,
//! `finalize_account_deletion`) live on `AccountIdentityStore`
//! and `AccountDeletionStore` as a single method each.
//!
//! The traits mirror the exact method signatures of
//! `RegistryStore` so an existing impl can be split into eight
//! per-adapter `impl` blocks without any method body changes.
//! The `RegistryStore` trait itself stays in `storage.rs` until
//! Task 10 removes it; this file is purely additive.
//!
//! ## Per-adapter impls
//!
//! Rust does not yet support stable trait upcasting
//! (`Arc<dyn Source> -> Arc<dyn Target>` when the two
//! `dyn`s are different traits; RFC 3324). The capability
//! aggregator therefore relies on each concrete adapter
//! implementing each capability explicitly:
//!
//! ```ignore
//! #[async_trait]
//! impl crate::http::registry::capabilities::RegistryHealth
//!     for SurrealRegistryStore { ... }
//! ```
//!
//! `RegistryStores::from_arc` then coerces the concrete
//! `Arc<SurrealRegistryStore>` (or `Arc<InMemoryStore>`) into
//! each `Arc<dyn Capability>` at construction time, where
//! the unsized coercion `Arc<T> -> Arc<dyn U>` is valid because
//! `T: U` is known at the call site. The aggregator clones the
//! `Arc` per capability; the same concrete allocation backs
//! all eight fields.
//!
//! The compile-time assertions
//! (`assert_registry_capabilities`) below pin the bound: a
//! concrete adapter that drops a capability will not
//! type-check.
//!
//! The whole module is marked `#[allow(dead_code)]` because
//! the trait methods have no callers in this milestone; Task 10
//! migrates `RegistryStore` consumers onto the capability
//! traits. The witness in the test module is enough to keep
//! the type system honest until that migration lands.

#![allow(dead_code)]

use async_trait::async_trait;
use chrono::{DateTime, Utc};

use super::models::*;
use super::storage::LeaseFence;
use crate::error::MemoryError;

// ─── RegistryHealth ─────────────────────────────────────────

/// Liveness probe. `ping` is the readiness gate every other
/// component relies on; it stays here so capability splits do
/// not accidentally drift the readiness semantics.
#[async_trait]
pub(crate) trait RegistryHealth: Send + Sync + 'static {
    async fn ping(&self) -> bool;
}

// ─── AccountIdentityStore ───────────────────────────────────

/// Account + Tenant + ExternalIdentity records. The atomic
/// `create_account_bundle` lives here as a single method.
#[async_trait]
pub(crate) trait AccountIdentityStore: Send + Sync + 'static {
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
    /// is unique and that the account exists.
    async fn link_external_identity(&self, identity: &ExternalIdentity) -> Result<(), MemoryError>;

    /// Remove a linked identity by id.
    async fn unlink_external_identity(
        &self,
        account_id: &str,
        identity_id: &str,
    ) -> Result<(), MemoryError>;

    async fn write_account(&self, account: &Account) -> Result<(), MemoryError>;
    async fn write_tenant(&self, tenant: &Tenant) -> Result<(), MemoryError>;
}

// ─── CredentialStore ────────────────────────────────────────

/// API-key CRUD plus the atomic
/// `create_api_key_if_below_limit` quota check. The quota
/// check is a single SQL `INSERT ... WHERE (SELECT COUNT(*) ...) < $max`
/// so a high-concurrency producer cannot insert past the cap
/// through a count-then-insert race.
#[async_trait]
pub(crate) trait CredentialStore: Send + Sync + 'static {
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
}

// ─── TenantProvisioningStore ────────────────────────────────

/// Tenant state machine + lease lifecycle + reconciliation
/// listings. The fenced CAS methods make a stale worker lose to
/// a fresh worker; the `claim_provisioning` and
/// `release_provisioning_lease` methods are the durable seam
/// for the lease lifecycle.
#[async_trait]
pub(crate) trait TenantProvisioningStore: Send + Sync + 'static {
    /// Transition an Account's status from `from` to `to`. The
    /// transition is conditional on the current status; a
    /// stale read returns `Conflict`.
    async fn transition_account_state(
        &self,
        account_id: &str,
        from: AccountStatus,
        to: AccountStatus,
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
}

// ─── PlanUsageStore ─────────────────────────────────────────

/// Plan CRUD and per-tenant usage reservation.
#[async_trait]
pub(crate) trait PlanUsageStore: Send + Sync + 'static {
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
}

// ─── OidcRequestStore (control-plane only) ──────────────────

/// OIDC request sealed payload with explicit expiry and AEAD
/// nonce. The atomic `take_oidc_request` enforces single-use
/// consumption of the in-flight state hash.
#[cfg(feature = "control-plane")]
#[async_trait]
pub(crate) trait OidcRequestStore: Send + Sync + 'static {
    /// Store OIDC request sealed payload with explicit expiry
    /// and AEAD nonce.
    async fn store_oidc_request(
        &self,
        state_hash: &str,
        sealed_payload: &[u8],
        aead_nonce: &[u8; 12],
    ) -> Result<(), MemoryError>;

    /// Atomically consume an OIDC request by state hash.
    /// Returns `None` if the state was already consumed or expired.
    async fn take_oidc_request(
        &self,
        state_hash: &str,
    ) -> Result<Option<(Vec<u8>, [u8; 12])>, MemoryError>;
}

// ─── ControlSessionStore (control-plane only) ───────────────

/// Server-side browser session for the control plane. The
/// keyed cookie hash is the only persisted identifier; the raw
/// cookie value is never stored.
#[cfg(feature = "control-plane")]
#[async_trait]
pub(crate) trait ControlSessionStore: Send + Sync + 'static {
    /// Store a control-plane session.
    async fn store_session(
        &self,
        session: &crate::control::session::ControlPlaneSession,
    ) -> Result<(), MemoryError>;

    /// Find a session by keyed cookie hash. Excludes expired
    /// (idle/absolute) sessions.
    async fn find_session(
        &self,
        cookie_hash: &str,
    ) -> Result<Option<crate::control::session::ControlPlaneSession>, MemoryError>;

    /// Update a session's idle_expiry timestamp atomically.
    async fn touch_session(
        &self,
        session_id: &str,
        idle_expiry: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), MemoryError>;

    /// Delete one browser session by its keyed cookie hash.
    async fn delete_session(&self, cookie_hash: &str) -> Result<(), MemoryError>;
}

// ─── AccountDeletionStore (control-plane only) ──────────────

/// Account deletion workflow. The four methods are atomic at
/// the storage layer; callers must not re-implement them as
/// application-level sequences.
#[cfg(feature = "control-plane")]
#[async_trait]
pub(crate) trait AccountDeletionStore: Send + Sync + 'static {
    /// Atomically consume a valid deletion challenge, fence the account and
    /// tenant into their deleting states, revoke all API keys, and append the
    /// immutable deletion-start audit event. Control-plane sessions are
    /// deliberately retained; their account-status check denies them.
    async fn begin_account_deletion(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;

    /// Start operator-initiated deletion without a user confirmation token.
    /// The same control-plane revocation and tombstone invariants apply.
    async fn begin_operator_deletion(
        &self,
        tenant_id: &str,
        actor: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;

    /// Fenced, idempotent completion of a deletion pass. The account and
    /// tenant tombstones remain durable; only the tenant-local worker removes
    /// expired ephemeral rows before this method is called.
    async fn finalize_account_deletion(
        &self,
        tenant_id: &str,
        lease_owner_id: &str,
        lease_id: &str,
        fencing_generation: u64,
        completed_at: DateTime<Utc>,
    ) -> Result<(), MemoryError>;

    /// Persist a one-use deletion challenge keyed by a
    /// verifier; the raw token is never stored.
    async fn create_deletion_challenge(
        &self,
        challenge: &crate::http::registry::models::DeletionChallengeRecord,
    ) -> Result<(), MemoryError>;

    /// Atomically consume a deletion challenge by verifier,
    /// ensuring the same Account + session tuple match.
    /// Returns `Conflict` when the challenge is missing,
    /// expired, or already consumed.
    async fn consume_deletion_challenge(
        &self,
        verifier: &str,
        account_id: &str,
        session_id: &str,
        now: DateTime<Utc>,
    ) -> Result<(), MemoryError>;
}

// ─── Compile-time capability assertions ─────────────────────

/// Compile-time proof that a concrete type implements every
/// always-on capability. The function body is empty; the bound
/// is the assertion. The `test-fixtures` adapter and the
/// production adapter both satisfy these bounds; the
/// assertion fires at the call site in
/// `RegistryStores::from_arc`.
#[allow(dead_code)]
pub(crate) fn assert_registry_capabilities<T>()
where
    T: RegistryHealth
        + AccountIdentityStore
        + CredentialStore
        + TenantProvisioningStore
        + PlanUsageStore
        + ?Sized,
{
}

/// Compile-time proof that a concrete type implements every
/// control-plane capability. Feature-gated to `control-plane`
/// because the underlying traits are too.
#[cfg(feature = "control-plane")]
#[allow(dead_code)]
pub(crate) fn assert_registry_control_plane_capabilities<T>()
where
    T: OidcRequestStore + ControlSessionStore + AccountDeletionStore + ?Sized,
{
}

#[cfg(test)]
mod tests {
    //! The capability traits are crate-private marker-style
    //! seams; the test module proves that a concrete adapter
    //! can satisfy them. The production adapters
    //! (`SurrealRegistryStore`, `InMemoryStore`) inherit the
    //! omnibus `RegistryStore` impl; the test adapter below
    //! implements every capability directly so the compile-time
    //! assertion has a concrete witness.
    //!
    //! The deferred `RegistryStores` aggregator
    //! (ADR-0054 stable-Rust note) cannot be built at
    //! runtime in stable Rust because trait upcasting is
    //! not yet stable. The capability traits are the durable
    //! deliverable for this milestone; the aggregator lands
    //! when the upstream `coerce_unsized`-equivalent helper
    //! becomes available.

    use super::*;
    use crate::http::leases::ProvisioningLease;
    use crate::http::registry::plan::UsageCounter;
    use async_trait::async_trait;

    /// A minimal adapter that implements every capability
    /// directly. The bodies are empty; the goal is to prove
    /// the trait bounds compile and can be checked at
    /// the aggregator's construction site.
    struct CapabilityWitness;

    #[async_trait]
    impl RegistryHealth for CapabilityWitness {
        async fn ping(&self) -> bool {
            true
        }
    }

    #[async_trait]
    impl AccountIdentityStore for CapabilityWitness {
        async fn find_account_by_id(&self, _: &str) -> Result<Option<Account>, MemoryError> {
            Ok(None)
        }
        async fn find_account_by_identity(
            &self,
            _: &str,
            _: &SubjectVerifier,
        ) -> Result<Option<Account>, MemoryError> {
            Ok(None)
        }
        async fn create_account_bundle(
            &self,
            _: &Account,
            _: &Tenant,
            _: Option<&ExternalIdentity>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn find_tenant_by_account(&self, _: &str) -> Result<Option<Tenant>, MemoryError> {
            Ok(None)
        }
        async fn find_tenant_by_id(&self, _: &str) -> Result<Option<Tenant>, MemoryError> {
            Ok(None)
        }
        async fn find_external_identities(
            &self,
            _: &str,
        ) -> Result<Vec<ExternalIdentity>, MemoryError> {
            Ok(Vec::new())
        }
        async fn link_external_identity(&self, _: &ExternalIdentity) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn unlink_external_identity(&self, _: &str, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn write_account(&self, _: &Account) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn write_tenant(&self, _: &Tenant) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[async_trait]
    impl CredentialStore for CapabilityWitness {
        async fn find_api_key(&self, _: &str) -> Result<Option<ApiKey>, MemoryError> {
            Ok(None)
        }
        async fn write_api_key(&self, _: &ApiKey) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn list_api_keys(&self, _: &str) -> Result<Vec<ApiKeyMeta>, MemoryError> {
            Ok(Vec::new())
        }
        async fn revoke_api_key(&self, _: &str, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn touch_api_key(&self, _: &str, _: DateTime<Utc>) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn create_api_key_if_below_limit(
            &self,
            _: &ApiKey,
            _: u32,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn revoke_all_api_keys(&self, _: &str) -> Result<u64, MemoryError> {
            Ok(0)
        }
    }

    #[async_trait]
    impl TenantProvisioningStore for CapabilityWitness {
        async fn transition_account_state(
            &self,
            _: &str,
            _: AccountStatus,
            _: AccountStatus,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn update_tenant_state(
            &self,
            _: &str,
            _: u64,
            _: TenantStatus,
            _: TenantStatus,
        ) -> Result<u64, MemoryError> {
            Ok(0)
        }
        async fn update_tenant_state_fenced(
            &self,
            _: &str,
            _: u64,
            _: TenantStatus,
            _: TenantStatus,
            _: &LeaseFence<'_>,
        ) -> Result<u64, MemoryError> {
            Ok(0)
        }
        async fn update_tenant_schema_version_fenced(
            &self,
            _: &str,
            _: u64,
            _: u32,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<u64, MemoryError> {
            Ok(0)
        }
        async fn claim_provisioning(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: i64,
        ) -> Result<Option<ProvisioningLease>, MemoryError> {
            Ok(None)
        }
        async fn release_provisioning_lease(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn heartbeat_provisioning(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
            _: DateTime<Utc>,
            _: DateTime<Utc>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn list_due_provisioning(
            &self,
            _: usize,
            _: DateTime<Utc>,
        ) -> Result<Vec<Tenant>, MemoryError> {
            Ok(Vec::new())
        }
        async fn list_ready_tenants(
            &self,
            _: Option<&str>,
            _: usize,
        ) -> Result<Vec<Tenant>, MemoryError> {
            Ok(Vec::new())
        }
        async fn list_deleting_tenants(
            &self,
            _: usize,
            _: DateTime<Utc>,
        ) -> Result<Vec<Tenant>, MemoryError> {
            Ok(Vec::new())
        }
        async fn list_tenants(&self, _: usize) -> Result<Vec<Tenant>, MemoryError> {
            Ok(Vec::new())
        }
        async fn append_provisioning_event(&self, _: &str, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[async_trait]
    impl PlanUsageStore for CapabilityWitness {
        async fn load_plan(&self, _: u32) -> Result<Plan, MemoryError> {
            Ok(Plan::default())
        }
        async fn ensure_plan(&self, _: &Plan) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn load_usage(&self, _: &str) -> Result<UsageCounter, MemoryError> {
            Ok(UsageCounter::default())
        }
        async fn reserve_ingest_usage(
            &self,
            _: &str,
            _: u64,
            _: &crate::http::registry::plan::Plan,
            _: DateTime<Utc>,
        ) -> Result<crate::http::registry::plan::QuotaDecision, MemoryError> {
            Ok(crate::http::registry::plan::QuotaDecision::Allow)
        }
        async fn reconcile_usage(&self, _: &str, _: UsageCounter) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[cfg(feature = "control-plane")]
    #[async_trait]
    impl OidcRequestStore for CapabilityWitness {
        async fn store_oidc_request(
            &self,
            _: &str,
            _: &[u8],
            _: &[u8; 12],
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn take_oidc_request(
            &self,
            _: &str,
        ) -> Result<Option<(Vec<u8>, [u8; 12])>, MemoryError> {
            Ok(None)
        }
    }

    #[cfg(feature = "control-plane")]
    #[async_trait]
    impl ControlSessionStore for CapabilityWitness {
        async fn store_session(
            &self,
            _: &crate::control::session::ControlPlaneSession,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn find_session(
            &self,
            _: &str,
        ) -> Result<Option<crate::control::session::ControlPlaneSession>, MemoryError> {
            Ok(None)
        }
        async fn touch_session(&self, _: &str, _: DateTime<Utc>) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn delete_session(&self, _: &str) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    #[cfg(feature = "control-plane")]
    #[async_trait]
    impl AccountDeletionStore for CapabilityWitness {
        async fn begin_account_deletion(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: DateTime<Utc>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn begin_operator_deletion(
            &self,
            _: &str,
            _: &str,
            _: DateTime<Utc>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn finalize_account_deletion(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: u64,
            _: DateTime<Utc>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn create_deletion_challenge(
            &self,
            _: &DeletionChallengeRecord,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
        async fn consume_deletion_challenge(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: DateTime<Utc>,
        ) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    /// The compile-time assertion fires here: if a capability
    /// method drifts (signature change, missing impl on the
    /// witness), this call site fails to type-check.
    #[test]
    fn capability_witness_satisfies_all_bounds() {
        assert_registry_capabilities::<CapabilityWitness>();
        #[cfg(feature = "control-plane")]
        assert_registry_control_plane_capabilities::<CapabilityWitness>();
        // A roundtrip through the trait objects proves the
        // vtable shape compiles. The witness is never used
        // at runtime, but the trait-object references are
        // enough to keep the bound honest.
        let _witness: Box<dyn RegistryHealth> = Box::new(CapabilityWitness);
        let _witness: Box<dyn AccountIdentityStore> = Box::new(CapabilityWitness);
        let _witness: Box<dyn CredentialStore> = Box::new(CapabilityWitness);
        let _witness: Box<dyn TenantProvisioningStore> = Box::new(CapabilityWitness);
        let _witness: Box<dyn PlanUsageStore> = Box::new(CapabilityWitness);
        #[cfg(feature = "control-plane")]
        {
            let _witness: Box<dyn OidcRequestStore> = Box::new(CapabilityWitness);
            let _witness: Box<dyn ControlSessionStore> = Box::new(CapabilityWitness);
            let _witness: Box<dyn AccountDeletionStore> = Box::new(CapabilityWitness);
        }
    }
}
