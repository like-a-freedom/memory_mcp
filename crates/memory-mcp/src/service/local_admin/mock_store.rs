use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;

use super::contracts::{
    AdminFence, AdminKeyInsert, AdminPrincipal, AdminState, AttemptDecision, AttemptInput,
    BrowserPolicyFence, ChallengeFinish, ChallengeIssue, ChallengeKind, ChallengeView,
    ClientBundle, ClientStateAction, ClientView, CredentialSnapshot, FailureAudit, IssuedChallenge,
    KeyInsertOutcome, LocalAdminStore, LocalKeyFingerprints, LocalResult, Page, PageRequest,
    RequestContext, SessionOpen, SessionRotate,
};

/// In-memory mock implementation of `LocalAdminStore` for tests.
pub struct InMemoryLocalAdminStore {
    inner: Mutex<Inner>,
}

struct Inner {
    policy_fingerprint: Option<LocalKeyFingerprints>,
    policy_fence: Option<BrowserPolicyFence>,
    credentials: HashMap<String, CredentialSnapshot>,
    challenges: HashMap<[u8; 32], ChallengeRecord>,
    sessions: HashMap<[u8; 32], SessionRecord>,
    next_admin_id: u64,
    next_generation: u64,
}

struct ChallengeRecord {
    username: String,
    kind: ChallengeKind,
    created_at: chrono::DateTime<chrono::Utc>,
}

struct SessionRecord {
    admin_id: String,
    username: String,
    generation: u64,
}

impl InMemoryLocalAdminStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(Inner {
                policy_fingerprint: None,
                policy_fence: None,
                credentials: HashMap::new(),
                challenges: HashMap::new(),
                sessions: HashMap::new(),
                next_admin_id: 1,
                next_generation: 1,
            }),
        }
    }

    /// Insert a pre-created credential for testing.
    pub fn insert_credential(&self, username: String, credential: CredentialSnapshot) {
        let mut inner = self.inner.lock().unwrap();
        inner.credentials.insert(username, credential);
    }
}

#[async_trait]
impl LocalAdminStore for InMemoryLocalAdminStore {
    async fn join_local_policy(
        &self,
        fingerprints: LocalKeyFingerprints,
    ) -> LocalResult<BrowserPolicyFence> {
        let mut inner = self.inner.lock().unwrap();
        if let Some(existing) = &inner.policy_fingerprint {
            if existing.session != fingerprints.session || existing.csrf != fingerprints.csrf {
                return Err(super::contracts::LocalAdminError::StateConflict);
            }
        } else {
            inner.policy_fingerprint = Some(fingerprints);
        }
        let fence = inner.policy_fence.clone().unwrap_or(BrowserPolicyFence {
            mode: super::contracts::BrowserAuthMode::Local,
            epoch: 1,
        });
        inner.policy_fence = Some(fence.clone());
        Ok(fence)
    }

    async fn issue_challenge(&self, command: ChallengeIssue) -> LocalResult<IssuedChallenge> {
        let mut inner = self.inner.lock().unwrap();
        inner.challenges.insert(
            command.verifier,
            ChallengeRecord {
                username: command.username.clone(),
                kind: command.kind,
                created_at: chrono::Utc::now(),
            },
        );
        Ok(IssuedChallenge {
            admin_id: "test-admin".to_string(),
            username: command.username,
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(900),
        })
    }

    async fn inspect_challenge(
        &self,
        verifier: &[u8; 32],
        kind: ChallengeKind,
        _policy: &BrowserPolicyFence,
    ) -> LocalResult<ChallengeView> {
        let inner = self.inner.lock().unwrap();
        let record = inner
            .challenges
            .get(verifier)
            .ok_or(super::contracts::LocalAdminError::NotFound)?;
        if record.kind != kind {
            return Err(super::contracts::LocalAdminError::NotFound);
        }
        Ok(ChallengeView {
            username: record.username.clone(),
            expires_at: record.created_at + chrono::Duration::seconds(900),
        })
    }

    async fn finish_challenge(&self, command: ChallengeFinish) -> LocalResult<()> {
        let mut inner = self.inner.lock().unwrap();
        let record = inner
            .challenges
            .remove(&command.verifier)
            .ok_or(super::contracts::LocalAdminError::NotFound)?;
        if record.kind != command.kind {
            return Err(super::contracts::LocalAdminError::NotFound);
        }
        let admin_id = format!("admin-{}", inner.next_admin_id);
        let username = record.username;
        let generation = inner.next_generation;
        inner.credentials.insert(
            username.clone(),
            CredentialSnapshot {
                admin_id,
                username,
                state: AdminState::Active,
                credential_generation: generation,
                password_phc: Some(command.password_phc),
            },
        );
        inner.next_admin_id += 1;
        Ok(())
    }

