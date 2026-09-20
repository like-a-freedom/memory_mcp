use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::error::MemoryError;
pub use crate::http::config::BrowserAuthMode;
use crate::http::registry::models::{AccountStatus, TenantStatus};

// ─── Error type ───────────────────────────────────────────

/// Local admin domain error. Wraps `MemoryError` for infrastructure
/// failures while providing typed domain variants for business logic.
#[derive(Debug, thiserror::Error)]
pub enum LocalAdminError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("invalid challenge")]
    InvalidChallenge,
    #[error("unauthenticated")]
    Unauthenticated,
    #[error("forbidden")]
    Forbidden,
    #[error("reauth required")]
    ReauthRequired,
    #[error("not found")]
    NotFound,
    #[error("state conflict")]
    StateConflict,
    #[error("version conflict")]
    VersionConflict,
    #[error("idempotency conflict")]
    IdempotencyConflict,
    #[error("key cap reached")]
    KeyCap,
    #[error("secret already issued for key {key_id}")]
    SecretAlreadyIssued { key_id: String },
    #[error("throttled, retry after {retry_after_seconds}s")]
    Throttled { retry_after_seconds: u32 },
    #[error("service unavailable")]
    Unavailable,
    #[error("internal error")]
    Infrastructure(#[from] MemoryError),
}

pub type LocalResult<T> = Result<T, LocalAdminError>;

// ─── Common types ─────────────────────────────────────────

/// Policy fence binding mode and epoch for browser sessions/challenges.
///
/// Defined by the storage layer (plan §3.1) so OIDC persistence can reference
/// it without importing local-admin business logic; re-exported here so the
/// service contracts and the store share one nominal type rather than two
/// structurally identical ones.
pub use crate::http::registry::models::BrowserPolicyFence;

/// HMAC fingerprints for session and CSRF keys, used to join the
/// durable policy.
#[derive(Debug, Clone)]
pub struct LocalKeyFingerprints {
    pub session: [u8; 32],
    pub csrf: [u8; 32],
}

/// Local admin state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdminState {
    PendingActivation,
    Active,
    RecoveryRequired,
}

/// Challenge kind: activation or reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeKind {
    Activate,
    Reset,
}

/// Server-generated request context.
#[derive(Debug, Clone)]
pub struct RequestContext {
    pub request_id: uuid::Uuid,
}

/// Auth attempt context with source IP.
#[derive(Debug, Clone)]
pub struct AuthAttemptContext {
    pub request: RequestContext,
    pub source: std::net::IpAddr,
}

/// Admin fence for session/guard checks.
#[derive(Debug, Clone)]
pub struct AdminFence {
    pub admin_id: String,
    pub session_id: String,
    pub credential_generation: u64,
    pub policy: BrowserPolicyFence,
}

/// Admin principal after successful authentication.
#[derive(Debug, Clone)]
pub struct AdminPrincipal {
    pub fence: AdminFence,
    pub username: String,
    pub auth_time: DateTime<Utc>,
    pub absolute_expiry: DateTime<Utc>,
}

/// Credential snapshot for login/activation checks.
#[derive(Debug, Clone)]
pub struct CredentialSnapshot {
    pub admin_id: String,
    pub username: String,
    pub state: AdminState,
    pub credential_generation: u64,
    pub password_phc: Option<String>,
}

/// Challenge creation command.
#[derive(Debug)]
pub struct ChallengeIssue {
    pub username: String,
    pub kind: ChallengeKind,
    pub verifier: [u8; 32],
    pub policy: BrowserPolicyFence,
    pub request: RequestContext,
}

/// Issued challenge with expiry.
#[derive(Debug)]
pub struct IssuedChallenge {
    pub admin_id: String,
    pub username: String,
    pub expires_at: DateTime<Utc>,
}

/// Challenge view returned to browser (no credential data).
#[derive(Debug, Serialize)]
pub struct ChallengeView {
    pub username: String,
    pub expires_at: DateTime<Utc>,
}

/// Challenge finish command (activation or reset).
#[derive(Debug)]
pub struct ChallengeFinish {
    pub verifier: [u8; 32],
    pub kind: ChallengeKind,
    pub password_phc: String,
    pub policy: BrowserPolicyFence,
    pub request: RequestContext,
}

/// Session open command.
#[derive(Debug)]
pub struct SessionOpen {
    pub credential: CredentialSnapshot,
    pub cookie_verifier: [u8; 32],
    pub policy: BrowserPolicyFence,
    pub request: RequestContext,
}

/// Session rotate command.
#[derive(Debug)]
pub struct SessionRotate {
    pub fence: AdminFence,
    pub credential: CredentialSnapshot,
    pub cookie_verifier: [u8; 32],
    pub request: RequestContext,
}

/// Admin key-issue command (plan §3.4).
///
/// Carries only what the caller chooses: the service generates the credential
/// material, the fingerprint and the persisted metadata from it.
#[derive(Debug, Clone)]
pub struct AdminKeyCreate {
    pub name: String,
    pub expiry: KeyExpiry,
    pub operation_id: uuid::Uuid,
}

