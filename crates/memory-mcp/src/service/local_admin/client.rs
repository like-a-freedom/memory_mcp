use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::service::local_admin::contracts::{
    AdminFence, AdminKeyInsert, ClientBundle, ClientCreate, ClientStateAction, ClientView,
    KeyExpiry, KeyInsertOutcome, LocalAdminStore, LocalResult, Page, PageRequest, RequestContext,
};

/// Service for managing local clients and their API keys.
pub struct LocalClientService {
    store: Arc<dyn LocalAdminStore>,
}

impl LocalClientService {
    pub fn new(store: Arc<dyn LocalAdminStore>) -> Self {
        Self { store }
    }

    /// Create a new client.
    ///
    /// The request fingerprint is derived from the *canonical body*, not
    /// from randomness: a retry carrying the same `operation_id` and the
    /// same `display_name` must resolve to the existing resource rather
    /// than raising an idempotency conflict. A changed body under the
    /// same operation id is what produces `409 idempotency_conflict`.
    pub async fn create_client(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        command: ClientCreate,
        account: crate::http::registry::models::Account,
        tenant: crate::http::registry::models::Tenant,
    ) -> LocalResult<ClientView> {
        let fingerprint = client_request_fingerprint(command.operation_id, &command.display_name);
        let bundle = ClientBundle {
            account,
            tenant,
            display_name: command.display_name,
            operation_id: command.operation_id,
            request_fingerprint: fingerprint,
        };
        self.store.create_client(fence, request, bundle).await
    }

    /// List clients with pagination.
    pub async fn list_clients(
        &self,
        fence: &AdminFence,
        page: PageRequest,
    ) -> LocalResult<Page<ClientView>> {
        self.store.list_clients(fence, page).await
    }

    /// Get a single client by account id.
    pub async fn client(&self, fence: &AdminFence, account_id: &str) -> LocalResult<ClientView> {
        self.store.client(fence, account_id).await
    }

    /// List keys for a client.
    pub async fn list_keys(
        &self,
        fence: &AdminFence,
        account_id: &str,
        page: PageRequest,
    ) -> LocalResult<Page<crate::http::registry::models::ApiKeyMeta>> {
        self.store.list_client_keys(fence, account_id, page).await
    }

    /// Insert a client key.
    pub async fn insert_key(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        command: AdminKeyInsert,
    ) -> LocalResult<KeyInsertOutcome> {
        self.store.insert_client_key(fence, request, command).await
    }

    /// Revoke a client key.
    pub async fn revoke_key(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        account_id: &str,
        key_id: &str,
    ) -> LocalResult<()> {
        self.store
            .revoke_client_key(fence, request, account_id, key_id)
            .await
    }

    /// Suspend or resume a client.
    pub async fn set_client_state(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        account_id: &str,
        expected_version: u64,
        action: ClientStateAction,
    ) -> LocalResult<()> {
        self.store
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
