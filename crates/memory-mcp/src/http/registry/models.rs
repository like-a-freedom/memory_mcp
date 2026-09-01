//! Registry record models.
//!
//! These types are the durable shape of the control namespace.
//! The trait surface in `storage.rs` and the API key parser in
//! `principal/api_keys.rs` reference them.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlanLimits {
    pub max_ingested_bytes: u64,
    pub max_episode_count: u64,
    pub max_open_app_sessions: u32,
    pub max_active_api_keys: u32,
    pub per_tenant_request_concurrency: u32,
    pub extraction_concurrency: u32,
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

impl KeyedVerifier {
    /// Compute HMAC-SHA256(pepper, secret) into a fixed-size
    /// verifier. Used when issuing new API keys.
    pub fn compute(pepper: &[u8], secret: &[u8]) -> Self {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        // HMAC-SHA256 accepts a key of any length. Keep the
        // public infallible constructor fail-closed if the
        // dependency ever violates that contract instead of
        // allowing malformed configuration to panic the server.
        let Ok(mut mac) = <Hmac<Sha256> as Mac>::new_from_slice(pepper) else {
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
    fn plan_limits_default_is_zero() {
        let l = PlanLimits::default();
        assert_eq!(l.max_ingested_bytes, 0);
        assert_eq!(l.per_tenant_request_concurrency, 0);
    }
}
