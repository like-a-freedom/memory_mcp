//! Registry record models.
//!
//! These types are the durable shape of the control namespace.
//! The trait surface in `storage.rs` and the API key parser in
//! `principal/api_keys.rs` reference them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::http::config::BrowserAuthMethod;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalIdentity {
    pub id: String,
    pub issuer: String,
    /// HMAC(identity_index_key, normalized_issuer || ":" || subject).
    /// Raw OIDC `sub` is never persisted.
    pub subject_verifier: SubjectVerifier,
    pub account_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub id: String,
    pub status: AccountStatus,
    pub tenant_id: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Active,
    Suspended,
    Deleting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tenant {
    pub id: String,
    pub status: TenantStatus,
    pub namespace_binding: NamespaceBinding,
    pub plan_version: u32,
    pub schema_version: u32,
    /// Stage to resume after a retryable failure; never inferred from a lease.
    pub retry_stage: Option<TenantStatus>,
    /// Durable snapshot of the currently claimed provisioning fence.
    pub provisioning_lease: Option<ProvisioningLeaseState>,
    pub created_at: DateTime<Utc>,
    pub version: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceBinding {
    pub namespace: String, // server-generated, opaque, immutable
    pub database: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvisioningLeaseState {
    pub owner_id: String,
    pub lease_id: String,
    pub expires_at: DateTime<Utc>,
    pub fencing_generation: u64,
    pub heartbeat_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum TenantStatus {
    Reserved,
    NamespaceCreating,
    Migrating,
    Ready,
    Suspended,
    Failed,
    Deleting,
    Purged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKey {
    pub id: String, // public, opaque
    pub account_id: String,
    pub name: String,
    pub verifier: KeyedVerifier, // HMAC over secret + pepper
    pub status: ApiKeyStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub version: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyStatus {
    Active,
    Revoked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiKeyMeta {
    pub id: String,
    pub name: String,
    pub status: ApiKeyStatus,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlPlaneSession {
    pub id: String,
    pub cookie_hash: [u8; 32], // keyed HMAC; raw cookie is never persisted
    pub account_id: String,
    pub auth_time: DateTime<Utc>,
    pub idle_expiry: DateTime<Utc>,
    pub absolute_expiry: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plan {
    pub id: String,
    pub version: u32,
    pub limits: PlanLimits,
}

impl Default for Plan {
    fn default() -> Self {
        Self {
            id: "free".to_string(),
            version: 1,
            limits: PlanLimits::default(),
        }
    }
}

pub const DEFAULT_MAX_INGESTED_BYTES: u64 = 1_073_741_824;
pub const DEFAULT_MAX_EPISODE_COUNT: u64 = 100_000;
pub const DEFAULT_INGEST_PER_MINUTE: u32 = 60;
pub const DEFAULT_MAX_OPEN_APP_SESSIONS: u32 = 32;
pub const DEFAULT_MAX_ACTIVE_API_KEYS: u32 = 5;
pub const DEFAULT_PER_TENANT_REQUEST_CONCURRENCY: u32 = 4;
pub const DEFAULT_EXTRACTION_CONCURRENCY: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanLimits {
    pub max_ingested_bytes: u64,
    pub max_episode_count: u64,
    pub ingest_per_minute: u32,
    pub max_open_app_sessions: u32,
    pub max_active_api_keys: u32,
    pub per_tenant_request_concurrency: u32,
    pub extraction_concurrency: u32,
}

impl Default for PlanLimits {
    fn default() -> Self {
        Self {
            // These are conservative development/free-tier defaults. Production
            // deployments should provision the versioned plan in the Registry.
            max_ingested_bytes: DEFAULT_MAX_INGESTED_BYTES,
            max_episode_count: DEFAULT_MAX_EPISODE_COUNT,
            ingest_per_minute: DEFAULT_INGEST_PER_MINUTE,
            max_open_app_sessions: DEFAULT_MAX_OPEN_APP_SESSIONS,
            max_active_api_keys: DEFAULT_MAX_ACTIVE_API_KEYS,
            per_tenant_request_concurrency: DEFAULT_PER_TENANT_REQUEST_CONCURRENCY,
            extraction_concurrency: DEFAULT_EXTRACTION_CONCURRENCY,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum UsageCounter {
    IngestedBytes,
    EpisodeCount,
    OpenAppSessions,
    ActiveApiKeys,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyedVerifier(pub [u8; 32]); // HMAC-SHA256(pepper, secret)

/// HMAC-SHA256(identity_index_key, normalized_issuer || ":" || subject).
/// Newtype so callers cannot pass a raw 32-byte slice where an
/// HMAC-indexed key is required.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct SubjectVerifier(pub [u8; 32]);

/// Pairing of an OIDC issuer and a `SubjectVerifier`. The
/// `Account ↔ ExternalIdentity` lookup index is the (issuer,
/// subject_verifier) unique pair.
#[derive(Debug, Clone)]
pub struct IdentityRef {
    pub issuer: String,
    pub subject_verifier: SubjectVerifier,
}

/// Browser-auth policy fence returned by `join_oidc_policy`.
/// The storage layer defines this so the `RegistryStore` trait can
/// reference it without importing the local_admin service contracts.
#[derive(Debug, Clone)]
pub struct BrowserPolicyFence {
    /// The methods this deployment enables (ADR-0057). A row created before
    /// migration 048 carries no set and is read as the single method named by
    /// its `mode`.
    pub methods: Vec<crate::http::config::BrowserAuthMethod>,
    pub epoch: u64,
}

impl BrowserPolicyFence {
    /// Whether `method` is enabled on this deployment.
    pub fn has(&self, method: crate::http::config::BrowserAuthMethod) -> bool {
        self.methods.contains(&method)
    }
}

/// Read the enabled browser authentication methods from a `browser_auth_policy`
/// row.
///
/// A row created before migration 048 carries no `methods` array and reads as
/// the single method named by its `mode`, which is what makes the change
/// additive for a deployment that already holds a single-mode row. Returns
/// `None` when neither source is present or a token is unrecognized, so the
/// caller fails closed with its own storage error.
pub fn policy_methods_from_row(row: &serde_json::Value) -> Option<Vec<BrowserAuthMethod>> {
    let tokens: Vec<&str> = match row.get("methods").and_then(|value| value.as_array()) {
        Some(values) => values.iter().filter_map(|value| value.as_str()).collect(),
        None => vec![row.get("mode")?.as_str()?],
    };
    if tokens.is_empty() {
        return None;
    }
    tokens.into_iter().map(BrowserAuthMethod::parse).collect()
}

/// One-use deletion challenge keyed by an HMAC verifier.
/// The raw token is never persisted; the verifier is the
/// only durable link.
#[derive(Debug, Clone)]
pub struct DeletionChallengeRecord {
    pub id: String,
    pub verifier: String,
    pub account_id: String,
    pub session_id: String,
    pub expires_at: DateTime<Utc>,
    pub consumed_at: Option<DateTime<Utc>>,
}

/// Who or what performed a control action.
///
/// Mirrors the `actor_kind` allowlist on the durable `audit_event` table, so an
/// unrecognized kind is a compile error here rather than a row the database
/// rejects at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditActorKind {
    /// The Account holder, acting in their own browser session.
    Account,
    /// An operator, acting through the operator surface.
    Operator,
    /// The server, acting on its own schedule.
    System,
}

impl AuditActorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Account => "account",
            Self::Operator => "operator",
            Self::System => "system",
        }
    }
}

/// The actor behind an identity change and the instant it happened.
///
/// The store methods that change an Account's identities take one of these and
/// write the audit row in the same transaction, so a change cannot be performed
/// without recording who performed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdentityAudit {
    pub actor_kind: AuditActorKind,
    pub actor_principal: String,
    pub occurred_at: DateTime<Utc>,
}

impl IdentityAudit {
    /// An identity change performed by the Account holder themselves.
    pub fn by_account(account_id: &str, occurred_at: DateTime<Utc>) -> Self {
        Self {
            actor_kind: AuditActorKind::Account,
            actor_principal: account_id.to_owned(),
            occurred_at,
        }
    }

    /// An identity change initiated by the deployment administrator (an
    /// identity invitation, ADR-0057). The provider attestation happens in
    /// the flow; the administrator is who caused the binding.
    pub fn by_operator(principal: &str, occurred_at: DateTime<Utc>) -> Self {
        Self {
            actor_kind: AuditActorKind::Operator,
            actor_principal: principal.to_owned(),
            occurred_at,
        }
    }
}

/// Which identity change an audit row records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityAuditAction {
    Linked,
    Unlinked,
}

impl IdentityAuditAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Linked => "identity_linked",
            Self::Unlinked => "identity_unlinked",
        }
    }
}

/// One row of the control audit log (`audit_event`, migration 045).
///
/// The log is append-only: the durable table grants no select, update or delete
/// permission to an unprivileged caller, and nothing rewrites a row once it is
/// written. Deletion events are appended inside the transaction that performs
/// the deletion; identity changes use [`ControlAuditEvent::identity_change`] and
/// are appended by the store method that performs the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlAuditEvent {
    pub account_id: String,
    pub actor_kind: AuditActorKind,
    /// The concrete principal behind `actor_kind`: the Account id for
    /// [`AuditActorKind::Account`], the operator id for
    /// [`AuditActorKind::Operator`].
    pub actor_principal: String,
    /// The durable action token, e.g. `account_deletion_started`.
    pub action: String,
    /// The resource inside the Account that the action touched. Only identity
    /// changes set it today; the Account itself is `account_id`.
    pub target_identity_id: Option<String>,
    /// The durable row id, unique per action. Deterministic, so appending the
    /// same event twice conflicts instead of appending a second row.
    pub correlation_id: String,
    pub occurred_at: DateTime<Utc>,
}

impl ControlAuditEvent {
    /// A control action whose only subject is the Account.
    pub fn for_account(
        account_id: &str,
        actor_kind: AuditActorKind,
        actor_principal: &str,
        action: &str,
        occurred_at: DateTime<Utc>,
    ) -> Self {
        Self {
            account_id: account_id.to_owned(),
            actor_kind,
            actor_principal: actor_principal.to_owned(),
            action: action.to_owned(),
            target_identity_id: None,
            correlation_id: format!("{action}_{account_id}"),
            occurred_at,
        }
    }

    /// The row for attaching or detaching one External Identity (ADR-0057).
    ///
    /// The identity id is both the named target and the basis of the row id, so
    /// the two events of one identity's lifetime stay distinguishable and a
    /// repeated append cannot duplicate the row.
    pub fn identity_change(
        action: IdentityAuditAction,
        identity_id: &str,
        account_id: &str,
        audit: &IdentityAudit,
    ) -> Self {
        let action = action.as_str();
        Self {
            account_id: account_id.to_owned(),
            actor_kind: audit.actor_kind,
            actor_principal: audit.actor_principal.clone(),
            action: action.to_owned(),
            target_identity_id: Some(identity_id.to_owned()),
            correlation_id: format!("{action}_{identity_id}"),
            occurred_at: audit.occurred_at,
        }
    }
}

impl KeyedVerifier {
    /// Compute HMAC-SHA256(pepper, secret) into a fixed-size
    /// verifier. Used when issuing new API keys.
    pub fn compute(pepper: &[u8], secret: &[u8]) -> Self {
        use hmac::{Hmac, KeyInit, Mac};
        use sha2::Sha256;
        // HMAC-SHA256 accepts a key of any length. Keep the
        // public infallible constructor fail-closed if the
        // dependency ever violates that contract instead of
        // allowing malformed configuration to panic the server.
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(pepper) else {
            return Self([0; 32]);
        };
        mac.update(secret);
        Self(mac.finalize().into_bytes().into())
    }

    /// Constant-time verify of `(pepper, secret)` against the
    /// stored verifier.
    pub fn verify(&self, pepper: &[u8], secret: &[u8]) -> bool {
        use subtle::ConstantTimeEq;
        let expected = Self::compute(pepper, secret).0;
        expected.ct_eq(&self.0).into()
    }
}

/// Opaque-id helpers. All ids are server-generated UUID v4s
/// with a type tag prefix so log scrapers and metrics can group
/// by id type without parsing the namespace.
pub fn new_account_id() -> String {
    format!("acct_{}", uuid::Uuid::new_v4())
}

pub fn new_tenant_id() -> String {
    format!("ten_{}", uuid::Uuid::new_v4())
}

pub fn new_api_key_id() -> String {
    format!("ak_{}", uuid::Uuid::new_v4())
}

pub fn new_external_identity_id() -> String {
    format!("idn_{}", uuid::Uuid::new_v4())
}

pub fn new_namespace_name() -> String {
    format!("tns_{}", uuid::Uuid::new_v4().simple())
}

/// A fresh `Account` + `Tenant` pair in the state every browser-auth workflow
/// starts from: an `Active` account whose tenant is `Reserved` at
/// `plan_version` and has not been provisioned yet.
///
/// Every caller that enrols a new client needs the same shape, and the
/// provisioning worker keys off exactly these fields, so it is built in one
/// place rather than re-typed per workflow.
pub fn new_reserved_bundle(plan_version: u32, now: DateTime<Utc>) -> (Account, Tenant) {
    let account = Account {
        id: new_account_id(),
        status: AccountStatus::Active,
        tenant_id: new_tenant_id(),
        created_at: now,
    };
    let tenant = Tenant {
        id: account.tenant_id.clone(),
        status: TenantStatus::Reserved,
        namespace_binding: NamespaceBinding {
            namespace: new_namespace_name(),
            database: "memory".into(),
        },
        plan_version,
        schema_version: 0,
        retry_stage: None,
        provisioning_lease: None,
        created_at: now,
        version: 0,
    };
    (account, tenant)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_have_expected_prefixes() {
        assert!(new_account_id().starts_with("acct_"));
        assert!(new_tenant_id().starts_with("ten_"));
        assert!(new_api_key_id().starts_with("ak_"));
        assert!(new_external_identity_id().starts_with("idn_"));
        assert!(new_namespace_name().starts_with("tns_"));
    }

    #[test]
    fn tenant_version_round_trips() {
        let t = Tenant {
            id: new_tenant_id(),
            status: TenantStatus::Ready,
            namespace_binding: NamespaceBinding {
                namespace: "tns_x".into(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 1,
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 7,
        };
        let json = serde_json::to_string(&t).expect("serialize");
        let back: Tenant = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, t.id);
        assert_eq!(back.version, 7);
        assert_eq!(back.status, TenantStatus::Ready);
    }

    #[test]
    fn tenant_status_serializes_as_snake_case() {
        let s = serde_json::to_string(&TenantStatus::NamespaceCreating).expect("serialize");
        assert_eq!(s, "\"namespace_creating\"");
    }

    #[test]
    fn plan_limits_default_is_safe_free_tier() {
        let l = PlanLimits::default();
        assert_eq!(l.max_ingested_bytes, DEFAULT_MAX_INGESTED_BYTES);
        assert_eq!(l.max_episode_count, DEFAULT_MAX_EPISODE_COUNT);
        assert_eq!(l.ingest_per_minute, DEFAULT_INGEST_PER_MINUTE);
        assert_eq!(l.max_open_app_sessions, DEFAULT_MAX_OPEN_APP_SESSIONS);
        assert_eq!(l.max_active_api_keys, DEFAULT_MAX_ACTIVE_API_KEYS);
        assert_eq!(
            l.per_tenant_request_concurrency,
            DEFAULT_PER_TENANT_REQUEST_CONCURRENCY
        );
        assert_eq!(l.extraction_concurrency, DEFAULT_EXTRACTION_CONCURRENCY);
    }
}
