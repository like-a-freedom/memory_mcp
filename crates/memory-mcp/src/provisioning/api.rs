use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

use crate::MemoryError;
use crate::identity::api::AuthMethod;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedApiKey {
    pub id: String,
    pub secret: String,
    pub name: String,
    pub expires_at: Option<DateTime<Utc>>,
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