/// A revealed-once client key (plan §3.4).
///
/// `secret` is the full `mem_sk_<key_id>_<secret>` credential — the only time
/// the raw credential exists outside the operator's clipboard. Only its
/// verifier is durable.
#[derive(Debug, Clone)]
pub struct IssuedClientKey {
    pub id: String,
    pub name: String,
    pub secret: String,
    pub expires_at: Option<DateTime<Utc>>,
}

/// One-time challenge code with the issued material.
#[derive(Debug)]
pub struct OneTimeChallenge {
    pub issued: IssuedChallenge,
    pub code: String,
}

/// Admin login result.
#[derive(Debug)]
pub struct AdminLogin {
    pub principal: AdminPrincipal,
    pub cookie: String,
}

// ─── Attempt domain and throttle types ────────────────────

/// Login/reauth share first domain; inspect/activate/reset share second.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptDomain {
    Credentials,
    Challenge,
}

/// Attempt input for throttle reservation.
#[derive(Debug, Clone)]
pub struct AttemptInput {
    pub domain: AttemptDomain,
    pub username_bucket: Option<u16>,
    pub source_bucket: u16,
    pub policy: BrowserPolicyFence,
    pub request: RequestContext,
}

/// Attempt reservation decision.
#[derive(Debug)]
pub enum AttemptDecision {
    Allowed,
    Limited { retry_after_seconds: u32 },
}

/// Failure audit event.
///
/// Only a service-reserved authentication attempt produces one of these
/// (plan §3.1): pre-admission, session and client-mutation denials are
/// aggregated by the rate buckets instead, so there is no
/// append-per-denial path and no caller-controlled audit cardinality.
///
/// The durable projection of an event is the allowlisted audit row —
/// actor, action, outcome, reason, request id and the target ids. The
/// remaining fields are service-set attributes that describe the
/// rejection without granting it authority: `policy` carries the epoch
/// the request was presented under, so a request still failing on a stale
/// fence records an event instead of an unrelated audit failure; the
/// bounded buckets are aggregated in `local_admin_rate_bucket`; and
/// `admitted_auth_attempt` separates these events from the aggregate
/// slots.
pub struct FailureAudit {
    pub request: RequestContext,
    pub policy: BrowserPolicyFence,
    pub action: FailureAction,
    pub reason: FailureReason,
    pub admin_id: Option<String>,
    pub username_bucket: Option<u16>,
    pub source_bucket: Option<u16>,
    pub admitted_auth_attempt: bool,
}

/// The closed vocabulary of rejected actions.
///
/// `Session` and `ClientMutation` are part of the ledger's closed enum but
/// are deliberately never appended: those denials are counted by the fixed
/// action/reason aggregate slots in `local_admin_rate_bucket`, so no
/// invalid-cookie or stale-mutation stream reaches the audit table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureAction {
    Login,
    Reauth,
    Challenge,
    Session,
    ClientMutation,
}

/// The closed vocabulary of rejection reasons. `InvalidSession`,
/// `StaleFence` and `Forbidden` describe aggregate-slot denials (see
/// [`FailureAction`]) rather than appended events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureReason {
    InvalidCredentials,
    InvalidChallenge,
    InvalidSession,
    StaleFence,
    Forbidden,
}

// ─── Pagination ───────────────────────────────────────────

#[derive(Debug)]
pub struct PageRequest {
    pub after: Option<String>,
    pub limit: u16,
}

#[derive(Debug, Serialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

// ─── Client types ─────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize)]
pub struct ClientCreate {
    pub display_name: String,
    pub operation_id: uuid::Uuid,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClientView {
    pub account_id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub account_status: AccountStatus,
    pub tenant_status: TenantStatus,
    pub plan_version: u32,
    pub schema_version: u32,
    pub version: u64,
    pub provisioning_reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ClientBundle {
    pub account: crate::http::registry::models::Account,
    pub tenant: crate::http::registry::models::Tenant,
    pub display_name: String,
    pub operation_id: uuid::Uuid,
    pub request_fingerprint: [u8; 32],
}

/// The expiry choice for an issued key.
///
/// The wire form is internally tagged by `kind` and **strict**: exactly
/// `{"kind":"never"}` or `{"kind":"days","days":<n>}`. `serde`'s
/// internally tagged enums silently ignore unknown and cross-variant fields,
/// which would let a typo like `"dayz":30` become an accidental
/// non-expiring key — the exact outcome spec §8 forbids. The strictness is
/// therefore implemented over a `deny_unknown_fields` struct, and the wire
/// shape is unchanged.
#[derive(Debug, Clone)]
pub enum KeyExpiry {
    Never,
    Days { days: u32 },
}

impl<'de> serde::Deserialize<'de> for KeyExpiry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            kind: String,
            #[serde(default)]
            days: Option<u32>,
        }

