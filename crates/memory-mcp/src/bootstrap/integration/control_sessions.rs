use std::sync::Arc;

use crate::MemoryError;
use crate::http::config::HttpConfig;
use crate::http::registry::models::BrowserPolicyFence;
use crate::http::registry::storage::RegistryStore;
use crate::identity::api::{InvitationSession, InvitationSessionPort};

pub(crate) struct ControlSessionInvitationAdapter {
    store: Arc<dyn RegistryStore>,
    config: HttpConfig,
    policy: BrowserPolicyFence,
}

impl ControlSessionInvitationAdapter {
    pub(crate) fn new(
        store: Arc<dyn RegistryStore>,
        config: HttpConfig,
        policy: BrowserPolicyFence,
    ) -> Self {
        Self {
            store,
            config,
            policy,
        }
    }
}

#[async_trait::async_trait]
impl InvitationSessionPort for ControlSessionInvitationAdapter {
    async fn has_valid_session(
        &self,
        _account_id: &str,
        cookie: Option<&str>,
    ) -> Result<bool, MemoryError> {
        let Some(cookie) = cookie else {
            return Ok(false);
        };
        let cookie_hash = hex::encode(crate::control::session::keyed_session_hash(
            &self.config.keys.control_plane_session,
            cookie.as_bytes(),
        )?);
        let Some(session) = self.store.find_session(&self.policy, &cookie_hash).await? else {
            return Ok(false);
        };
        self.store
            .touch_session(&self.policy, &session.id, &cookie_hash)
            .await?;
        Ok(true)
    }

    async fn issue_session(&self, account_id: &str) -> Result<InvitationSession, MemoryError> {
        let account = self
            .store
            .find_account_by_id(account_id)
            .await?
            .ok_or_else(|| MemoryError::NotFound("invitation account not found".into()))?;
        let cookie_value = crate::control::session::generate_session_cookie_value();
        let session = crate::control::session::ControlPlaneSession::new(
            &account,
            &cookie_value,
            self.policy.epoch,
            &self.config,
        )?;
        self.store.store_session(&self.policy, &session).await?;
        Ok(InvitationSession { cookie_value })
    }
}
