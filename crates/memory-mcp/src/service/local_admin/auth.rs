use std::sync::Arc;

use crate::service::local_admin::contracts::{
    AdminLogin, AdminPrincipal, AttemptDecision, AttemptDomain, AttemptInput, AuthAttemptContext,
    BrowserPolicyFence, ChallengeFinish, ChallengeIssue, ChallengeKind, ChallengeView,
    LocalAdminError, LocalAdminStore, LocalKeyFingerprints, LocalResult, OneTimeChallenge,
    RequestContext, SessionOpen, SessionRotate,
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
        let fingerprints = compute_fingerprints(&session_key, &csrf_key)?;
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
#[allow(dead_code)]
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

/// Browser-facing auth service. Owns the PasswordHasher and the
/// mandatory admission throttle.
#[derive(Clone)]
pub struct LocalAdminService {
    authority: Arc<LocalAdminAuthority>,
    hasher: Arc<PasswordHasher>,
    /// Domain-separated key used only to bucket attempt inputs into a
    /// fixed number of throttle slots. Derived from the CSRF key so it
    /// is per-deployment rather than global.
    attempt_key: [u8; 32],
}

/// Number of throttle slots per dimension. Keyed inputs map into this
/// fixed range so the throttle table can never grow with offered input.
pub const ATTEMPT_BUCKET_SLOTS: u16 = 4096;

impl LocalAdminService {
    pub fn new(authority: Arc<LocalAdminAuthority>, hasher: Arc<PasswordHasher>) -> Self {
        let attempt_key = crate::service::credential_material::hmac_fingerprint(
            authority.csrf_key(),
            b"local_admin_attempt_bucket_key\0",
            b"",
        );
        Self {
            authority,
            hasher,
            attempt_key,
        }
    }

    /// Reserve one attempt slot **before** any credential lookup or KDF
    /// work. This is mandatory: every entry point that can consume
    /// password material or challenge material calls this first, so a
    /// saturation or storage outage cannot be bypassed by choosing a
    /// different route.
    ///
    /// `username` is the raw offered value. It is normalized when it
    /// parses, and keyed as bounded raw input when it does not, so an
    /// attacker cannot escape throttling by submitting malformed
    /// usernames.
    async fn admit(
        &self,
        context: &AuthAttemptContext,
        domain: AttemptDomain,
        username: Option<&str>,
    ) -> LocalResult<()> {
        let username_bucket = match domain {
            AttemptDomain::Credentials => {
                let raw = username.ok_or_else(|| {
                    LocalAdminError::InvalidInput("credentials domain requires a username".into())
                })?;
                Some(self.username_bucket(raw)?)
            }
            AttemptDomain::Challenge => None,
        };
        let input = AttemptInput {
            domain,
            username_bucket,
            source_bucket: self.source_bucket(context.source)?,
            policy: self.authority.policy().clone(),
            request: context.request.clone(),
        };
        match self.authority.store().reserve_attempt(input).await? {
            AttemptDecision::Allowed => Ok(()),
            AttemptDecision::Limited {
                retry_after_seconds,
            } => Err(LocalAdminError::Throttled {
                retry_after_seconds,
            }),
        }
    }

    /// Bucket a normalized username, falling back to bounded raw input
    /// for values that do not satisfy the username policy.
    fn username_bucket(&self, raw: &str) -> LocalResult<u16> {
        let normalized = crate::service::local_admin::policy::normalize_username(raw)
            .unwrap_or_else(|_| bounded_raw(raw));
        bucket(&self.attempt_key, b"username\0", normalized.as_bytes())
    }

    /// Bucket a source address. IPv4-mapped IPv6 addresses are
    /// normalized to their IPv4 form so one client cannot appear as two
    /// independent buckets.
    fn source_bucket(&self, source: std::net::IpAddr) -> LocalResult<u16> {
        let normalized = match source {
            std::net::IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => std::net::IpAddr::V4(v4),
                None => std::net::IpAddr::V6(v6),
            },
            other => other,
        };
        bucket(
            &self.attempt_key,
            b"source\0",
            normalized.to_string().as_bytes(),
        )
    }

    /// Inspect a challenge code without consuming it.
    pub async fn inspect_challenge(
        &self,
        context: &AuthAttemptContext,
        code: &str,
        kind: ChallengeKind,
    ) -> LocalResult<ChallengeView> {
        self.admit(context, AttemptDomain::Challenge, None).await?;
        let verifier = parse_challenge_code(code)?;
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
        self.admit(context, AttemptDomain::Challenge, None).await?;
        crate::service::local_admin::policy::validate_password(&password)?;
        let verifier = parse_challenge_code(code)?;
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
        self.admit(context, AttemptDomain::Credentials, Some(username))
            .await?;
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
        let cookie = format!("__Host-memory_mcp_admin={}", hex::encode(cookie_verifier));
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
        self.admit(
            context,
            AttemptDomain::Credentials,
            Some(&principal.username),
        )
        .await?;
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
            hex::encode(new_cookie_verifier)
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

fn compute_fingerprints(
    session_key: &[u8; 32],
    csrf_key: &[u8; 32],
) -> LocalResult<LocalKeyFingerprints> {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let invalid = |e: hmac::digest::InvalidLength| {
        LocalAdminError::InvalidInput(format!("fingerprint key rejected: {e}"))
    };
    let session_fp = HmacSha256::new_from_slice(b"local_admin_session_fingerprint")
        .map_err(invalid)?
        .chain_update(session_key)
        .finalize()
        .into_bytes()
        .into();
    let csrf_fp = HmacSha256::new_from_slice(b"local_admin_csrf_fingerprint")
        .map_err(invalid)?
        .chain_update(csrf_key)
        .finalize()
        .into_bytes()
        .into();
    Ok(LocalKeyFingerprints {
        session: session_fp,
        csrf: csrf_fp,
    })
}

/// Map keyed input into the fixed throttle slot range.
fn bucket(key: &[u8; 32], label: &[u8], data: &[u8]) -> LocalResult<u16> {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    let mut mac = Hmac::<Sha256>::new_from_slice(key)
        .map_err(|e| LocalAdminError::InvalidInput(format!("throttle key rejected: {e}")))?;
    mac.update(label);
    mac.update(data);
    let digest = mac.finalize().into_bytes();
    Ok(u16::from_be_bytes([digest[0], digest[1]]) % ATTEMPT_BUCKET_SLOTS)
}

/// Bound raw input for keying when it fails username validation.
///
/// Truncating to 64 bytes keeps the keying cost constant regardless of
/// the offered length; the input is never stored or echoed.
fn bounded_raw(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let end = bytes.len().min(64);
    // `from_utf8_lossy` on a byte slice can split a scalar value at the
    // boundary; trimming to a char boundary first keeps the keying
    // deterministic and avoids replacement characters.
    let mut end = end;
    while end > 0 && !raw.is_char_boundary(end) {
        end -= 1;
    }
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[allow(dead_code)]
fn generate_random_32() -> [u8; 32] {
    let mut buf = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut buf);
    buf
}

#[allow(dead_code)]
fn parse_hex_32(hex_str: &str) -> LocalResult<[u8; 32]> {
    let bytes = hex::decode(hex_str)
        .map_err(|e| LocalAdminError::InvalidInput(format!("invalid hex: {e}")))?;
    bytes
        .try_into()
        .map_err(|_| LocalAdminError::InvalidInput("hex must be 32 bytes".into()))
}

/// Parse a challenge code.
///
/// Every malformed code collapses to `InvalidChallenge` so a caller
/// cannot distinguish "not hex", "wrong length" and "unknown code" —
/// spec §8 requires one identical `400 invalid_challenge` for all of
/// them, and a syntax error is not an account-existence oracle.
fn parse_challenge_code(hex_str: &str) -> LocalResult<[u8; 32]> {
    let bytes = hex::decode(hex_str).map_err(|_| LocalAdminError::InvalidChallenge)?;
    bytes
        .try_into()
        .map_err(|_| LocalAdminError::InvalidChallenge)
}

use rand_core::RngCore;
