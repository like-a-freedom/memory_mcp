use std::sync::Arc;

use crate::service::local_admin::contracts::{
    AdminLogin, AdminPrincipal, AttemptDecision, AttemptDomain, AttemptInput, AuthAttemptContext,
    BrowserPolicyFence, ChallengeFinish, ChallengeIssue, ChallengeKind, ChallengeView,
    FailureAction, FailureAudit, FailureReason, LocalAdminError, LocalAdminStore,
    LocalKeyFingerprints, LocalResult, OneTimeChallenge, RequestContext, SessionOpen,
    SessionRotate,
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
        self.issue_challenge(username, ChallengeKind::Activate, request)
            .await
    }

    /// Recover an admin (CLI path). Invalidates sessions, issues reset code.
    pub async fn recover_admin(
        &self,
        username: &str,
        request: &RequestContext,
    ) -> LocalResult<OneTimeChallenge> {
        // The store implementation handles the generation increment, session
        // revocation and old-challenge revocation for a reset.
        self.issue_challenge(username, ChallengeKind::Reset, request)
            .await
    }

    /// Issue one challenge for `username` and return the code to hand over.
    ///
    /// Only the code's verifier reaches the store: the code itself is returned
    /// to the caller (CLI stdout) and never persisted (see
    /// [`challenge_verifier`]).
    async fn issue_challenge(
        &self,
        username: &str,
        kind: ChallengeKind,
        request: &RequestContext,
    ) -> LocalResult<OneTimeChallenge> {
        let code_bytes = generate_random_32();
        let verifier = challenge_verifier(self.authority.session_key(), &code_bytes)?;
        let command = ChallengeIssue {
            username: username.to_string(),
            kind,
            verifier,
            policy: self.authority.policy().clone(),
            request: request.clone(),
        };
        let issued = self.authority.store().issue_challenge(command).await?;
        Ok(OneTimeChallenge {
            issued,
            code: hex::encode(code_bytes),
        })
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
        let normalized = crate::service::local_admin::policy::normalize_peer_ip(source);
        bucket(
            &self.attempt_key,
            b"source\0",
            normalized.to_string().as_bytes(),
        )
    }

    /// Record the single admitted-failure audit event for an attempt this
    /// service already reserved.
    ///
    /// Only a *reserved* authentication attempt appends an event (plan
    /// §3.1). Pre-admission, session and client-mutation denials are
    /// counted by the rate buckets instead, so there is deliberately no
    /// append-per-denial path.
    ///
    /// A storage failure while writing the event is reported as
    /// `Unavailable` (503) rather than the caller's rejection: spec §10
    /// requires that a failed-login audit write is never silently
    /// dropped, and a sanitized 503 is the only answer that is both
    /// fail-closed and honest about the lost security event.
    async fn record_admitted_failure(
        &self,
        context: &AuthAttemptContext,
        action: FailureAction,
        reason: FailureReason,
        admin_id: Option<String>,
        username: Option<&str>,
    ) -> LocalResult<()> {
        let username_bucket = match username {
            Some(raw) => Some(self.username_bucket(raw)?),
            None => None,
        };
        let event = FailureAudit {
            request: context.request.clone(),
            policy: self.authority.policy().clone(),
            action,
            reason,
            admin_id,
            username_bucket,
            source_bucket: Some(self.source_bucket(context.source)?),
            admitted_auth_attempt: true,
        };
        self.authority
            .store()
            .record_failure(event)
            .await
            .map_err(|error| match error {
                LocalAdminError::Infrastructure(_) => LocalAdminError::Unavailable,
                other => other,
            })
    }

    /// Inspect a challenge code without consuming it.
    pub async fn inspect_challenge(
        &self,
        context: &AuthAttemptContext,
        code: &str,
        kind: ChallengeKind,
    ) -> LocalResult<ChallengeView> {
        self.admit(context, AttemptDomain::Challenge, None).await?;
        let verifier = match parse_challenge_code(code)
            .and_then(|bytes| challenge_verifier(self.authority.session_key(), &bytes))
        {
            Ok(verifier) => verifier,
            Err(error) => return self.reject_invalid_challenge(context, error).await,
        };
        match self
            .authority
            .store()
            .inspect_challenge(&verifier, kind, self.authority.policy())
            .await
        {
            Ok(view) => Ok(view),
            Err(error) => self.reject_challenge(context, error).await,
        }
    }

    /// Record the admitted failure for a rejected challenge code, then hand the
    /// original rejection back. Used by both the inspect and finish paths so
    /// the event they append cannot drift apart.
    async fn reject_invalid_challenge<T>(
        &self,
        context: &AuthAttemptContext,
        error: LocalAdminError,
    ) -> LocalResult<T> {
        self.record_admitted_failure(
            context,
            FailureAction::Challenge,
            FailureReason::InvalidChallenge,
            None,
            None,
        )
        .await?;
        Err(error)
    }

    /// Record the admitted failure when a challenge operation is refused
    /// for a reason that is about the presented code, then hand the
    /// original rejection back. Storage and availability errors are not
    /// credential rejections and carry no closed-enum reason, so they are
    /// propagated unchanged.
    async fn reject_challenge<T>(
        &self,
        context: &AuthAttemptContext,
        error: LocalAdminError,
    ) -> LocalResult<T> {
        if matches!(error, LocalAdminError::InvalidChallenge) {
            return self.reject_invalid_challenge(context, error).await;
        }
        Err(error)
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
        let verifier = match parse_challenge_code(code)
            .and_then(|bytes| challenge_verifier(self.authority.session_key(), &bytes))
        {
            Ok(verifier) => verifier,
            Err(error) => return self.reject_invalid_challenge(context, error).await,
        };
        let password_phc = self.hasher.hash(password).await?;
        let command = ChallengeFinish {
            verifier,
            kind,
            password_phc,
            policy: self.authority.policy().clone(),
            request: context.request.clone(),
        };
        match self.authority.store().finish_challenge(command).await {
            Ok(()) => Ok(()),
            Err(error) => self.reject_challenge(context, error).await,
        }
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
                self.record_admitted_failure(
                    context,
                    FailureAction::Login,
                    FailureReason::InvalidCredentials,
                    None,
                    Some(username),
                )
                .await?;
                return Err(LocalAdminError::InvalidCredentials);
            }
        };
        let ok = self
            .hasher
            .verify(password, credential.password_phc.clone())
            .await?;
        if !ok {
            self.record_admitted_failure(
                context,
                FailureAction::Login,
                FailureReason::InvalidCredentials,
                Some(credential.admin_id.clone()),
                Some(username),
            )
            .await?;
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
    ///
    /// The ledger's signature carries the request context so a rejection can be
    /// attributed to a request even though resolution reads no
    /// request-scoped state; the store's transaction uses the cookie verifier
    /// and the current policy fence only.
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
        let credential = match credential {
            Some(credential) => credential,
            None => {
                self.record_admitted_failure(
                    context,
                    FailureAction::Reauth,
                    FailureReason::InvalidCredentials,
                    Some(principal.fence.admin_id.clone()),
                    Some(&principal.username),
                )
                .await?;
                return Err(LocalAdminError::InvalidCredentials);
            }
        };
        let ok = self
            .hasher
            .verify(password, credential.password_phc.clone())
            .await?;
        if !ok {
            self.record_admitted_failure(
                context,
                FailureAction::Reauth,
                FailureReason::InvalidCredentials,
                Some(principal.fence.admin_id.clone()),
                Some(&principal.username),
            )
            .await?;
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
    let digest = crate::service::credential_material::hmac_fingerprint(key, label, data);
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

fn generate_random_32() -> [u8; 32] {
    crate::service::credential_material::random_32()
}

/// Purpose label for the challenge-verifier HMAC. Fixed application text, never
/// configuration, so the derivation is stable while the key is.
const CHALLENGE_VERIFIER_LABEL: &[u8] = b"local_admin_challenge_verifier_v1";

/// Derive the durable form of a challenge code.
///
/// Spec §7 requires `local_admin_challenge` to hold an HMAC verifier and to
/// never store the raw code. The code is therefore not persisted anywhere: it
/// leaves the process once, as CLI stdout, and the row only lets a caller who
/// already holds the code prove that fact. Reading the registry (or a database
/// backup) cannot redeem a live activation or reset code inside its 900-second
/// window.
///
/// The derivation is keyed by the deployment's session key under a distinct
/// label, so it shares no domain with the session-cookie, CSRF or policy
/// fingerprint derivations. `new_from_slice` cannot fail for a fixed 32-byte
/// key; the unreachable case fails closed with a typed error rather than
/// persisting an inert verifier that any code could match.
fn challenge_verifier(session_key: &[u8; 32], code: &[u8; 32]) -> LocalResult<[u8; 32]> {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;

    let mut mac = HmacSha256::new_from_slice(session_key).map_err(|e| {
        LocalAdminError::InvalidInput(format!("challenge verifier key rejected: {e}"))
    })?;
    mac.update(CHALLENGE_VERIFIER_LABEL);
    mac.update(code);
    Ok(mac.finalize().into_bytes().into())
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