        let wire = Wire::deserialize(deserializer)?;
        match (wire.kind.as_str(), wire.days) {
            ("never", None) => Ok(KeyExpiry::Never),
            ("days", Some(days)) => Ok(KeyExpiry::Days { days }),
            ("never", Some(_)) => Err(serde::de::Error::custom(
                "`days` is not valid for a never-expiring key",
            )),
            ("days", None) => Err(serde::de::Error::custom(
                "a day-count expiry requires `days`",
            )),
            (other, _) => Err(serde::de::Error::unknown_variant(other, &["never", "days"])),
        }
    }
}

#[derive(Debug, Clone)]
pub struct AdminKeyInsert {
    pub account_id: String,
    pub key_id: String,
    pub name: String,
    pub verifier: crate::http::registry::models::KeyedVerifier,
    pub expiry: KeyExpiry,
    pub operation_id: uuid::Uuid,
    pub request_fingerprint: [u8; 32],
}

#[derive(Debug)]
pub enum KeyInsertOutcome {
    Created(crate::http::registry::models::ApiKeyMeta),
    AlreadyIssued { key_id: String },
}

#[derive(Debug, Clone, Copy)]
pub enum ClientStateAction {
    Suspend,
    Resume,
}

// ─── Store trait ──────────────────────────────────────────

/// Durable local admin store. Implemented by `SurrealRegistryStore`
/// in a child module.
#[async_trait]
pub trait LocalAdminStore: Send + Sync + 'static {
    /// Join or verify the local policy singleton. Compares
    /// mode/epoch/config fingerprints; fails on drift.
    async fn join_local_policy(
        &self,
        fingerprints: LocalKeyFingerprints,
    ) -> LocalResult<BrowserPolicyFence>;

    /// Issue a new challenge (activation or reset). Unique canonical
    /// admin, fixed 900-second DB TTL, verifier only.
    async fn issue_challenge(&self, command: ChallengeIssue) -> LocalResult<IssuedChallenge>;

    /// Inspect a challenge without consuming it.
    async fn inspect_challenge(
        &self,
        verifier: &[u8; 32],
        kind: ChallengeKind,
        policy: &BrowserPolicyFence,
    ) -> LocalResult<ChallengeView>;

    /// Finish a challenge (activate or reset password).
    async fn finish_challenge(&self, command: ChallengeFinish) -> LocalResult<()>;

    /// Look up a credential snapshot by username.
    async fn credential(
        &self,
        username: &str,
        policy: &BrowserPolicyFence,
    ) -> LocalResult<Option<CredentialSnapshot>>;

    /// Open a new session after successful login.
    async fn open_session(&self, command: SessionOpen) -> LocalResult<AdminPrincipal>;

    /// Resolve a session by cookie verifier.
    async fn resolve_session(
        &self,
        cookie_verifier: &[u8; 32],
        policy: &BrowserPolicyFence,
    ) -> LocalResult<AdminPrincipal>;

    /// Rotate a session (reauth).
    async fn rotate_session(&self, command: SessionRotate) -> LocalResult<AdminPrincipal>;

    /// Revoke a session.
    async fn revoke_session(&self, fence: &AdminFence, request: &RequestContext)
    -> LocalResult<()>;

    /// Reserve throttle attempt atomically.
    async fn reserve_attempt(&self, input: AttemptInput) -> LocalResult<AttemptDecision>;

    /// Reclaim expired throttle buckets.
    ///
    /// Bounded maintenance only: expiry is enforced by database time on
    /// every reservation, so a delayed or skipped pass can never admit an
    /// extra attempt — it only keeps the table small.
    async fn cleanup_rate_buckets(&self) -> LocalResult<u64>;

    /// Record a failure audit event (called after rollback or rejection).
    async fn record_failure(&self, event: FailureAudit) -> LocalResult<()>;

    // ── Client methods (Task 7) ──

    /// Create a client atomically.
    async fn create_client(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        bundle: ClientBundle,
    ) -> LocalResult<ClientView>;

    /// List clients created by local workflow.
    async fn list_clients(
        &self,
        fence: &AdminFence,
        page: PageRequest,
    ) -> LocalResult<Page<ClientView>>;

    /// Get a single client by account id.
    async fn client(&self, fence: &AdminFence, account_id: &str) -> LocalResult<ClientView>;

    /// List keys for a client.
    async fn list_client_keys(
        &self,
        fence: &AdminFence,
        account_id: &str,
        page: PageRequest,
    ) -> LocalResult<Page<crate::http::registry::models::ApiKeyMeta>>;

    /// Insert a client key.
    async fn insert_client_key(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        command: AdminKeyInsert,
    ) -> LocalResult<KeyInsertOutcome>;

    /// Revoke a client key.
    async fn revoke_client_key(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        account_id: &str,
        key_id: &str,
    ) -> LocalResult<()>;

    /// Set client state (suspend/resume).
    async fn set_client_state(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        account_id: &str,
        expected_version: u64,
        action: ClientStateAction,
    ) -> LocalResult<()>;
}
