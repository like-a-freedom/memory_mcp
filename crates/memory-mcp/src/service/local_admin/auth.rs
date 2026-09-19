use std::sync::Arc;

use crate::service::local_admin::contracts::{
    AdminLogin, AdminPrincipal, AuthAttemptContext, BrowserPolicyFence, ChallengeFinish,
    ChallengeIssue, ChallengeKind, ChallengeView, LocalAdminError, LocalAdminStore,
    LocalKeyFingerprints, LocalResult, OneTimeChallenge, RequestContext, SessionOpen, SessionRotate,
};
use crate::service::local_admin::password::PasswordHasher;

/// Shared authority holding store, keys, and the joined policy fence.
pub struct LocalAdminAuthority {
    store: Arc<dyn LocalAdminStore>,
    session_key: [u8; 32],
    csrf_key: [u8; 32],
    policy: BrowserPolicyFence,
}

impl LocalAdminAuthority {
    /// Join or verify the durable local policy. Computes fingerprints,
    /// joins the policy, and privately owns store/keys/fence.
    pub async fn join(
        store: Arc<dyn LocalAdminStore>,
        session_key: [u8; 32],
        csrf_key: [u8; 32],
    ) -> LocalResult<Arc<Self>> {
        let fingerprints = compute_fingerprints(&session_key, &csrf_key);
        let policy = store.join_local_policy(fingerprints).await?;
        Ok(Arc::new(Self {
            store,
            session_key,
            csrf_key,
            policy,
        }))
    }

    pub fn policy(&self) -> &BrowserPolicyFence {
        &self.policy
    }

    pub fn store(&self) -> &Arc<dyn LocalAdminStore> {
        &self.store
    }

    pub fn session_key(&self) -> &[u8; 32] {
        &self.session_key
    }

    pub fn csrf_key(&self) -> &[u8; 32] {
        &self.csrf_key
    }
}

/// CLI-facing management service. No PasswordHasher needed —
/// the CLI only issues activation/reset codes.
pub struct AdminManagementService {
    authority: Arc<LocalAdminAuthority>,
}

impl AdminManagementService {
    pub fn new(authority: Arc<LocalAdminAuthority>) -> Self {
        Self { authority }
    }

    /// Create a new admin (CLI path). Returns a one-time activation code.
    pub async fn create_admin(
        &self,
        username: &str,
        request: &RequestContext,
    ) -> LocalResult<OneTimeChallenge> {
        let verifier = generate_random_32();
        let command = ChallengeIssue {
            username: username.to_string(),
            kind: ChallengeKind::Activate,
            verifier,
            policy: self.authority.policy().clone(),
            request: request.clone(),
        };
        let issued = self.authority.store().issue_challenge(command).await?;
        let code = hex::encode(verifier);
        Ok(OneTimeChallenge { issued, code })
    }

    /// Recover an admin (CLI path). Invalidates sessions, issues reset code.
    pub async fn recover_admin(
        &self,
        username: &str,
        request: &RequestContext,
    ) -> LocalResult<OneTimeChallenge> {
        // Issue a reset challenge — the store implementation handles
        // generation increment, session revocation, and old challenge revocation.
        let verifier = generate_random_32();
        let command = ChallengeIssue {
            username: username.to_string(),
            kind: ChallengeKind::Reset,
            verifier,
            policy: self.authority.policy().clone(),
            request: request.clone(),
        };
        let issued = self.authority.store().issue_challenge(command).await?;
        let code = hex::encode(verifier);
        Ok(OneTimeChallenge { issued, code })
    }
}

/// Browser-facing auth service. Owns the PasswordHasher.
#[derive(Clone)]
pub struct LocalAdminService {
    authority: Arc<LocalAdminAuthority>,
    hasher: Arc<PasswordHasher>,
}

impl LocalAdminService {
    pub fn new(authority: Arc<LocalAdminAuthority>, hasher: Arc<PasswordHasher>) -> Self {
        Self { authority, hasher }
    }

    /// Inspect a challenge code without consuming it.
    pub async fn inspect_challenge(
        &self,
        _context: &AuthAttemptContext,
        code: &str,
        kind: ChallengeKind,
    ) -> LocalResult<ChallengeView> {
        let verifier = parse_hex_32(code)?;
        self.authority
            .store()
            .inspect_challenge(&verifier, kind, self.authority.policy())
            .await
    }

