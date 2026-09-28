use std::sync::Arc;

use crate::MemoryError;
use crate::http::config::HttpConfig;
use crate::http::registry::models::BrowserPolicyFence;
use crate::http::registry::storage::{AccountStore, SessionStore};
use crate::identity::api::{InvitationSession, InvitationSessionPort};

/// A first-login session for an invited account.
///
/// Two owner traits, and the pair is the whole of what it needs: it reads the
/// account to mint a session against, and it reads and writes the session. It
/// used to hold the registry, which also let it issue API keys.
pub(crate) struct ControlSessionInvitationAdapter {
    accounts: Arc<dyn AccountStore>,
    sessions: Arc<dyn SessionStore>,
    config: HttpConfig,
    policy: BrowserPolicyFence,
}

impl ControlSessionInvitationAdapter {
    pub(crate) fn new(
        accounts: Arc<dyn AccountStore>,
        sessions: Arc<dyn SessionStore>,
        config: HttpConfig,
        policy: BrowserPolicyFence,
    ) -> Self {
        Self {
            accounts,
            sessions,
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
        let Some(session) = self
            .sessions
            .find_session(&self.policy, &cookie_hash)
            .await?
        else {
            return Ok(false);
        };
        self.sessions
            .touch_session(&self.policy, &session.id, &cookie_hash)
            .await?;
        Ok(true)
    }

    async fn issue_session(&self, account_id: &str) -> Result<InvitationSession, MemoryError> {
        let account = self
            .accounts
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
        self.sessions.store_session(&self.policy, &session).await?;
        Ok(InvitationSession { cookie_value })
    }
}
