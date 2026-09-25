use std::sync::Arc;

use crate::MemoryError;
use crate::http::registry::models::{AccountStatus, NamespaceBinding, Tenant, TenantStatus};
use crate::http::registry::models::{ApiKey, ApiKeyStatus, KeyedVerifier};
use crate::http::registry::storage::RegistryStore;
use crate::provisioning::api::{
    ApiKeyIssuancePort, ApiKeyOwner, ClientCreation, ClientCreationError, ClientCreationPort,
    ClientView, NewApiKeyRecord,
};
use crate::service::local_admin::contracts::{
    AdminFence, ClientBundle, LocalAdminError, LocalAdminStore, RequestContext,
};

pub(crate) struct LocalAdminClientCreation {
    store: Arc<dyn LocalAdminStore>,
}

impl LocalAdminClientCreation {
    pub(crate) fn new(store: Arc<dyn LocalAdminStore>) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl ClientCreationPort for LocalAdminClientCreation {
    async fn create_client(
        &self,
        command: ClientCreation,
    ) -> Result<ClientView, ClientCreationError> {
        let authority = &command.authority;
        let fence = AdminFence {
            admin_id: authority.admin_id.clone(),
            session_id: authority.session_verifier.clone(),
            credential_generation: authority.credential_generation,
            policy: crate::service::local_admin::contracts::BrowserPolicyFence {
                methods: authority
                    .policy_methods
                    .iter()
                    .map(|method| match method {
                        crate::identity::api::AuthMethod::Local => {
                            crate::http::config::BrowserAuthMethod::Local
                        }
                        crate::identity::api::AuthMethod::Oidc => {
                            crate::http::config::BrowserAuthMethod::Oidc
                        }
                    })
                    .collect(),
                epoch: authority.policy_epoch,
            },
        };
        let bundle = ClientBundle {
            account: crate::http::registry::models::Account {
                id: command.account_id,
                status: AccountStatus::Active,
                tenant_id: command.tenant_id.clone(),
                created_at: command.now,
            },
            tenant: Tenant {
                id: command.tenant_id,
                status: TenantStatus::Reserved,
                namespace_binding: NamespaceBinding {
                    namespace: command.namespace,
                    database: command.database,
                },
                plan_version: command.plan_version,
                schema_version: command.schema_version,
                retry_stage: None,
                provisioning_lease: None,
                created_at: command.now,
                version: 0,
            },
            display_name: command.display_name,
            operation_id: command.operation_id,
            request_fingerprint: command.request_fingerprint,
        };
        let view = self
            .store
            .create_client(
                &fence,
                &RequestContext {
                    request_id: authority.request_id,
                },
                bundle,
            )
            .await
            .map_err(client_creation_error)?;
        Ok(ClientView {
            account_id: view.account_id,
            tenant_id: view.tenant_id,
            display_name: view.display_name,
            account_status: account_status_name(view.account_status),
            tenant_status: tenant_status_name(view.tenant_status),
            plan_version: view.plan_version,
            schema_version: view.schema_version,
            version: view.version,
            provisioning_reason: view.provisioning_reason,
        })
    }
}

fn account_status_name(status: AccountStatus) -> String {
    match status {
        AccountStatus::Active => "active".into(),
        AccountStatus::Suspended => "suspended".into(),
        AccountStatus::Deleting => "deleting".into(),
    }
}

fn tenant_status_name(status: TenantStatus) -> String {
    match status {
        TenantStatus::Reserved => "reserved".into(),
        TenantStatus::NamespaceCreating => "namespace_creating".into(),
        TenantStatus::Migrating => "migrating".into(),
        TenantStatus::Ready => "ready".into(),
        TenantStatus::Suspended => "suspended".into(),
        TenantStatus::Failed => "failed".into(),
        TenantStatus::Deleting => "deleting".into(),
        TenantStatus::Purged => "purged".into(),
    }
}

fn client_creation_error(error: LocalAdminError) -> ClientCreationError {
    match error {
        LocalAdminError::InvalidInput(message) => ClientCreationError::InvalidInput(message),
        LocalAdminError::ReauthRequired => ClientCreationError::ReauthenticationRequired,
        LocalAdminError::Unauthenticated => ClientCreationError::Unauthorized,
        LocalAdminError::Forbidden => ClientCreationError::Forbidden,
        LocalAdminError::NotFound => ClientCreationError::NotFound,
        LocalAdminError::IdempotencyConflict => ClientCreationError::IdempotencyConflict,
        LocalAdminError::Unavailable | LocalAdminError::Infrastructure(_) => {
            ClientCreationError::Unavailable
        }
        LocalAdminError::InvalidCredentials
        | LocalAdminError::InvalidChallenge
        | LocalAdminError::StateConflict
        | LocalAdminError::VersionConflict
        | LocalAdminError::KeyCap
        | LocalAdminError::SecretAlreadyIssued { .. }
        | LocalAdminError::Throttled { .. } => ClientCreationError::Unavailable,
    }
}

pub(crate) struct RegistryApiKeyIssuance {
    store: Arc<dyn RegistryStore>,
    pepper: String,
}

impl RegistryApiKeyIssuance {
    pub(crate) fn new(store: Arc<dyn RegistryStore>, pepper: String) -> Self {
        Self { store, pepper }
    }
}

#[async_trait::async_trait]
impl ApiKeyIssuancePort for RegistryApiKeyIssuance {
    async fn owner(&self, account_id: &str) -> Result<Option<ApiKeyOwner>, MemoryError> {
        let Some(account) = self.store.find_account_by_id(account_id).await? else {
            return Ok(None);
        };
        let Some(tenant) = self.store.find_tenant_by_id(&account.tenant_id).await? else {
            return Err(MemoryError::NotFound(format!(
                "tenant {} not found",
                account.tenant_id
            )));
        };
        Ok(Some(ApiKeyOwner {
            tenant_id: tenant.id,
            plan_version: tenant.plan_version,
        }))
    }

    async fn active_key_cap(
        &self,
        _tenant_id: &str,
        plan_version: u32,
    ) -> Result<u32, MemoryError> {
        Ok(self
            .store
            .load_plan(plan_version)
            .await?
            .limits
            .max_active_api_keys)
    }

    async fn insert_key(&self, key: NewApiKeyRecord) -> Result<(), MemoryError> {
        let api_key = ApiKey {
            id: key.id,
            account_id: key.account_id,
            name: key.name,
            verifier: KeyedVerifier::compute(self.pepper.as_bytes(), key.secret.as_bytes()),
            status: ApiKeyStatus::Active,
            created_at: key.now,
            expires_at: key.expires_at,
            last_used_at: None,
            version: 0,
        };
        self.store
            .create_api_key_if_below_limit(&api_key, key.cap)
            .await
    }
}
