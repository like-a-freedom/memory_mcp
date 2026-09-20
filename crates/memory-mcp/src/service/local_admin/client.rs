use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::service::credential_material::generate_api_key_material;
use crate::service::local_admin::auth::LocalAdminAuthority;
use crate::service::local_admin::contracts::{
    AdminFence, AdminKeyCreate, AdminKeyInsert, ClientBundle, ClientCreate, ClientStateAction,
    ClientView, IssuedClientKey, KeyExpiry, KeyInsertOutcome, LocalAdminError, LocalAdminStore,
    LocalResult, Page, PageRequest, RequestContext,
};

/// Service for managing local clients and their API keys.
///
/// Owns the deployment's default plan version and API-key pepper, so the HTTP
/// adapter only parses, authorizes and serializes: the account/tenant bundle,
/// the credential material and the request fingerprints are all produced here
/// (plan §3.2, §3.4).
pub struct ClientAdminService {
    authority: Arc<LocalAdminAuthority>,
    plan_version: u32,
    pepper: String,
}

impl ClientAdminService {
    pub fn new(authority: Arc<LocalAdminAuthority>, plan_version: u32, pepper: String) -> Self {
        Self {
            authority,
            plan_version,
            pepper,
        }
    }

    fn store(&self) -> &Arc<dyn LocalAdminStore> {
        self.authority.store()
    }

    /// Create a new client: the Account, its `Reserved` Tenant at the
    /// deployment's default plan version, and the metadata sidecar, as one
    /// store transaction.
    ///
    /// The request fingerprint is derived from the *canonical body*, not from
    /// randomness: a retry carrying the same `operation_id` and the same
    /// `display_name` must resolve to the existing resource rather than
    /// raising an idempotency conflict. A changed body under the same
    /// operation id is what produces `409 idempotency_conflict`.
    pub async fn create(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        command: ClientCreate,
    ) -> LocalResult<ClientView> {
        use crate::http::registry::models as registry;

        let account_id = registry::new_account_id();
        let tenant_id = registry::new_tenant_id();
        let now = chrono::Utc::now();
        let fingerprint = client_request_fingerprint(command.operation_id, &command.display_name);
        let bundle = ClientBundle {
            account: registry::Account {
                id: account_id,
                status: registry::AccountStatus::Active,
                tenant_id: tenant_id.clone(),
                created_at: now,
            },
            tenant: registry::Tenant {
                id: tenant_id,
                status: registry::TenantStatus::Reserved,
                namespace_binding: registry::NamespaceBinding {
                    namespace: registry::new_namespace_name(),
                    database: "memory".into(),
                },
                plan_version: self.plan_version,
                schema_version: 0,
                retry_stage: None,
                provisioning_lease: None,
                created_at: now,
                version: 0,
            },
            display_name: command.display_name,
            operation_id: command.operation_id,
            request_fingerprint: fingerprint,
        };
        self.store().create_client(fence, request, bundle).await
    }

    /// List clients with pagination (plan §3.4 `list`).
    pub async fn list(
        &self,
        fence: &AdminFence,
        page: PageRequest,
    ) -> LocalResult<Page<ClientView>> {
        self.store().list_clients(fence, page).await
    }

    /// Get a single client by account id (plan §3.4 `get`).
    pub async fn get(&self, fence: &AdminFence, account_id: &str) -> LocalResult<ClientView> {
        self.store().client(fence, account_id).await
    }

    /// List keys for a client (plan §3.4 `keys`).
    pub async fn keys(
        &self,
        fence: &AdminFence,
        account_id: &str,
        page: PageRequest,
    ) -> LocalResult<Page<crate::http::registry::models::ApiKeyMeta>> {
        self.store().list_client_keys(fence, account_id, page).await
    }

    /// Issue a client key, returning the revealed-once credential.
    ///
    /// The material is generated here, before admission, and discarded unless
    /// the store reports a fresh insert: a replayed operation resolves to
    /// `409 secret_already_issued` carrying the existing public key id, never a
    /// second secret.
    pub async fn issue_key(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        account_id: &str,
        command: AdminKeyCreate,
    ) -> LocalResult<IssuedClientKey> {
        let (key_id, verifier, secret) = generate_api_key_material(self.pepper.as_bytes());
        let insert = AdminKeyInsert {
            account_id: account_id.to_owned(),
            key_id: key_id.clone(),
            name: command.name.clone(),
            verifier,
            expiry: command.expiry.clone(),
            operation_id: command.operation_id,
            request_fingerprint: key_request_fingerprint(
                command.operation_id,
                &command.name,
                &command.expiry,
            ),
        };
        match self
            .store()
            .insert_client_key(fence, request, insert)
            .await?
        {
            KeyInsertOutcome::Created(meta) => Ok(IssuedClientKey {
                id: meta.id,
                name: meta.name,
                secret,
                expires_at: meta.expires_at,
            }),
            KeyInsertOutcome::AlreadyIssued { key_id } => {
                Err(LocalAdminError::SecretAlreadyIssued { key_id })
            }
        }
    }

    /// Revoke a client key.
    pub async fn revoke_key(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        account_id: &str,
        key_id: &str,
    ) -> LocalResult<()> {
        self.store()
            .revoke_client_key(fence, request, account_id, key_id)
            .await
    }

    /// Suspend or resume a client (plan §3.4 `set_state`).
    pub async fn set_state(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        account_id: &str,
        expected_version: u64,
        action: ClientStateAction,
    ) -> LocalResult<()> {
        self.store()
            .set_client_state(fence, request, account_id, expected_version, action)
            .await
    }
}

/// Canonical, deterministic fingerprint of a client-create body.
pub fn client_request_fingerprint(operation_id: uuid::Uuid, display_name: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"local_admin_client_create\0");
    hasher.update(operation_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(display_name.as_bytes());
    hasher.finalize().into()
}

/// Canonical, deterministic fingerprint of a key-issue body.
pub fn key_request_fingerprint(
    operation_id: uuid::Uuid,
    name: &str,
    expiry: &KeyExpiry,
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"local_admin_key_create\0");
    hasher.update(operation_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(name.as_bytes());
    hasher.update(b"\0");
    match expiry {
        KeyExpiry::Never => hasher.update(b"never"),
        KeyExpiry::Days { days } => {
            hasher.update(b"days\0");
            hasher.update(days.to_be_bytes());
        }
    }
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_fingerprint_is_stable_for_identical_bodies() {
        let operation = uuid::Uuid::new_v4();
        assert_eq!(
            client_request_fingerprint(operation, "team-alpha"),
            client_request_fingerprint(operation, "team-alpha")
        );
    }

    #[test]
    fn client_fingerprint_changes_with_body() {
        let operation = uuid::Uuid::new_v4();
        assert_ne!(
            client_request_fingerprint(operation, "team-alpha"),
            client_request_fingerprint(operation, "team-beta")
        );
        assert_ne!(
            client_request_fingerprint(operation, "team-alpha"),
            client_request_fingerprint(uuid::Uuid::new_v4(), "team-alpha")
        );
    }

    #[test]
    fn key_fingerprint_distinguishes_expiry_choices() {
        let operation = uuid::Uuid::new_v4();
        assert_ne!(
            key_request_fingerprint(operation, "k", &KeyExpiry::Never),
            key_request_fingerprint(operation, "k", &KeyExpiry::Days { days: 30 })
        );
        assert_ne!(
            key_request_fingerprint(operation, "k", &KeyExpiry::Days { days: 30 }),
            key_request_fingerprint(operation, "k", &KeyExpiry::Days { days: 31 })
        );
    }
}
