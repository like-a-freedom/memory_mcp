use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::service::credential_material::generate_api_key_material;
use crate::service::local_admin::auth::LocalAdminAuthority;
use crate::service::local_admin::contracts::{
    AdminFence, AdminKeyCreate, AdminKeyInsert, ClientStateAction, ClientView, IssuedClientKey,
    KeyExpiry, KeyInsertOutcome, LocalAdminError, LocalAdminStore, LocalResult, Page, PageRequest,
    RequestContext,
};

/// Service for administering existing clients and their API keys.
///
/// Client *creation* is not here. It is provisioning's: `create_client` is
/// reached through `provisioning::api::create_client` and `ClientCreationPort`,
/// which builds the account/tenant bundle and owns the workflow. This service
/// kept a second copy of that bundle construction with no caller — a
/// controller that issued its own accounts rather than going through the
/// capability that owns client creation. It is deleted rather than wired up,
/// because a second live path to the same durable write is the thing this
/// refactor is removing everywhere else.
pub struct ClientAdminService {
    authority: Arc<LocalAdminAuthority>,
    pepper: String,
}

impl ClientAdminService {
    /// `plan_version` is no longer taken. It existed only to build the reserved
    /// tenant bundle, which creation now does through provisioning.
    pub fn new(authority: Arc<LocalAdminAuthority>, pepper: String) -> Self {
        Self { authority, pepper }
    }

    fn store(&self) -> &Arc<dyn LocalAdminStore> {
        self.authority.store()
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

/// The client-create fingerprint lives in [`crate::provisioning::api`], which
/// computes it on the live create path. This service no longer creates
/// clients, so it has no body to fingerprint; the two were byte-identical,
/// including the domain-separation tag, so a retry written against either
/// resolved to the same value while both existed.
///
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
