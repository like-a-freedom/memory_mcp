use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::MemoryError;
use crate::http::leases::ProvisioningLease;
use crate::identity::api::AuthMethod;
use crate::models::registry::TenantStatus;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientAuthority {
    pub admin_id: String,
    pub session_verifier: String,
    pub credential_generation: u64,
    pub policy_epoch: u64,
    pub policy_methods: Vec<AuthMethod>,
    pub request_id: uuid::Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateClientCommand {
    pub authority: ClientAuthority,
    pub operation_id: uuid::Uuid,
    pub display_name: String,
    pub plan_version: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientCreation {
    pub authority: ClientAuthority,
    pub operation_id: uuid::Uuid,
    pub display_name: String,
    pub plan_version: u32,
    pub account_id: String,
    pub tenant_id: String,
    pub namespace: String,
    pub database: String,
    pub schema_version: u32,
    pub request_fingerprint: [u8; 32],
    pub now: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ClientView {
    pub account_id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub account_status: String,
    pub tenant_status: String,
    pub plan_version: u32,
    pub schema_version: u32,
    pub version: u64,
    pub provisioning_reason: Option<String>,
}

#[derive(Debug)]
pub enum ClientCreationError {
    InvalidInput(String),
    ReauthenticationRequired,
    Unauthorized,
    Forbidden,
    NotFound,
    IdempotencyConflict,
    Unavailable,
}

#[async_trait::async_trait]
pub trait ClientCreationPort: Send + Sync {
    async fn create_client(
        &self,
        command: ClientCreation,
    ) -> Result<ClientView, ClientCreationError>;
}

pub async fn create_client(
    port: &(impl ClientCreationPort + ?Sized),
    command: CreateClientCommand,
    now: DateTime<Utc>,
) -> Result<ClientView, ClientCreationError> {
    let account_id = format!("acct_{}", uuid::Uuid::new_v4());
    let tenant_id = format!("ten_{}", uuid::Uuid::new_v4());
    let namespace = format!("tns_{}", uuid::Uuid::new_v4().simple());
    let request_fingerprint = client_fingerprint(command.operation_id, &command.display_name);
    port.create_client(ClientCreation {
        authority: command.authority,
        operation_id: command.operation_id,
        display_name: command.display_name,
        plan_version: command.plan_version,
        account_id,
        tenant_id,
        namespace,
        database: "memory".into(),
        schema_version: 0,
        request_fingerprint,
        now,
    })
    .await
}

fn client_fingerprint(operation_id: uuid::Uuid, display_name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"local_admin_client_create\0");
    hasher.update(operation_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(display_name.as_bytes());
    hasher.finalize().into()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenAppSessionCommand {
    pub app: String,
    pub payload: serde_json::Value,
    pub max_open_per_tenant: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSessionCommand {
    pub handle: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSessionUsage {
    pub expected_version: u64,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSessionRecord {
    pub handle: String,
    pub app: String,
    pub version: u64,
    pub payload: serde_json::Value,
    pub idle_expiry: DateTime<Utc>,
    pub absolute_expiry: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSessionView {
    pub handle: String,
    pub app: String,
    pub version: u64,
    pub payload: serde_json::Value,
    pub expires_at: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait AppSessionPersistence: Send + Sync {
    async fn open(
        &self,
        app: &str,
        payload: serde_json::Value,
        max_open_per_tenant: u32,
        now: DateTime<Utc>,
    ) -> Result<AppSessionRecord, MemoryError>;
    async fn load(&self, handle: &str) -> Result<Option<AppSessionRecord>, MemoryError>;
    async fn command(
        &self,
        handle: &str,
        expected_version: u64,
        payload: serde_json::Value,
    ) -> Result<u64, MemoryError>;
    async fn close(&self, handle: &str) -> Result<(), MemoryError>;
}

pub async fn open_app_session(
    persistence: &(impl AppSessionPersistence + ?Sized),
    command: OpenAppSessionCommand,
) -> Result<AppSessionRecord, MemoryError> {
    persistence
        .open(
            &command.app,
            command.payload,
            command.max_open_per_tenant,
            Utc::now(),
        )
        .await
}

pub async fn read_app_session(
    persistence: &(impl AppSessionPersistence + ?Sized),
    command: &AppSessionCommand,
    now: DateTime<Utc>,
) -> Result<Option<AppSessionView>, MemoryError> {
    let Some(record) = persistence.load(&command.handle).await? else {
        return Ok(None);
    };
    let expires_at = record.idle_expiry.min(record.absolute_expiry);
    if expires_at <= now {
        return Ok(None);
    }
    Ok(Some(AppSessionView {
        handle: record.handle,
        app: record.app,
        version: record.version,
        payload: record.payload,
        expires_at,
    }))
}

pub async fn write_app_session(
    persistence: &(impl AppSessionPersistence + ?Sized),
    command: &AppSessionCommand,
    usage: &AppSessionUsage,
) -> Result<u64, MemoryError> {
    persistence
        .command(
            &command.handle,
            usage.expected_version,
            usage.payload.clone(),
        )
        .await
}

pub async fn close_app_session(
    persistence: &(impl AppSessionPersistence + ?Sized),
    command: &AppSessionCommand,
) -> Result<(), MemoryError> {
    persistence.close(&command.handle).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    Completed,
    CompletedBeforeCancel,
    CancelRequested,
    Cancelled,
    CancelledBeforeCommit,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskView {
    pub task_id: String,
    pub state: TaskState,
    pub created_at: DateTime<Utc>,
    pub progress: Option<serde_json::Value>,
    pub result: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
}

impl TaskView {
    /// The protocol status a task view reports. `cancel_requested` and
    /// `completed_before_cancel` both surface as `cancelled`: neither outcome
    /// promises the caller a completed result.
    pub fn status(&self) -> &'static str {
        match self.state {
            TaskState::Queued | TaskState::Running => "working",
            TaskState::Completed => "completed",
            TaskState::Failed => "failed",
            TaskState::CancelRequested
            | TaskState::CompletedBeforeCancel
            | TaskState::Cancelled
            | TaskState::CancelledBeforeCommit => "cancelled",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EnqueueTaskCommand {
    pub fingerprint: String,
    pub params: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelTaskCommand {
    pub task_id: String,
}

#[async_trait::async_trait]
pub trait DurableTaskPort: Send + Sync {
    async fn enqueue(
        &self,
        fingerprint: String,
        params: serde_json::Value,
    ) -> Result<String, MemoryError>;
    async fn load(&self, task_id: &str) -> Result<Option<TaskView>, MemoryError>;
    async fn set_cancellation_intent(&self, task_id: &str) -> Result<(), MemoryError>;
}

pub async fn enqueue_task(
    port: &(impl DurableTaskPort + ?Sized),
    command: EnqueueTaskCommand,
) -> Result<String, MemoryError> {
    port.enqueue(command.fingerprint, command.params).await
}

pub async fn task_view(
    port: &(impl DurableTaskPort + ?Sized),
    task_id: &str,
) -> Result<Option<TaskView>, MemoryError> {
    port.load(task_id).await
}

pub async fn cancel_task(
    port: &(impl DurableTaskPort + ?Sized),
    command: &CancelTaskCommand,
) -> Result<(), MemoryError> {
    port.set_cancellation_intent(&command.task_id).await
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateApiKeyCommand {
    pub account_id: String,
    pub name: String,
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKeyOwner {
    pub tenant_id: String,
    pub plan_version: u32,
}

/// A newly issued API key.
///
/// `secret` is a one-time credential. The derived `Debug` is
/// overridden so a `{:?}` in a log line or an error report cannot
/// render it; recovering the value is only possible through the
/// explicit field access the issuing handler performs.
#[derive(Clone, PartialEq, Eq)]
pub struct CreatedApiKey {
    pub id: String,
    pub secret: String,
    pub name: String,
    pub expires_at: Option<DateTime<Utc>>,
}

impl std::fmt::Debug for CreatedApiKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CreatedApiKey")
            .field("id", &self.id)
            .field("secret", &"[REDACTED]")
            .field("name", &self.name)
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct NewApiKeyRecord {
    pub id: String,
    pub account_id: String,
    pub name: String,
    pub expires_at: Option<DateTime<Utc>>,
    /// The one-time secret. It is shown to the caller once and never
    /// persisted; the adapter owns deriving the irreversible verifier.
    pub secret: String,
    pub cap: u32,
    pub now: DateTime<Utc>,
}

#[async_trait::async_trait]
pub trait ApiKeyIssuancePort: Send + Sync {
    async fn owner(&self, account_id: &str) -> Result<Option<ApiKeyOwner>, MemoryError>;
    async fn active_key_cap(&self, tenant_id: &str, plan_version: u32) -> Result<u32, MemoryError>;
    async fn insert_key(&self, key: NewApiKeyRecord) -> Result<(), MemoryError>;
}

pub async fn create_api_key(
    port: &(impl ApiKeyIssuancePort + ?Sized),
    command: CreateApiKeyCommand,
    now: DateTime<Utc>,
) -> Result<CreatedApiKey, MemoryError> {
    let owner = port.owner(&command.account_id).await?.ok_or_else(|| {
        MemoryError::NotFound(format!("account {} not found", command.account_id))
    })?;
    let cap = port
        .active_key_cap(&owner.tenant_id, owner.plan_version)
        .await?;
    let secret = random_token();
    let id = new_api_key_id();
    let expires_at = command
        .expires_in_days
        .map(|days| now + chrono::Duration::days(i64::from(days)));
    port.insert_key(NewApiKeyRecord {
        id: id.clone(),
        account_id: command.account_id.clone(),
        name: command.name.clone(),
        expires_at,
        secret: secret.clone(),
        cap,
        now,
    })
    .await?;
    Ok(CreatedApiKey {
        id,
        secret,
        name: command.name,
        expires_at,
    })
}

fn random_token() -> String {
    hex::encode(rand::random::<[u8; 32]>())
}

fn new_api_key_id() -> String {
    format!("ak_{}", hex::encode(rand::random::<[u8; 12]>()))
}

/// The Tenant lifecycle transition table.
///
/// Which moves between Tenant statuses are legal is a fact about a Tenant, not
/// about HTTP, so it lives here rather than in the transport adapter that used
/// to hold it. ADR-0066 is the general statement; this is its second instance.
///
/// Anything outside the table is a programmer error and surfaces as
/// [`MemoryError::Validation`] with both ends named, because that message is
/// how an operator finds the offending pair.
pub fn can_transition(from: TenantStatus, to: TenantStatus) -> bool {
    use TenantStatus::{
        Deleting, Failed, Migrating, NamespaceCreating, Purged, Ready, Reserved, Suspended,
    };
    match (from, to) {
        (Reserved, NamespaceCreating) => true,
        (NamespaceCreating, Migrating) => true,
        (NamespaceCreating, Failed) => true,
        (Migrating, Ready) => true,
        (Migrating, Failed) => true,
        (Ready, Suspended) => true,
        (Suspended, Ready) => true,
        // `Deleting` is reachable from every non-terminal state, and `Purged`
        // only from `Deleting`. Purged is the one edge that destroys records,
        // so a `Purged -> X` edge would bring back a tenant whose rows are
        // gone. `tests/tenant_lifecycle.rs` asserts the table's shape rather
        // than its cases: a blanket rule is the kind that grows when a variant
        // is added, and a shape assertion catches that.
        (Reserved | NamespaceCreating | Migrating | Ready | Suspended | Failed, Deleting) => true,
        (Deleting, Purged) => true,
        // Retry from Failed: a worker may re-enter either working stage.
        (Failed, NamespaceCreating) => true,
        (Failed, Migrating) => true,
        _ => false,
    }
}

/// The error a refused pair produces.
fn refuse(from: TenantStatus, to: TenantStatus) -> MemoryError {
    MemoryError::Validation(format!("provisioning transition {from:?}->{to:?}"))
}

/// The two operations a Tenant's lifecycle needs from a store.
///
/// Narrower than `TenantStore`, which has eleven methods and answers questions
/// about tenancy that a transition does not ask. Two is what the use cases
/// need, and a use case taking the whole trait could reach account and session
/// state through a status change.
#[async_trait::async_trait]
pub trait TenantLifecyclePort: Send + Sync {
    /// Compare-and-set the Tenant's status on `expected_version`.
    async fn update_tenant_state(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
    ) -> Result<u64, MemoryError>;

    /// The same, fenced: the predicate also carries owner, lease and
    /// generation, so a worker whose lease was reassigned cannot advance the
    /// Tenant it no longer owns.
    async fn update_tenant_state_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
        lease: &ProvisioningLease,
    ) -> Result<u64, MemoryError>;
}

/// Adapts the registry's `TenantStore` to the two methods a transition needs.
///
/// A struct rather than a blanket impl, for a specific reason rather than a
/// general one: `TenantStore` requires `+ 'static`, and its fenced method
/// borrows through `LeaseFence<'a>`, which `async_trait` cannot lift into a
/// `Box<dyn Future + 'static>` when the lease is a borrowed parameter of the
/// caller's trait. Owning the borrow inside an adapter struct puts it in the
/// same scope as the future that uses it.
///
/// It is a struct rather than an impl on the store because `TenantStore` lives
/// under `http/`, which is the direction ADR-0058 forbids a bounded context
/// from importing. Here the edge runs from `provisioning` to an `http` *type*
/// the caller supplies, which is the composition root's job.
pub struct TenantLifecycle<'a> {
    store: &'a dyn crate::http::registry::storage::TenantStore,
}

impl<'a> TenantLifecycle<'a> {
    /// Borrow a store as the narrow port the transitions take.
    pub fn new(store: &'a dyn crate::http::registry::storage::TenantStore) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl TenantLifecyclePort for TenantLifecycle<'_> {
    async fn update_tenant_state(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
    ) -> Result<u64, MemoryError> {
        self.store
            .update_tenant_state(tenant_id, expected_version, from, to)
            .await
    }

    async fn update_tenant_state_fenced(
        &self,
        tenant_id: &str,
        expected_version: u64,
        from: TenantStatus,
        to: TenantStatus,
        lease: &ProvisioningLease,
    ) -> Result<u64, MemoryError> {
        self.store
            .update_tenant_state_fenced(
                tenant_id,
                expected_version,
                from,
                to,
                &crate::http::registry::storage::LeaseFence::from_lease(lease),
            )
            .await
    }
}

/// Move a Tenant from one status to another.
///
/// The pair is validated **before** the compare-and-set, so an illegal pair
/// never reaches storage and never consumes a version. That ordering is the
/// point of the function existing: the store's CAS is on `expected_version`,
/// not on `from`, so a caller that skips this check can write
/// `Migrating -> Ready` and leave a provisioning worker believing it still
/// holds a lease on a tenant it has lost.
pub async fn transition_tenant(
    port: &(impl TenantLifecyclePort + ?Sized),
    tenant_id: &str,
    expected_version: u64,
    from: TenantStatus,
    to: TenantStatus,
) -> Result<u64, MemoryError> {
    if !can_transition(from, to) {
        return Err(refuse(from, to));
    }
    port.update_tenant_state(tenant_id, expected_version, from, to)
        .await
}

/// Move a Tenant under a provisioning lease.
///
/// Same validation, same ordering; the lease travels to the store, which
/// applies it inside its own predicate rather than as a second round trip.
pub async fn transition_tenant_fenced(
    port: &(impl TenantLifecyclePort + ?Sized),
    tenant_id: &str,
    expected_version: u64,
    from: TenantStatus,
    to: TenantStatus,
    lease: &ProvisioningLease,
) -> Result<u64, MemoryError> {
    if !can_transition(from, to) {
        return Err(refuse(from, to));
    }
    port.update_tenant_state_fenced(tenant_id, expected_version, from, to, lease)
        .await
}

#[cfg(test)]
mod tests {
    use super::client_fingerprint;

    /// The fingerprint decides whether a retried create resolves to the
    /// existing client or raises `409 idempotency_conflict`, so stability
    /// across a retry is the property the durable replay depends on.
    #[test]
    fn client_fingerprint_is_stable_for_identical_bodies() {
        let operation = uuid::Uuid::new_v4();
        assert_eq!(
            client_fingerprint(operation, "team-alpha"),
            client_fingerprint(operation, "team-alpha")
        );
    }

    #[test]
    fn client_fingerprint_changes_with_body() {
        let operation = uuid::Uuid::new_v4();
        assert_ne!(
            client_fingerprint(operation, "team-alpha"),
            client_fingerprint(operation, "team-beta")
        );
        assert_ne!(
            client_fingerprint(operation, "team-alpha"),
            client_fingerprint(uuid::Uuid::new_v4(), "team-alpha")
        );
    }

    /// The domain-separation tag is what keeps a client-create fingerprint
    /// from ever colliding with a key-issue one for the same body.
    #[test]
    fn client_fingerprint_is_domain_separated() {
        let operation = uuid::Uuid::new_v4();
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut hasher, b"local_admin_client_create\0");
        sha2::Digest::update(&mut hasher, operation.as_bytes());
        sha2::Digest::update(&mut hasher, b"\0");
        sha2::Digest::update(&mut hasher, b"team-alpha");
        let expected: [u8; 32] = sha2::Digest::finalize(hasher).into();
        assert_eq!(client_fingerprint(operation, "team-alpha"), expected);
    }
}