    async fn credential(
        &self,
        username: &str,
        _policy: &BrowserPolicyFence,
    ) -> LocalResult<Option<CredentialSnapshot>> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.credentials.get(username).cloned())
    }

    async fn open_session(&self, command: SessionOpen) -> LocalResult<AdminPrincipal> {
        let mut inner = self.inner.lock().unwrap();
        let generation = command.credential.credential_generation;
        inner.sessions.insert(
            command.cookie_verifier,
            SessionRecord {
                admin_id: command.credential.admin_id.clone(),
                username: command.credential.username.clone(),
                generation,
            },
        );
        Ok(AdminPrincipal {
            fence: AdminFence {
                admin_id: command.credential.admin_id,
                session_id: hex::encode(command.cookie_verifier),
                credential_generation: generation,
                policy: BrowserPolicyFence {
                    mode: super::contracts::BrowserAuthMode::Local,
                    epoch: 1,
                },
            },
            username: command.credential.username,
            auth_time: chrono::Utc::now(),
            absolute_expiry: chrono::Utc::now() + chrono::Duration::hours(24),
        })
    }

    async fn resolve_session(
        &self,
        cookie_verifier: &[u8; 32],
        _policy: &BrowserPolicyFence,
    ) -> LocalResult<AdminPrincipal> {
        let inner = self.inner.lock().unwrap();
        let record = inner
            .sessions
            .get(cookie_verifier)
            .ok_or(super::contracts::LocalAdminError::Unauthenticated)?;
        Ok(AdminPrincipal {
            fence: AdminFence {
                admin_id: record.admin_id.clone(),
                session_id: hex::encode(cookie_verifier),
                credential_generation: record.generation,
                policy: BrowserPolicyFence {
                    mode: super::contracts::BrowserAuthMode::Local,
                    epoch: 1,
                },
            },
            username: record.username.clone(),
            auth_time: chrono::Utc::now(),
            absolute_expiry: chrono::Utc::now() + chrono::Duration::hours(24),
        })
    }

    async fn rotate_session(&self, command: SessionRotate) -> LocalResult<AdminPrincipal> {
        let mut inner = self.inner.lock().unwrap();
        let new_verifier = command.cookie_verifier;
        let new_gen = command.credential.credential_generation + 1;
        inner.sessions.insert(
            new_verifier,
            SessionRecord {
                admin_id: command.credential.admin_id.clone(),
                username: command.credential.username.clone(),
                generation: new_gen,
            },
        );
        Ok(AdminPrincipal {
            fence: AdminFence {
                admin_id: command.credential.admin_id,
                session_id: hex::encode(new_verifier),
                credential_generation: new_gen,
                policy: BrowserPolicyFence {
                    mode: super::contracts::BrowserAuthMode::Local,
                    epoch: 1,
                },
            },
            username: command.credential.username,
            auth_time: chrono::Utc::now(),
            absolute_expiry: chrono::Utc::now() + chrono::Duration::hours(24),
        })
    }

    async fn revoke_session(
        &self,
        fence: &AdminFence,
        _request: &RequestContext,
    ) -> LocalResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .sessions
            .retain(|_, record| record.admin_id != fence.admin_id);
        Ok(())
    }

    async fn reserve_attempt(&self, _input: AttemptInput) -> LocalResult<AttemptDecision> {
        Ok(AttemptDecision::Allowed)
    }

    async fn record_failure(&self, _event: FailureAudit) -> LocalResult<()> {
        Ok(())
    }

    // ── Client methods ──

    async fn create_client(
        &self,
        _fence: &AdminFence,
        _request: &RequestContext,
        _bundle: ClientBundle,
    ) -> LocalResult<ClientView> {
        Err(super::contracts::LocalAdminError::Unavailable)
    }

    async fn list_clients(
        &self,
        _fence: &AdminFence,
        _page: PageRequest,
    ) -> LocalResult<Page<ClientView>> {
        Ok(Page {
            items: vec![],
            next_cursor: None,
        })
    }

    async fn client(
        &self,
        _fence: &AdminFence,
        _account_id: &str,
    ) -> LocalResult<ClientView> {
        Err(super::contracts::LocalAdminError::NotFound)
    }

    async fn list_client_keys(
        &self,
        _fence: &AdminFence,
        _account_id: &str,
        _page: PageRequest,
    ) -> LocalResult<Page<crate::http::registry::models::ApiKeyMeta>> {
        Ok(Page {
            items: vec![],
            next_cursor: None,
        })
    }

    async fn insert_client_key(
        &self,
        _fence: &AdminFence,
        _request: &RequestContext,
        _command: AdminKeyInsert,
    ) -> LocalResult<KeyInsertOutcome> {
        Err(super::contracts::LocalAdminError::Unavailable)
    }

    async fn revoke_client_key(
        &self,
        _fence: &AdminFence,
        _request: &RequestContext,
        _account_id: &str,
        _key_id: &str,
    ) -> LocalResult<()> {
        Ok(())
    }

    async fn set_client_state(
        &self,
        _fence: &AdminFence,
        _request: &RequestContext,
        _account_id: &str,
        _expected_version: u64,
        _action: ClientStateAction,
    ) -> LocalResult<()> {
        Ok(())
    }
}