    /// Finish a challenge (activate or reset password).
    pub async fn finish_challenge(
        &self,
        context: &AuthAttemptContext,
        code: &str,
        kind: ChallengeKind,
        password: String,
    ) -> LocalResult<()> {
        crate::service::local_admin::policy::validate_password(&password)?;
        let verifier = parse_hex_32(code)?;
        let password_phc = self.hasher.hash(password).await?;
        let command = ChallengeFinish {
            verifier,
            kind,
            password_phc,
            policy: self.authority.policy().clone(),
            request: context.request.clone(),
        };
        self.authority.store().finish_challenge(command).await
    }

    /// Login with username/password.
    pub async fn login(
        &self,
        context: &AuthAttemptContext,
        username: &str,
        password: String,
    ) -> LocalResult<AdminLogin> {
        let credential = self
            .authority
            .store()
            .credential(username, self.authority.policy())
            .await?;
        let credential = match credential {
            Some(c) => c,
            None => {
                let _ = self.hasher.verify(password, None).await?;
                return Err(LocalAdminError::InvalidCredentials);
            }
        };
        let ok = self
            .hasher
            .verify(password, credential.password_phc.clone())
            .await?;
        if !ok {
            return Err(LocalAdminError::InvalidCredentials);
        }
        let cookie_verifier = generate_random_32();
        let command = SessionOpen {
            credential,
            cookie_verifier,
            policy: self.authority.policy().clone(),
            request: context.request.clone(),
        };
        let principal = self.authority.store().open_session(command).await?;
        let cookie = format!("__Host-memory_mcp_admin={}", hex::encode(&cookie_verifier));
        Ok(AdminLogin { principal, cookie })
    }

    /// Resolve a session from a cookie verifier.
    pub async fn resolve(
        &self,
        _request: &RequestContext,
        cookie_verifier: &[u8; 32],
    ) -> LocalResult<AdminPrincipal> {
        self.authority
            .store()
            .resolve_session(cookie_verifier, self.authority.policy())
            .await
    }

    /// Reauthenticate (rotate session with password).
    pub async fn reauthenticate(
        &self,
        context: &AuthAttemptContext,
        principal: &AdminPrincipal,
        password: String,
    ) -> LocalResult<AdminLogin> {
        let credential = self
            .authority
            .store()
            .credential(&principal.username, self.authority.policy())
            .await?;
        let credential = credential.ok_or(LocalAdminError::InvalidCredentials)?;
        let ok = self
            .hasher
            .verify(password, credential.password_phc.clone())
            .await?;
        if !ok {
            return Err(LocalAdminError::InvalidCredentials);
        }
        // Rotate session.
        let new_cookie_verifier = generate_random_32();
        let command = SessionRotate {
            fence: principal.fence.clone(),
            credential,
            cookie_verifier: new_cookie_verifier,
            request: context.request.clone(),
        };
        let new_principal = self.authority.store().rotate_session(command).await?;
        let cookie = format!(
            "__Host-memory_mcp_admin={}",
            hex::encode(&new_cookie_verifier)
        );
        Ok(AdminLogin {
            principal: new_principal,
            cookie,
        })
    }

    /// Logout (revoke session).
    pub async fn logout(
        &self,
        request: &RequestContext,
        principal: &AdminPrincipal,
    ) -> LocalResult<()> {
        self.authority
            .store()
            .revoke_session(&principal.fence, request)
            .await
    }
}

// ─── Helpers ──────────────────────────────────────────────

fn compute_fingerprints(session_key: &[u8; 32], csrf_key: &[u8; 32]) -> LocalKeyFingerprints {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let session_fp = HmacSha256::new_from_slice(b"local_admin_session_fingerprint")
        .expect("HMAC accepts any key")
        .chain_update(session_key)
        .finalize()
        .into_bytes()
        .into();
    let csrf_fp = HmacSha256::new_from_slice(b"local_admin_csrf_fingerprint")
        .expect("HMAC accepts any key")
        .chain_update(csrf_key)
        .finalize()
        .into_bytes()
        .into();
    LocalKeyFingerprints {
        session: session_fp,
        csrf: csrf_fp,
    }
}

fn generate_random_32() -> [u8; 32] {
    let mut buf = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut buf);
    buf
}

fn parse_hex_32(hex_str: &str) -> LocalResult<[u8; 32]> {
    let bytes = hex::decode(hex_str)
        .map_err(|e| LocalAdminError::InvalidInput(format!("invalid hex: {e}")))?;
    bytes
        .try_into()
        .map_err(|_| LocalAdminError::InvalidInput("hex must be 32 bytes".into()))
}

use rand_core::RngCore;
