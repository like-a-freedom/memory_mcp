use std::sync::Arc;

use crate::service::local_admin::contracts::{
    AdminFence, AdminKeyInsert, ClientBundle, ClientCreate, ClientStateAction, ClientView,
    KeyInsertOutcome, LocalAdminStore, LocalResult, Page, PageRequest, RequestContext,
};
use rand_core::RngCore;

/// Service for managing local clients and their API keys.
pub struct LocalClientService {
    store: Arc<dyn LocalAdminStore>,
}

impl LocalClientService {
    pub fn new(store: Arc<dyn LocalAdminStore>) -> Self {
        Self { store }
    }

    /// Create a new client. Enforces the 32-client cap.
    pub async fn create_client(
        &self,
        fence: &AdminFence,
        request: &RequestContext,
        command: ClientCreate,
        account: crate::http::registry::models::Account,
        tenant: crate::http::registry::models::Tenant,
    ) -> LocalResult<ClientView> {
        let fingerprint = compute_request_fingerprint();
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
    pub async fn client(
        &self,
        fence: &AdminFence,
        account_id: &str,
    ) -> LocalResult<ClientView> {
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
        self.store.revoke_client_key(fence, request, account_id, key_id).await
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
        self.store.set_client_state(fence, request, account_id, expected_version, action).await
    }
}

fn compute_request_fingerprint() -> [u8; 32] {
    let mut buf = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut buf);
    buf
}
