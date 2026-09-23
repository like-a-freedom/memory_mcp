//! Same-origin API client for the local administrator surface.
//!
//! This module is separate from [`crate::api`] on purpose: the local admin
//! routes have their own JSON shapes, their own error envelope and two distinct
//! CSRF families (pre-auth and session bound). The OIDC/account client is left
//! untouched.
//!
//! Rules encoded here:
//!
//! * Every request is a relative, same-origin path. The `fetch` credentials
//!   mode is left at its default (`same-origin`) — gloo-net does not re-export
//!   `RequestCredentials`, and the default is exactly the required mode, so no
//!   `omit`/`include` override is ever applied.
//! * `204 No Content` is never handed to a JSON decoder.
//! * Credential POSTs (`login`, `activate`, `reset`, `reauth`) and key issuance
//!   are never retried automatically; nothing in this module loops on failure.
//! * No password, one-time code, CSRF token or key secret is written to
//!   `localStorage`, `sessionStorage` or `IndexedDB` — this module never touches
//!   those APIs, and secrets are redacted from `Debug`.
//! * Idempotency ids are generated once per deliberate user action, from the
//!   browser CSPRNG only.

use std::fmt;

use gloo_net::http::{Request, RequestBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

// ─── Routes ───────────────────────────────────────────────

/// `GET` — the enabled browser authentication methods,
/// `{"methods":["local","oidc"]}`.
pub const PATH_AUTH_CONFIG: &str = "/api/v1/auth/config";
/// `GET` — start the OIDC authorization-code flow. The server mounts it at
/// `/auth/oidc/authorize` (`http/router.rs`), **not** under `/api/v1`; a link
/// that guessed the API prefix would 404.
pub const PATH_OIDC_AUTHORIZE: &str = "/auth/oidc/authorize";
/// `GET` — short-lived pre-auth CSRF token.
pub const PATH_PREAUTH_CSRF: &str = "/api/v1/auth/local/csrf";
/// `POST` — validate a challenge code without consuming it.
pub const PATH_CHALLENGE: &str = "/api/v1/auth/local/challenge";
/// `POST` — activate the first administrator.
pub const PATH_ACTIVATE: &str = "/api/v1/auth/local/activate";
/// `POST` — reset an administrator password.
pub const PATH_RESET: &str = "/api/v1/auth/local/reset";
/// `POST` — establish an administrator session.
pub const PATH_LOGIN: &str = "/api/v1/auth/local/login";
/// `GET` — current administrator session.
pub const PATH_SESSION: &str = "/api/v1/admin/session";
/// `POST` — rotate the session after re-entering the password.
pub const PATH_REAUTH: &str = "/api/v1/admin/reauth";
/// `POST` — revoke the presented session.
pub const PATH_LOGOUT: &str = "/api/v1/admin/logout";
/// `GET`/`POST` — client collection.
pub const PATH_CLIENTS: &str = "/api/v1/admin/clients";

/// CSRF header required on every local mutation.
pub const HEADER_CSRF: &str = "X-CSRF-Token";
/// Idempotency header required on client creation and key issuance.
pub const HEADER_IDEMPOTENCY: &str = "Idempotency-Key";

/// Browser authentication method reporting local username/password login.
pub const METHOD_LOCAL: &str = "local";
/// Browser authentication method delegating to an external identity provider.
pub const METHOD_OIDC: &str = "oidc";

/// Default and maximum page size accepted by the backend.
pub const MAX_PAGE_LIMIT: u16 = 100;
/// Smallest page size the UI will request.
pub const MIN_PAGE_LIMIT: u16 = 1;

/// Bounds for an explicit day-count expiry.
pub const MIN_EXPIRY_DAYS: u32 = 1;
/// Bounds for an explicit day-count expiry.
pub const MAX_EXPIRY_DAYS: u32 = 3_650;

/// Maximum length of a display or key name, in Unicode scalar values.
pub const MAX_NAME_SCALARS: usize = 100;
/// Maximum length of a display or key name, in UTF-8 bytes.
pub const MAX_NAME_BYTES: usize = 400;

/// Longest backend-supplied note the UI will render.
const MAX_NOTE_SCALARS: usize = 200;

// ─── Error ────────────────────────────────────────────────

/// Failure from a local admin API call.
///
/// `message` and `key_id` are backend diagnostics: they are kept for debugging
/// and are never rendered. Operator-facing copy comes from
/// [`AdminApiError::user_message`], [`AdminApiError::login_message`] or
/// [`AdminApiError::challenge_message`], which map the stable `code` to fixed
/// strings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminApiError {
    /// HTTP status, or `0` when the request never reached the backend.
    pub status: u16,
    /// Stable backend error code (the local envelope, not the OIDC mapper).
    pub code: String,
    /// Backend diagnostic. Never displayed.
    pub message: String,
    /// Present only for `secret_already_issued`.
    pub key_id: Option<String>,
    /// Backend request correlation id. Never displayed; kept so an operator can
    /// quote it to support when reporting a failure.
    #[serde(default)]
    pub correlation_id: Option<String>,
}

#[derive(Deserialize)]
struct ErrorEnvelope {
    error: ErrorDetail,
    #[serde(default)]
    correlation_id: Option<String>,
}

#[derive(Deserialize)]
struct ErrorDetail {
    code: String,
    message: String,
    #[serde(default)]
    key_id: Option<String>,
}

impl AdminApiError {
    /// Parse the backend envelope
    /// `{"error":{"code":…,"message":…,"key_id"?},"correlation_id":…}`.
    ///
    /// A body that is not that envelope (proxy HTML, truncated JSON, empty)
    /// degrades to a synthetic `unknown_error`; unparsed backend text is never
    /// surfaced to the operator.
    pub fn from_response(status: u16, body: &str) -> Self {
        match serde_json::from_str::<ErrorEnvelope>(body) {
            Ok(envelope) => Self {
                status,
                code: envelope.error.code,
                message: envelope.error.message,
                key_id: envelope.error.key_id,
                correlation_id: envelope.correlation_id,
            },
            Err(_) => Self {
                status,
                code: "unknown_error".to_owned(),
                message: "unparseable error response".to_owned(),
                key_id: None,
                correlation_id: None,
            },
        }
    }

    /// The request could not be sent at all (network, CSP, abort).
    fn transport(detail: &str) -> Self {
        Self {
            status: 0,
            code: "transport_error".to_owned(),
            message: detail.to_owned(),
            key_id: None,
            correlation_id: None,
        }
    }

    #[cfg(test)]
    fn transport_error_for_test() -> Self {
        Self::transport("network failure")
    }

    /// A successful response body was empty where a body was required.
    fn empty_body(status: u16) -> Self {
        Self {
            status,
            code: "empty_response".to_owned(),
            message: "expected a JSON body but the response was empty".to_owned(),
            key_id: None,
            correlation_id: None,
        }
    }

    /// A successful response body was not the expected JSON shape.
    fn malformed_body(status: u16) -> Self {
        Self {
            status,
            code: "malformed_response".to_owned(),
            message: "response body did not match the expected shape".to_owned(),
            key_id: None,
            correlation_id: None,
        }
    }

    /// The response carried a success status the contract does not allow there.
    fn unexpected_status(status: u16) -> Self {
        Self {
            status,
            code: "unexpected_status".to_owned(),
            message: "unexpected success status for this endpoint".to_owned(),
            key_id: None,
            correlation_id: None,
        }
    }

    /// No session CSRF token is held by this client, so a session mutation
    /// cannot be attempted.
    fn missing_session_csrf() -> Self {
        Self {
            status: 0,
            code: "session_missing".to_owned(),
            message: "no session CSRF token is held by this client".to_owned(),
            key_id: None,
            correlation_id: None,
        }
    }

    /// The browser exposed no secure random source.
    fn no_crypto() -> Self {
        Self {
            status: 0,
            code: "no_crypto".to_owned(),
            message: "the browser exposed no cryptography API".to_owned(),
            key_id: None,
            correlation_id: None,
        }
    }

    /// The browser refused or does not provide clipboard access.
    fn clipboard_unavailable() -> Self {
        Self {
            status: 0,
            code: "clipboard_unavailable".to_owned(),
            message: "the clipboard API is unavailable or refused the write".to_owned(),
            key_id: None,
            correlation_id: None,
        }
    }

    /// The session is valid but too old for this operation: the operator must
    /// re-enter the password, and the UI retries only after an explicit click.
    pub fn is_reauth_required(&self) -> bool {
        self.code == "reauth_required"
    }

    /// No usable administrator session is behind this request.
    ///
    /// On the login form the same status means rejected credentials; only the
    /// calling page knows which reading applies.
    pub fn is_unauthenticated(&self) -> bool {
        self.status == 401 || self.code == "session_missing"
    }

    /// Whether a page should ask the operator to sign in again.
    pub fn ends_session(&self) -> bool {
        self.is_unauthenticated() || self.code == "session_expired"
    }

    /// Fixed operator-facing copy for an authenticated admin page.
    ///
    /// Never interpolates `message`, `key_id`, or any other backend string.
    pub fn user_message(&self) -> &'static str {
        match self.code.as_str() {
            "throttled" => "Too many attempts. Wait a few minutes before trying again.",
            "reauth_required" => "Confirm your password to continue.",
            "session_missing" | "session_expired" => "Your session ended. Sign in again.",
            "unauthorized" | "unauthenticated" => "You are signed out. Sign in again to continue.",
            "forbidden" => "That action was refused. Reload the page and try again.",
            "not_found" => "That client or key no longer exists.",
            "conflict" | "idempotency_conflict" => {
                "The client changed since this page was loaded. Reload and try again."
            }
            "secret_already_issued" => {
                "A secret was already issued for this request. Revoke that key and issue a new one."
            }
            "bad_request" => "The request was rejected. Check the values and try again.",
            "temporarily_unavailable" => {
                "The service is temporarily unavailable. Try again shortly."
            }
            "internal_error" => "The service reported an internal error. Try again shortly.",
            "not_implemented" => "This operation is not available in this build.",
            "empty_response" | "malformed_response" | "unexpected_status" => {
                "The service answered in a way this page did not expect. Try again."
            }
            "no_crypto" => {
                "This browser cannot generate a secure operation id. Use a current browser over HTTPS."
            }
            "clipboard_unavailable" => {
                "The secret could not be copied automatically. Select it and copy it manually before closing this panel."
            }
            "transport_error" => {
                "The request could not be sent. Check your connection and try again."
            }
            _ => "The request failed. Try again.",
        }
    }

    /// Copy for the sign-in form. A rejected credential POST and an expired
    /// session share one status code, and only this form is about credentials.
    pub fn login_message(&self) -> &'static str {
        match self.code.as_str() {
            "throttled" => "Too many attempts. Wait a few minutes before trying again.",
            "temporarily_unavailable" | "internal_error" | "transport_error" => self.user_message(),
            _ => "Sign-in failed. Check your username and password.",
        }
    }

    /// Copy for the activation/reset forms: every code failure reads the same
    /// so the form cannot become an oracle for which codes exist.
    pub fn challenge_message(&self) -> &'static str {
        match self.code.as_str() {
            "throttled" => "Too many attempts. Wait a few minutes before trying again.",
            "temporarily_unavailable" | "internal_error" | "transport_error" => self.user_message(),
            _ => {
                "That code could not be used. It may be invalid, expired or already used. Ask an operator to generate a new one with the CLI."
            }
        }
    }
}

impl fmt::Display for AdminApiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // `message` is deliberately excluded: it is backend text and this
        // Display is the only place a caller could accidentally log it.
        write!(formatter, "admin api error {}", self.code)
    }
}

// ─── Wire DTOs ────────────────────────────────────────────

/// `GET /api/v1/auth/config` response: the enabled browser authentication
/// methods (ADR-0057).
///
/// A deployment serves a *set*, so this is a list rather than one value, and the
/// login page renders one form per entry it recognises. An entry it does not
/// recognise is ignored rather than fatal: a newer server may offer a method this
/// build has no form for, and the ones it does know still work.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AuthConfig {
    #[serde(default)]
    pub methods: Vec<String>,
}

impl AuthConfig {
    /// Whether the deployment offers the administrator's password door.
    pub fn has_local(&self) -> bool {
        self.methods.iter().any(|method| method == METHOD_LOCAL)
    }

    /// Whether the deployment offers the identity-provider redirect.
    pub fn has_oidc(&self) -> bool {
        self.methods.iter().any(|method| method == METHOD_OIDC)
    }
}

/// `GET /api/v1/auth/local/csrf` response.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CsrfResponse {
    pub csrf_token: String,
}

/// `POST /api/v1/auth/local/challenge` response: no credential data, and only
/// returned for material that is currently valid.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChallengeResponse {
    pub username: String,
    pub expires_at: String,
}

/// `GET /api/v1/admin/session` response.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct SessionResponse {
    pub admin_id: String,
    pub username: String,
    pub auth_time: String,
    pub absolute_expiry: String,
    pub csrf_token: String,
}

impl fmt::Debug for SessionResponse {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The session CSRF token is a credential: redacted from Debug.
        formatter
            .debug_struct("SessionResponse")
            .field("admin_id", &self.admin_id)
            .field("username", &self.username)
            .field("auth_time", &self.auth_time)
            .field("absolute_expiry", &self.absolute_expiry)
            .field("csrf_token", &"<redacted>")
            .finish()
    }
}

/// A client as returned by every `/api/v1/admin/clients` route.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ClientView {
    pub account_id: String,
    pub tenant_id: String,
    pub display_name: String,
    pub account_status: String,
    pub tenant_status: String,
    pub plan_version: u32,
    pub schema_version: u32,
    /// Compare-and-set version for suspend/resume.
    pub version: u64,
    /// Safe, bounded reason when provisioning did not complete.
    #[serde(default)]
    pub provisioning_reason: Option<String>,
}

/// Tenant statuses that have not reached a terminal state.
const TENANT_PROVISIONING: [&str; 3] = ["reserved", "namespace_creating", "migrating"];

impl ClientView {
    /// The client finished provisioning and can serve data.
    pub fn is_ready(&self) -> bool {
        self.account_status == "active" && self.tenant_status == "ready"
    }

    /// Provisioning is still in flight, so the row is worth polling.
    pub fn is_provisioning(&self) -> bool {
        TENANT_PROVISIONING.contains(&self.tenant_status.as_str())
    }

    /// Provisioning ended in failure.
    pub fn is_failed(&self) -> bool {
        self.tenant_status == "failed"
    }

    /// The coherent suspended pair created by this admin workflow.
    pub fn is_suspended(&self) -> bool {
        self.account_status == "suspended" && self.tenant_status == "suspended"
    }

    /// Only a ready client can be suspended.
    pub fn can_suspend(&self) -> bool {
        self.is_ready()
    }

    /// Only a coherently suspended client can be resumed; an unfinished
    /// tenant is never offered as resumable.
    pub fn can_resume(&self) -> bool {
        self.is_suspended()
    }

    /// Keys can be issued only while the client is ready.
    pub fn can_issue_keys(&self) -> bool {
        self.is_ready()
    }

    /// Human-readable provisioning state.
    pub fn state_label(&self) -> &str {
        self.tenant_status.as_str()
    }

    /// Whether the row should be polled for further changes.
    pub fn stays_visible_when_polling(&self) -> bool {
        self.is_provisioning()
    }

    /// Bounded, control-character-free rendering of the backend reason.
    pub fn safe_provisioning_reason(&self) -> Option<String> {
        self.provisioning_reason.as_deref().map(sanitize_note)
    }
}

/// `POST /api/v1/admin/clients` response body: the created client's view.
///
/// The backend answers `202 Accepted` with the same shape `GET` returns, so
/// this is an alias rather than a second struct that could drift.
pub type CreateClientResponse = ClientView;

/// A cursor-paginated collection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default)]
    pub next_cursor: Option<String>,
}

/// Key metadata as returned by `GET /api/v1/admin/clients/{id}/keys`.
///
/// Never includes the secret.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ApiKeyMeta {
    pub id: String,
    pub name: String,
    pub status: String,
    pub created_at: String,
    #[serde(default)]
    pub expires_at: Option<String>,
    #[serde(default)]
    pub last_used_at: Option<String>,
}

/// Derived key status. Expiry is computed from the timestamp; the backend
/// stores only `active`/`revoked`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyDisplayStatus {
    Active,
    Revoked,
    Expired,
}

impl KeyDisplayStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Revoked => "revoked",
            Self::Expired => "expired",
        }
    }
}

impl ApiKeyMeta {
    /// Derive the status shown to the operator.
    ///
    /// `now_millis` is the browser clock; when it (or the timestamp) cannot be
    /// read, the server-reported status is used unchanged. Expiry is inclusive:
    /// a key is already invalid at its exact expiry instant.
    pub fn display_status(&self, now_millis: Option<i64>) -> KeyDisplayStatus {
        if self.status == "revoked" {
            return KeyDisplayStatus::Revoked;
        }
        match (
            self.expires_at.as_deref().and_then(parse_rfc3339_millis),
            now_millis,
        ) {
            (Some(expires_at), Some(now)) if now >= expires_at => KeyDisplayStatus::Expired,
            _ => KeyDisplayStatus::Active,
        }
    }
}

/// `POST /api/v1/admin/clients/{id}/keys` response: the only occurrence of the
/// key secret.
#[derive(Clone, PartialEq, Eq, Deserialize)]
pub struct CreatedKey {
    pub id: String,
    pub name: String,
    pub secret: String,
    #[serde(default)]
    pub expires_at: Option<String>,
}

impl fmt::Debug for CreatedKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The secret is a credential: redacted from Debug.
        formatter
            .debug_struct("CreatedKey")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("secret", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

/// Explicit expiry choice for a new key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KeyExpiry {
    Never,
    Days { days: u32 },
}

/// Which challenge a pasted code belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChallengeKind {
    Activate,
    Reset,
}

impl ChallengeKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Activate => "activate",
            Self::Reset => "reset",
        }
    }
}

/// Which coherent state change the operator asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientStateAction {
    Suspend,
    Resume,
}

impl ClientStateAction {
    const fn path_segment(self) -> &'static str {
        match self {
            Self::Suspend => "suspend",
            Self::Resume => "resume",
        }
    }
}

// ─── Validation ───────────────────────────────────────────

/// Why an operator-supplied name was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    ControlCharacters,
    TooManyBytes,
    TooManyScalars,
}

impl NameError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "Enter a name.",
            Self::ControlCharacters => "The name contains characters that are not allowed.",
            Self::TooManyBytes => "The name is too long (400 bytes maximum).",
            Self::TooManyScalars => "The name is too long (100 characters maximum).",
        }
    }
}

/// Trim outer whitespace and validate an operator-supplied name.
///
/// Returns the exact value to send: the backend applies the same rule, so the
/// UI never submits something it would itself reject.
pub fn validate_name(raw: &str) -> Result<String, NameError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(NameError::Empty);
    }
    if trimmed.chars().any(char::is_control) {
        return Err(NameError::ControlCharacters);
    }
    if trimmed.len() > MAX_NAME_BYTES {
        return Err(NameError::TooManyBytes);
    }
    if trimmed.chars().count() > MAX_NAME_SCALARS {
        return Err(NameError::TooManyScalars);
    }
    Ok(trimmed.to_owned())
}

/// Why an expiry choice was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExpiryError {
    Blank,
    BothSupplied,
    NotADayCount,
    OutOfRange,
}

impl ExpiryError {
    pub fn message(self) -> &'static str {
        match self {
            Self::Blank => "Choose how long the key should last.",
            Self::BothSupplied => "Choose either a day count or “never”, not both.",
            Self::NotADayCount => "Enter the number of days as a whole number.",
            Self::OutOfRange => "Enter between 1 and 3650 days, or choose “never”.",
        }
    }
}

/// Validate the explicit expiry selection.
///
/// `never` is the "Never expires" choice; `days` is the raw text of the
/// day-count field. There is no silent default: a blank choice is rejected
/// instead of being read as "never", and supplying both is rejected instead of
/// picking one.
pub fn parse_expiry(never: bool, days: &str) -> Result<KeyExpiry, ExpiryError> {
    let days = days.trim();
    if never {
        if days.is_empty() {
            return Ok(KeyExpiry::Never);
        }
        return Err(ExpiryError::BothSupplied);
    }
    if days.is_empty() {
        return Err(ExpiryError::Blank);
    }
    if !days.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ExpiryError::NotADayCount);
    }
    let parsed = days.parse::<u32>().map_err(|_| ExpiryError::NotADayCount)?;
    if !(MIN_EXPIRY_DAYS..=MAX_EXPIRY_DAYS).contains(&parsed) {
        return Err(ExpiryError::OutOfRange);
    }
    Ok(KeyExpiry::Days { days: parsed })
}

/// Whether an idempotency id must be kept for a manual retry.
///
/// A refusal the backend answered is a definite "nothing was created", so the
/// next attempt is a fresh action with a fresh id. Only an unknown outcome —
/// the request never arrived, or an infrastructure failure that may have
/// committed — keeps the id, because reusing it is what makes the retry safe.
pub fn keeps_operation_id(failure: &AdminApiError) -> bool {
    matches!(
        failure.code.as_str(),
        "transport_error" | "temporarily_unavailable" | "internal_error"
    )
}

/// Clamp a requested page size into the documented range.
pub const fn page_limit(limit: u16) -> u16 {
    if limit < MIN_PAGE_LIMIT {
        MIN_PAGE_LIMIT
    } else if limit > MAX_PAGE_LIMIT {
        MAX_PAGE_LIMIT
    } else {
        limit
    }
}

/// Render a backend-supplied note as bounded plain text.
///
/// Control characters become spaces rather than being dropped, so words from
/// wrapped lines stay separated, and the result never injects a line break or
/// a control sequence into the page.
pub fn sanitize_note(raw: &str) -> String {
    raw.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(MAX_NOTE_SCALARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Percent-encode one path segment.
///
/// Identifiers are opaque and server-generated; encoding them (including `/`,
/// `:`, `..`) keeps a crafted id from addressing a different route.
pub fn encode_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.as_bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~');
        if unreserved {
            encoded.push(char::from(*byte));
        } else {
            encoded.push('%');
            encoded.push(char::from_digit(u32::from(*byte) >> 4, 16).unwrap_or('0'));
            encoded.push(char::from_digit(u32::from(*byte) & 0x0f, 16).unwrap_or('0'));
        }
    }
    encoded
}

fn client_path(account_id: &str) -> String {
    format!("{PATH_CLIENTS}/{}", encode_path_segment(account_id))
}

fn keys_path(account_id: &str) -> String {
    format!("{}/keys", client_path(account_id))
}

fn key_path(account_id: &str, key_id: &str) -> String {
    format!("{}/{}", keys_path(account_id), encode_path_segment(key_id))
}

fn state_path(account_id: &str, action: ClientStateAction) -> String {
    format!("{}/{}", client_path(account_id), action.path_segment())
}

// ─── Response handling ────────────────────────────────────

/// Accept a successful response that carries no body.
///
/// Takes only the status: a `204 No Content` has nothing to decode, so this
/// helper cannot accidentally hand an empty body to a JSON parser.
pub fn accept_no_content(status: u16) -> Result<(), AdminApiError> {
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(AdminApiError::from_response(status, ""))
    }
}

/// Require one exact success status.
///
pub fn accept_status(status: u16, expected: u16) -> Result<(), AdminApiError> {
    if status == expected {
        Ok(())
    } else {
        Err(AdminApiError::unexpected_status(status))
    }
}

/// Decode a successful JSON response body.
///
/// Bodyless endpoints use [`accept_no_content`] instead; an empty body reaching
/// this function is a contract violation and is reported as one rather than
/// being decoded again.
pub fn parse_json_body<T: DeserializeOwned>(status: u16, body: &str) -> Result<T, AdminApiError> {
    if body.is_empty() {
        return Err(AdminApiError::empty_body(status));
    }
    serde_json::from_str(body).map_err(|_| AdminApiError::malformed_body(status))
}

// ─── Timestamps ───────────────────────────────────────────

/// Parse the UTC RFC 3339 timestamps the backend emits into milliseconds since
/// the Unix epoch.
///
/// This is used only to derive a display label, so anything not fully
/// understood returns `None` and the caller falls back to the server-reported
/// status instead of guessing. It never participates in an authorization
/// decision.
pub fn parse_rfc3339_millis(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    let separators = (
        bytes[4] == b'-',
        bytes[7] == b'-',
        bytes[10] == b'T' || bytes[10] == b't',
        bytes[13] == b':',
        bytes[16] == b':',
    );
    if separators != (true, true, true, true, true) {
        return None;
    }
    let year = fixed_digits(bytes, 0, 4)?;
    let month = fixed_digits(bytes, 5, 7)?;
    let day = fixed_digits(bytes, 8, 10)?;
    let hour = fixed_digits(bytes, 11, 13)?;
    let minute = fixed_digits(bytes, 14, 16)?;
    let second = fixed_digits(bytes, 17, 19)?;
    if !(1..=12).contains(&month) || day < 1 || day > days_in_month(year, month) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 60 {
        return None;
    }

    let rest = &value[19..];
    let (fraction, offset) = split_fraction_and_offset(rest)?;
    let offset_minutes = offset_minutes(offset)?;

    let days = days_from_civil(year, month, day);
    let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset_minutes * 60;
    Some(seconds * 1_000 + fraction)
}

fn fixed_digits(bytes: &[u8], start: usize, end: usize) -> Option<i64> {
    let slice = bytes.get(start..end)?;
    if !slice.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let mut value: i64 = 0;
    for byte in slice {
        value = value * 10 + i64::from(byte - b'0');
    }
    Some(value)
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap_year(year) => 29,
        2 => 28,
        _ => 0,
    }
}

fn is_leap_year(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

/// Days between 1970-01-01 and the given civil date (Howard Hinnant's algorithm).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn split_fraction_and_offset(rest: &str) -> Option<(i64, &str)> {
    if let Some(fraction) = rest.strip_prefix('.') {
        let offset_at = fraction.find(['Z', 'z', '+', '-'])?;
        let digits = &fraction[..offset_at];
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        let mut millis: i64 = 0;
        for byte in digits.bytes().take(3) {
            millis = millis * 10 + i64::from(byte - b'0');
        }
        for _ in digits.len().min(3)..3 {
            millis *= 10;
        }
        Some((millis, &fraction[offset_at..]))
    } else {
        Some((0, rest))
    }
}

fn offset_minutes(offset: &str) -> Option<i64> {
    if offset.len() == 1 && (offset == "Z" || offset == "z") {
        return Some(0);
    }
    let bytes = offset.as_bytes();
    if bytes.len() != 6 {
        return None;
    }
    let sign = match bytes[0] {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    if bytes[3] != b':' {
        return None;
    }
    let hours = fixed_digits(bytes, 1, 3)?;
    let minutes = fixed_digits(bytes, 4, 6)?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some(sign * (hours * 60 + minutes))
}

// ─── Idempotency ids ──────────────────────────────────────

/// An idempotency operation id: a lowercase RFC 4122 v4 UUID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationId(String);

impl OperationId {
    /// Validate a canonical lowercase v4 UUID.
    pub fn parse(raw: &str) -> Option<Self> {
        let bytes = raw.as_bytes();
        if bytes.len() != 36 {
            return None;
        }
        for (index, byte) in bytes.iter().enumerate() {
            if matches!(index, 8 | 13 | 18 | 23) {
                if *byte != b'-' {
                    return None;
                }
            } else if !byte.is_ascii_digit() && !matches!(byte, b'a'..=b'f') {
                return None;
            }
        }
        if bytes[14] != b'4' || !matches!(bytes[19], b'8' | b'9' | b'a' | b'b') {
            return None;
        }
        Some(Self(raw.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ─── Browser host primitives ──────────────────────────────
//
// The four functions below are the only place this crate calls into the browser
// directly. They used to go through `dioxus::document::eval`, which the web
// renderer implements with `new Function`. The shipped CSP allows WebAssembly
// compilation only, so all four were blocked, and a blocked call is a
// WebAssembly trap rather than a recoverable error: it aborted the scheduler
// tick that raised it, which is why every interactive flow died silently.
//
// Calling the platform APIs directly removes the dynamic code entirely.
// `gloo-timers`, `js-sys`, `wasm-bindgen-futures` and `web-sys` are already
// compiled into this crate's WebAssembly through `dioxus-web` and `gloo-net`,
// so naming them adds no crate to the lock file and no byte to the bundle.
//
// One trap is worth naming because it is invisible at compile time: the standard
// library's clock has no `wasm32-unknown-unknown` implementation, so
// `std::time::SystemTime::now()` compiles and then panics with "time not
// implemented on this platform" the first time it runs. Nothing in this crate
// may use `std::time`, `std::thread`, `std::fs` or `std::net` for the same
// reason; the browser APIs are the only available source.

/// Generate one operation id for a single deliberate user action.
///
/// Call this once per action the operator takes and reuse the value for every
/// manual retry of that action: a fresh id per retry would defeat the backend
/// idempotency record.
///
/// The 16 bytes come from the window's `crypto.getRandomValues`, a
/// secure-context API this deployment always provides (TLS is required, and
/// loopback counts as secure). There is deliberately no `Math.random` fallback.
pub fn fresh_operation_id() -> Result<OperationId, AdminApiError> {
    let window = web_sys::window().ok_or_else(AdminApiError::no_crypto)?;
    let crypto = window.crypto().map_err(|_| AdminApiError::no_crypto())?;
    let mut bytes = [0u8; 16];
    crypto
        .get_random_values_with_u8_array(&mut bytes)
        .map_err(|_| AdminApiError::no_crypto())?;
    OperationId::parse(&format_v4(bytes)).ok_or_else(AdminApiError::no_crypto)
}

/// Render 16 random bytes as a canonical lowercase v4 UUID.
///
/// The version and variant bits are set here rather than trusted from the
/// source, so the result always satisfies [`OperationId::parse`].
fn format_v4(mut bytes: [u8; 16]) -> String {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Yield to the browser event loop for `millis`.
///
/// Infallible by design: `setTimeout` is part of the environment, so a polling
/// loop no longer needs a "no event loop" escape hatch that cannot fire.
pub async fn sleep_ms(millis: u32) {
    gloo_timers::future::TimeoutFuture::new(millis).await;
}

/// Copy a value to the clipboard through the browser, without persisting it.
///
/// The value goes straight to `navigator.clipboard.writeText`: it is never
/// interpolated into executable source, and nothing is written to
/// `localStorage`, `sessionStorage` or `IndexedDB` on the way.
///
/// `navigator.clipboard` exists only in a secure context, which is checked
/// first. Without that check an insecure origin would trap the task instead of
/// reporting an unavailable clipboard.
pub async fn copy_to_clipboard(value: &str) -> Result<(), AdminApiError> {
    let window = web_sys::window().ok_or_else(AdminApiError::clipboard_unavailable)?;
    if !window.is_secure_context() {
        return Err(AdminApiError::clipboard_unavailable());
    }
    let promise = window.navigator().clipboard().write_text(value);
    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(|_| AdminApiError::clipboard_unavailable())
}

/// Current wall clock in milliseconds since the Unix epoch.
///
/// Read from the browser's `Date.now()`. The standard library's clock is not
/// implemented on `wasm32-unknown-unknown` — `SystemTime::now()` panics there —
/// so the browser is the only source of wall clock this crate has.
///
/// `Date.now()` is an integer-valued `f64` milliseconds since the epoch. A
/// non-finite reading, or one outside the representable range, is reported as
/// unknown rather than truncated into a wrong instant, and callers then fall
/// back to the server-reported status.
///
/// This is display only: it labels key expiry and never influences an
/// authorization decision.
pub fn browser_now_millis() -> Option<i64> {
    millis_to_i64(js_sys::Date::now())
}

/// Accept a browser millisecond reading only when it names a real instant.
fn millis_to_i64(millis: f64) -> Option<i64> {
    if !millis.is_finite() || millis < 0.0 || millis > i64::MAX as f64 {
        return None;
    }
    Some(millis as i64)
}

// ─── Session CSRF ─────────────────────────────────────────

/// Session-bound CSRF token from `GET /api/v1/admin/session`.
///
/// Held in component state only: never in `localStorage`, `sessionStorage`,
/// `IndexedDB`, a URL, or a `Debug` rendering.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionCsrf(String);

impl SessionCsrf {
    pub fn new(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionCsrf {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SessionCsrf(<redacted>)")
    }
}

// ─── Client ───────────────────────────────────────────────

/// Same-origin client for the local administrator API.
///
/// The session CSRF token lives in memory only, for as long as this value is
/// alive; components hold it in a signal, so it disappears on unmount and on
/// page reload.
#[derive(Clone)]
pub struct AdminApi {
    session_csrf: Option<SessionCsrf>,
}

impl fmt::Debug for AdminApi {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("AdminApi")
            .field(
                "session_csrf",
                &self.session_csrf.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

impl Default for AdminApi {
    fn default() -> Self {
        Self::new()
    }
}

impl AdminApi {
    /// A client with no session token: usable for the public and pre-auth
    /// routes, and it will refuse session mutations until
    /// [`AdminApi::with_session_csrf`] supplies a token.
    pub const fn new() -> Self {
        Self { session_csrf: None }
    }

    /// Adopt the session CSRF token returned by `GET /api/v1/admin/session`.
    pub fn with_session_csrf(mut self, csrf: SessionCsrf) -> Self {
        self.session_csrf = Some(csrf);
        self
    }

    /// Drop the session token (logout, session expiry), whatever the server
    /// answered.
    pub fn forget_session(&mut self) {
        self.session_csrf = None;
    }

    fn session_csrf(&self) -> Result<&str, AdminApiError> {
        self.session_csrf
            .as_ref()
            .map(SessionCsrf::as_str)
            .ok_or_else(AdminApiError::missing_session_csrf)
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T, AdminApiError> {
        let request = Request::get(&crate::base::url(path))
            .build()
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        self.send_json(request, None).await
    }

    /// Decode a JSON response body, optionally requiring one exact status.
    async fn send_json<T: DeserializeOwned>(
        &self,
        request: Request,
        expected: Option<u16>,
    ) -> Result<T, AdminApiError> {
        let response = request
            .send()
            .await
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        let status = response.status();
        if !response.ok() {
            let body = response.text().await.unwrap_or_default();
            return Err(AdminApiError::from_response(status, &body));
        }
        if let Some(expected) = expected {
            accept_status(status, expected)?;
        }
        let body = response
            .text()
            .await
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        parse_json_body::<T>(status, &body)
    }

    async fn send_without_body(&self, request: Request) -> Result<(), AdminApiError> {
        let response = request
            .send()
            .await
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        let status = response.status();
        if !response.ok() {
            let body = response.text().await.unwrap_or_default();
            return Err(AdminApiError::from_response(status, &body));
        }
        // Success: the body is deliberately not read.
        accept_no_content(status)
    }

    /// `GET /api/v1/auth/config` — which sign-in methods this deployment
    /// serves.
    pub async fn auth_config(&self) -> Result<AuthConfig, AdminApiError> {
        self.get_json(PATH_AUTH_CONFIG).await
    }

    /// `GET /api/v1/auth/local/csrf` — short-lived pre-auth CSRF token.
    ///
    /// The token is returned to the caller and never retained here.
    pub async fn preauth_csrf(&self) -> Result<CsrfResponse, AdminApiError> {
        self.get_json(PATH_PREAUTH_CSRF).await
    }

    /// `POST /api/v1/auth/local/challenge` — validate a code without consuming
    /// it. Never retried automatically.
    pub async fn inspect(
        &self,
        code: &str,
        kind: ChallengeKind,
    ) -> Result<ChallengeResponse, AdminApiError> {
        let csrf = self.preauth_csrf().await?.csrf_token;
        let request = Request::post(&crate::base::url(PATH_CHALLENGE))
            .header(HEADER_CSRF, &csrf)
            .json(&ChallengeBody {
                code,
                kind: kind.as_str(),
            })
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        self.send_json(request, None).await
    }

    /// `POST /api/v1/auth/local/activate` — set the first admin password.
    ///
    /// Never retried automatically.
    pub async fn activate(&self, code: &str, password: &str) -> Result<(), AdminApiError> {
        let csrf = self.preauth_csrf().await?.csrf_token;
        let request = self
            .finish_request(PATH_ACTIVATE, &csrf, code, password)
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        self.send_without_body(request).await
    }

    /// `POST /api/v1/auth/local/reset` — replace an admin password.
    ///
    /// Never retried automatically.
    pub async fn reset(&self, code: &str, password: &str) -> Result<(), AdminApiError> {
        let csrf = self.preauth_csrf().await?.csrf_token;
        let request = self
            .finish_request(PATH_RESET, &csrf, code, password)
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        self.send_without_body(request).await
    }

    fn finish_request(
        &self,
        path: &str,
        csrf: &str,
        code: &str,
        password: &str,
    ) -> Result<Request, gloo_net::Error> {
        Request::post(&crate::base::url(path))
            .header(HEADER_CSRF, csrf)
            .json(&FinishBody { code, password })
    }

    /// `POST /api/v1/auth/local/login` — establish an admin session.
    ///
    /// Never retried automatically: a rejected credential POST must remain one
    /// deliberate attempt and must not multiply the throttle budget.
    pub async fn login(&self, username: &str, password: &str) -> Result<(), AdminApiError> {
        let csrf = self.preauth_csrf().await?.csrf_token;
        let request = Request::post(&crate::base::url(PATH_LOGIN))
            .header(HEADER_CSRF, &csrf)
            .json(&LoginBody { username, password })
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        self.send_without_body(request).await
    }

    /// `GET /api/v1/admin/session` — current session and its CSRF token.
    pub async fn session(&self) -> Result<SessionResponse, AdminApiError> {
        self.get_json(PATH_SESSION).await
    }

    /// `POST /api/v1/admin/reauth` — rotate the session after the operator
    /// re-enters the password. Never retried automatically.
    pub async fn reauth(&self, password: &str) -> Result<(), AdminApiError> {
        let csrf = self.session_csrf()?;
        let request = Request::post(&crate::base::url(PATH_REAUTH))
            .header(HEADER_CSRF, csrf)
            .json(&ReauthBody { password })
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        self.send_without_body(request).await
    }

    /// `POST /api/v1/admin/logout` — revoke the presented session.
    ///
    /// With no token held there is no session to protect, so the request is
    /// still sent unauthenticated: the backend documents that a repeat logout
    /// returns `204` without touching another session.
    pub async fn logout(&self) -> Result<(), AdminApiError> {
        let mut request = Request::post(&crate::base::url(PATH_LOGOUT));
        if let Some(csrf) = self.session_csrf.as_ref() {
            request = request.header(HEADER_CSRF, csrf.as_str());
        }
        let request = request
            .build()
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        self.send_without_body(request).await
    }

    /// `GET /api/v1/admin/clients?after=&limit=` — one page of clients.
    pub async fn clients(
        &self,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Page<ClientView>, AdminApiError> {
        let request = page_request(Request::get(&crate::base::url(PATH_CLIENTS)), after, limit)?;
        self.send_json(request, None).await
    }

    /// `POST /api/v1/admin/clients` — create one client.
    ///
    /// The caller generates `operation_id` once per deliberate create action and
    /// reuses it for any manual retry, so a lost response cannot create a second
    /// client.
    pub async fn create_client(
        &self,
        display_name: &str,
        operation_id: &OperationId,
    ) -> Result<CreateClientResponse, AdminApiError> {
        let csrf = self.session_csrf()?;
        let request = Request::post(&crate::base::url(PATH_CLIENTS))
            .header(HEADER_CSRF, csrf)
            .header(HEADER_IDEMPOTENCY, operation_id.as_str())
            .json(&CreateClientBody { display_name })
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        // The created view arrives as `202 Accepted` with a `Location` header;
        // this client reads the body only.
        self.send_json(request, None).await
    }

    /// `GET /api/v1/admin/clients/{account_id}` — one client's metadata.
    pub async fn client(&self, account_id: &str) -> Result<ClientView, AdminApiError> {
        self.get_json(&client_path(account_id)).await
    }

    /// `GET /api/v1/admin/clients/{account_id}/keys` — one page of key
    /// metadata.
    pub async fn keys(
        &self,
        account_id: &str,
        after: Option<&str>,
        limit: u16,
    ) -> Result<Page<ApiKeyMeta>, AdminApiError> {
        let request = page_request(
            Request::get(&crate::base::url(&keys_path(account_id))),
            after,
            limit,
        )?;
        self.send_json(request, None).await
    }

    /// `POST /api/v1/admin/clients/{account_id}/keys` — issue one key.
    ///
    /// Never retried automatically: a retry is a separate deliberate action that
    /// reuses the same `operation_id`, so the backend answers
    /// `409 secret_already_issued` rather than returning a secret twice.
    pub async fn issue_key(
        &self,
        account_id: &str,
        name: &str,
        expiry: &KeyExpiry,
        operation_id: &OperationId,
    ) -> Result<CreatedKey, AdminApiError> {
        let csrf = self.session_csrf()?;
        let request = Request::post(&crate::base::url(&keys_path(account_id)))
            .header(HEADER_CSRF, csrf)
            .header(HEADER_IDEMPOTENCY, operation_id.as_str())
            .json(&IssueKeyBody { name, expiry })
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        // The secret is taken only from an exact `201 Created`: no other success
        // status may be shown to the operator as a credential.
        self.send_json(request, Some(201)).await
    }

    /// `DELETE /api/v1/admin/clients/{account_id}/keys/{key_id}` — scoped
    /// revoke. Explicit operator action only; never retried automatically.
    pub async fn revoke_key(&self, account_id: &str, key_id: &str) -> Result<(), AdminApiError> {
        let csrf = self.session_csrf()?;
        let request = Request::delete(&key_path(account_id, key_id))
            .header(HEADER_CSRF, csrf)
            .build()
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        self.send_without_body(request).await
    }

    /// `POST /api/v1/admin/clients/{account_id}/suspend|resume` — coherent,
    /// compare-and-set state change. Never retried automatically.
    pub async fn set_state(
        &self,
        account_id: &str,
        expected_version: u64,
        action: ClientStateAction,
    ) -> Result<(), AdminApiError> {
        let csrf = self.session_csrf()?;
        let request = Request::post(&crate::base::url(&state_path(account_id, action)))
            .header(HEADER_CSRF, csrf)
            .json(&SetStateBody { expected_version })
            .map_err(|error| AdminApiError::transport(&error.to_string()))?;
        self.send_without_body(request).await
    }
}

/// Attach the shared cursor/limit contract to a list request.
fn page_request(
    builder: RequestBuilder,
    after: Option<&str>,
    limit: u16,
) -> Result<Request, AdminApiError> {
    let limit = page_limit(limit).to_string();
    let mut params: Vec<(&str, &str)> = vec![("limit", limit.as_str())];
    if let Some(cursor) = after {
        params.push(("after", cursor));
    }
    builder
        .query(params)
        .build()
        .map_err(|error| AdminApiError::transport(&error.to_string()))
}

#[derive(Serialize)]
struct ChallengeBody<'a> {
    code: &'a str,
    kind: &'a str,
}

#[derive(Serialize)]
struct FinishBody<'a> {
    code: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
struct LoginBody<'a> {
    username: &'a str,
    password: &'a str,
}

#[derive(Serialize)]
struct ReauthBody<'a> {
    password: &'a str,
}

#[derive(Serialize)]
struct CreateClientBody<'a> {
    display_name: &'a str,
}

#[derive(Serialize)]
struct IssueKeyBody<'a> {
    name: &'a str,
    expiry: &'a KeyExpiry,
}

#[derive(Serialize)]
struct SetStateBody {
    expected_version: u64,
}

// ─── Tests ────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Operation ids ─────────────────────────────────────

    /// The formatter is the only part of id generation that is portable, so it
    /// is the part worth pinning: the version and variant bits are set here, not
    /// trusted from the source.
    #[test]
    fn formatting_sets_the_v4_version_and_variant_bits() {
        let id = format_v4([0x00; 16]);
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
        assert_eq!(&id[14..15], "4", "version nibble");
        assert!(
            matches!(&id[19..20], "8" | "9" | "a" | "b"),
            "variant nibble must be RFC 4122, got {}",
            &id[19..20]
        );
        assert!(OperationId::parse(&id).is_some());
    }

    #[test]
    fn formatting_is_lowercase_hex_in_canonical_positions() {
        assert_eq!(
            format_v4([
                0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
                0xee, 0xff,
            ]),
            "00112233-4455-4677-8899-aabbccddeeff"
        );
    }

    #[test]
    fn every_formatted_id_passes_the_parser() {
        for seed in 0u8..=255 {
            let id = format_v4([seed; 16]);
            assert!(OperationId::parse(&id).is_some(), "{id} was rejected");
        }
    }

    // ── Browser clock ─────────────────────────────────────

    /// `browser_now_millis` cannot be called from a host test: `Date::now()` is a
    /// `wasm-bindgen` import that has no host implementation. The range check is
    /// the part that can be pinned here, and it is the part that decides whether
    /// an unusable reading reaches `ApiKeyMeta::display_status`.
    #[test]
    fn a_clock_reading_is_accepted_only_when_it_names_a_real_instant() {
        assert_eq!(millis_to_i64(0.0), Some(0));
        assert_eq!(millis_to_i64(1_756_000_000_000.0), Some(1_756_000_000_000));
        // Sub-millisecond precision is truncated, never rounded up into a
        // different instant.
        assert_eq!(millis_to_i64(1_756_000_000_000.75), Some(1_756_000_000_000));

        // Before the epoch, non-finite, and out of range are all "unknown"
        // rather than a silently wrong instant.
        assert_eq!(millis_to_i64(-1.0), None);
        assert_eq!(millis_to_i64(f64::NAN), None);
        assert_eq!(millis_to_i64(f64::INFINITY), None);
        assert_eq!(millis_to_i64(f64::NEG_INFINITY), None);
        assert_eq!(millis_to_i64(f64::MAX), None);
    }

    /// An unknown clock must degrade to the server-reported status, not to a
    /// guess that a key has expired.
    #[test]
    fn an_unknown_clock_leaves_the_server_reported_status_alone() {
        let key: ApiKeyMeta = serde_json::from_str(
            r#"{"id":"key:4","name":"ci","status":"active","created_at":"2026-09-19T09:00:00+00:00","expires_at":"2026-09-19T10:00:00+00:00","last_used_at":null}"#,
        )
        .expect("key meta");

        let now = parse_rfc3339_millis("2026-09-20T10:00:00+00:00").expect("now");
        assert_eq!(key.display_status(Some(now)), KeyDisplayStatus::Expired);
        assert_eq!(key.display_status(None), KeyDisplayStatus::Active);
    }

    // ── Wire format ───────────────────────────────────────

    #[test]
    fn auth_config_deserializes_every_recognised_method() {
        let local: AuthConfig =
            serde_json::from_str(r#"{"methods":["local"]}"#).expect("auth config");
        assert_eq!(local.methods, vec![METHOD_LOCAL.to_owned()]);
        assert!(local.has_local());
        assert!(!local.has_oidc());

        let oidc: AuthConfig =
            serde_json::from_str(r#"{"methods":["oidc"]}"#).expect("auth config");
        assert!(oidc.has_oidc());
        assert!(!oidc.has_local());

        // Both at once is the additive case ADR-0057 exists for, and the two
        // answers are independent: enabling one never changes the other.
        let both: AuthConfig =
            serde_json::from_str(r#"{"methods":["local","oidc"]}"#).expect("auth config");
        assert!(both.has_local());
        assert!(both.has_oidc());
    }

    #[test]
    fn unknown_methods_are_ignored_rather_than_fatal() {
        let other: AuthConfig =
            serde_json::from_str(r#"{"methods":["saml"]}"#).expect("auth config");
        assert!(!other.has_local());
        assert!(!other.has_oidc());

        // A missing list is an empty set, not a deserialization failure: the
        // page has to be able to say "no method I recognise" either way.
        let missing: AuthConfig = serde_json::from_str(r#"{}"#).expect("auth config");
        assert!(!missing.has_local());
        assert!(!missing.has_oidc());
    }

    #[test]
    fn csrf_response_deserializes() {
        let csrf: CsrfResponse =
            serde_json::from_str(r#"{"csrf_token":"preauth.token"}"#).expect("csrf");
        assert_eq!(csrf.csrf_token, "preauth.token");
    }

    #[test]
    fn challenge_response_deserializes() {
        let view: ChallengeResponse = serde_json::from_str(
            r#"{"username":"operator","expires_at":"2026-09-19T10:15:00+00:00"}"#,
        )
        .expect("challenge");
        assert_eq!(view.username, "operator");
        assert_eq!(view.expires_at, "2026-09-19T10:15:00+00:00");
    }

    #[test]
    fn session_response_deserializes_and_redacts_its_token() {
        let session: SessionResponse = serde_json::from_str(
            r#"{"admin_id":"admin-1","username":"operator","auth_time":"2026-09-19T09:00:00+00:00","absolute_expiry":"2026-09-19T17:00:00+00:00","csrf_token":"session.token"}"#,
        )
        .expect("session");
        assert_eq!(session.admin_id, "admin-1");
        assert_eq!(session.username, "operator");
        assert_eq!(session.auth_time, "2026-09-19T09:00:00+00:00");
        assert_eq!(session.absolute_expiry, "2026-09-19T17:00:00+00:00");
        assert_eq!(session.csrf_token, "session.token");
        assert!(format!("{session:?}").contains("<redacted>"));
        assert!(!format!("{session:?}").contains("session.token"));
    }

    #[test]
    fn client_view_deserializes() {
        let client: ClientView = serde_json::from_str(
            r#"{"account_id":"account:1","tenant_id":"tenant:1","display_name":"Acme","account_status":"active","tenant_status":"ready","plan_version":3,"schema_version":7,"version":42,"provisioning_reason":null}"#,
        )
        .expect("client view");
        assert_eq!(client.account_id, "account:1");
        assert_eq!(client.tenant_id, "tenant:1");
        assert_eq!(client.display_name, "Acme");
        assert_eq!(client.account_status, "active");
        assert_eq!(client.tenant_status, "ready");
        assert_eq!(client.plan_version, 3);
        assert_eq!(client.schema_version, 7);
        assert_eq!(client.version, 42);
        assert_eq!(client.provisioning_reason, None);
        assert!(client.is_ready());
        assert!(client.can_suspend());
        assert!(client.can_issue_keys());
        assert!(!client.can_resume());
    }

    #[test]
    fn client_view_without_reason_field_deserializes() {
        let client: ClientView = serde_json::from_str(
            r#"{"account_id":"account:2","tenant_id":"tenant:2","display_name":"Beta","account_status":"active","tenant_status":"migrating","plan_version":1,"schema_version":1,"version":1}"#,
        )
        .expect("client view");
        assert_eq!(client.provisioning_reason, None);
        assert!(client.is_provisioning());
        assert!(client.stays_visible_when_polling());
        assert!(!client.is_ready());
    }

    #[test]
    fn client_view_classifies_failed_and_suspended_states() {
        let failed: ClientView = serde_json::from_str(
            r#"{"account_id":"account:3","tenant_id":"tenant:3","display_name":"Gamma","account_status":"active","tenant_status":"failed","plan_version":1,"schema_version":0,"version":9,"provisioning_reason":"migration failed: step 4\nretry exhausted"}"#,
        )
        .expect("client view");
        assert!(failed.is_failed());
        assert!(!failed.is_provisioning());
        assert_eq!(
            failed.safe_provisioning_reason().as_deref(),
            Some("migration failed: step 4 retry exhausted")
        );

        let suspended: ClientView = serde_json::from_str(
            r#"{"account_id":"account:4","tenant_id":"tenant:4","display_name":"Delta","account_status":"suspended","tenant_status":"suspended","plan_version":1,"schema_version":2,"version":10,"provisioning_reason":null}"#,
        )
        .expect("client view");
        assert!(suspended.is_suspended());
        assert!(suspended.can_resume());
        assert!(!suspended.can_suspend());
    }

    #[test]
    fn create_client_response_is_the_client_view() {
        // `POST /api/v1/admin/clients` answers 202 with the same body shape.
        let created: CreateClientResponse = serde_json::from_str(
            r#"{"account_id":"account:5","tenant_id":"tenant:5","display_name":"Epsilon","account_status":"active","tenant_status":"reserved","plan_version":1,"schema_version":0,"version":0,"provisioning_reason":null}"#,
        )
        .expect("created client");
        assert_eq!(created.display_name, "Epsilon");
        assert!(created.is_provisioning());
    }

    #[test]
    fn page_deserializes_with_and_without_cursor() {
        let page: Page<ClientView> = serde_json::from_str(
            r#"{"items":[{"account_id":"account:1","tenant_id":"tenant:1","display_name":"Acme","account_status":"active","tenant_status":"ready","plan_version":1,"schema_version":1,"version":1,"provisioning_reason":null}],"next_cursor":"account:1"}"#,
        )
        .expect("page");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.next_cursor.as_deref(), Some("account:1"));

        let last: Page<ApiKeyMeta> =
            serde_json::from_str(r#"{"items":[],"next_cursor":null}"#).expect("page");
        assert!(last.items.is_empty());
        assert_eq!(last.next_cursor, None);

        let absent: Page<ApiKeyMeta> = serde_json::from_str(r#"{"items":[]}"#).expect("page");
        assert_eq!(absent.next_cursor, None);
    }

    #[test]
    fn api_key_meta_deserializes() {
        let key: ApiKeyMeta = serde_json::from_str(
            r#"{"id":"key:1","name":"ci","status":"active","created_at":"2026-09-19T09:00:00+00:00","expires_at":"2026-10-19T09:00:00+00:00","last_used_at":null}"#,
        )
        .expect("key meta");
        assert_eq!(key.id, "key:1");
        assert_eq!(key.name, "ci");
        assert_eq!(key.status, "active");
        assert_eq!(key.created_at, "2026-09-19T09:00:00+00:00");
        assert_eq!(key.expires_at.as_deref(), Some("2026-10-19T09:00:00+00:00"));
        assert_eq!(key.last_used_at, None);
    }

    #[test]
    fn key_display_status_derives_expiry() {
        let never: ApiKeyMeta = serde_json::from_str(
            r#"{"id":"key:1","name":"ci","status":"active","created_at":"2026-09-19T09:00:00+00:00","expires_at":null,"last_used_at":null}"#,
        )
        .expect("key");
        assert_eq!(
            never.display_status(Some(i64::MAX)),
            KeyDisplayStatus::Active
        );
        assert_eq!(never.display_status(None), KeyDisplayStatus::Active);

        let expiring: ApiKeyMeta = serde_json::from_str(
            r#"{"id":"key:2","name":"ci","status":"active","created_at":"2026-09-19T09:00:00+00:00","expires_at":"2026-09-19T10:00:00+00:00","last_used_at":null}"#,
        )
        .expect("key");
        let expiry = parse_rfc3339_millis("2026-09-19T10:00:00+00:00").expect("expiry");
        // One millisecond before expiry the key is still active; at the exact
        // instant it is already invalid.
        assert_eq!(
            expiring.display_status(Some(expiry - 1)),
            KeyDisplayStatus::Active
        );
        assert_eq!(
            expiring.display_status(Some(expiry)),
            KeyDisplayStatus::Expired
        );
        assert_eq!(
            expiring.display_status(Some(expiry + 1)),
            KeyDisplayStatus::Expired
        );
    }

    #[test]
    fn revoked_wins_over_expired() {
        let revoked: ApiKeyMeta = serde_json::from_str(
            r#"{"id":"key:3","name":"ci","status":"revoked","created_at":"2026-01-01T00:00:00+00:00","expires_at":"2026-01-02T00:00:00+00:00","last_used_at":null}"#,
        )
        .expect("key");
        assert_eq!(
            revoked.display_status(Some(i64::MAX)),
            KeyDisplayStatus::Revoked
        );
    }

    #[test]
    fn created_key_deserializes_and_redacts_the_secret() {
        let created: CreatedKey = serde_json::from_str(
            r#"{"id":"key:9","name":"ci","secret":"mem_sk_abcdef","expires_at":"2026-10-19T09:00:00+00:00"}"#,
        )
        .expect("created key");
        assert_eq!(created.id, "key:9");
        assert_eq!(created.name, "ci");
        assert_eq!(created.secret, "mem_sk_abcdef");
        assert_eq!(
            created.expires_at.as_deref(),
            Some("2026-10-19T09:00:00+00:00")
        );
        assert!(!format!("{created:?}").contains("mem_sk_abcdef"));
    }

    #[test]
    fn key_expiry_deserializes() {
        let never: KeyExpiry = serde_json::from_str(r#"{"kind":"never"}"#).expect("never");
        assert_eq!(never, KeyExpiry::Never);
        let days: KeyExpiry = serde_json::from_str(r#"{"kind":"days","days":30}"#).expect("days");
        assert_eq!(days, KeyExpiry::Days { days: 30 });
    }

    #[test]
    fn key_expiry_serializes_exactly_as_the_backend_expects() {
        assert_eq!(
            serde_json::to_string(&KeyExpiry::Never).expect("serialize"),
            r#"{"kind":"never"}"#
        );
        assert_eq!(
            serde_json::to_string(&KeyExpiry::Days { days: 30 }).expect("serialize"),
            r#"{"kind":"days","days":30}"#
        );
    }

    // ── Error envelope ────────────────────────────────────

    #[test]
    fn error_envelope_without_key_id_deserializes() {
        let envelope =
            r#"{"error":{"code":"conflict","message":"version conflict"},"correlation_id":"8a1b"}"#;
        let error = AdminApiError::from_response(409, envelope);
        assert_eq!(error.status, 409);
        assert_eq!(error.code, "conflict");
        assert_eq!(error.message, "version conflict");
        assert_eq!(error.key_id, None);
        assert!(!error.is_reauth_required());
        assert!(!error.is_unauthenticated());
    }

    #[test]
    fn error_envelope_with_key_id_deserializes() {
        let envelope = r#"{"error":{"code":"secret_already_issued","message":"secret already issued for key key:7","key_id":"key:7"},"correlation_id":"8a1b"}"#;
        let error = AdminApiError::from_response(409, envelope);
        assert_eq!(error.status, 409);
        assert_eq!(error.code, "secret_already_issued");
        assert_eq!(error.key_id.as_deref(), Some("key:7"));
    }

    #[test]
    fn error_body_that_is_not_the_envelope_degrades_safely() {
        for body in ["", "<html>502 Bad Gateway</html>", "{\"error\":"] {
            let error = AdminApiError::from_response(502, body);
            assert_eq!(error.status, 502);
            assert_eq!(error.code, "unknown_error");
            assert_eq!(error.key_id, None);
            assert!(!error.user_message().contains("html"));
        }
    }

    #[test]
    fn admin_api_error_flat_shape_round_trips() {
        let error = AdminApiError {
            status: 403,
            code: "reauth_required".to_owned(),
            message: "recent authentication required".to_owned(),
            key_id: None,
            correlation_id: Some("8a1b".to_owned()),
        };
        let encoded = serde_json::to_string(&error).expect("serialize");
        let decoded: AdminApiError = serde_json::from_str(&encoded).expect("deserialize");
        assert_eq!(decoded, error);
    }

    #[test]
    fn backend_correlation_id_is_retained_but_never_displayed() {
        let envelope =
            r#"{"error":{"code":"conflict","message":"version conflict"},"correlation_id":"8a1b"}"#;
        let error = AdminApiError::from_response(409, envelope);
        assert_eq!(error.correlation_id.as_deref(), Some("8a1b"));
        assert!(!error.user_message().contains("8a1b"));

        let without =
            AdminApiError::from_response(409, r#"{"error":{"code":"conflict","message":"x"}}"#);
        assert_eq!(without.correlation_id, None);
    }

    #[test]
    fn reauth_and_session_classification() {
        let reauth = AdminApiError::from_response(
            403,
            r#"{"error":{"code":"reauth_required","message":"recent authentication required"}}"#,
        );
        assert!(reauth.is_reauth_required());
        assert!(reauth.ends_session() || !reauth.is_unauthenticated());
        assert_eq!(reauth.user_message(), "Confirm your password to continue.");

        let expired = AdminApiError::from_response(
            401,
            r#"{"error":{"code":"unauthenticated","message":"unauthenticated"}}"#,
        );
        assert!(expired.is_unauthenticated());
        assert!(expired.ends_session());
        assert_eq!(
            expired.user_message(),
            "You are signed out. Sign in again to continue."
        );

        let missing = AdminApiError::missing_session_csrf();
        assert!(missing.ends_session());
        assert_eq!(missing.user_message(), "Your session ended. Sign in again.");
    }

    #[test]
    fn form_messages_never_interpolate_backend_text() {
        let secretish = AdminApiError::from_response(
            400,
            r#"{"error":{"code":"bad_request","message":"kdf: argon2 cannot hash 1234"}}"#,
        );
        for message in [
            secretish.user_message(),
            secretish.login_message(),
            secretish.challenge_message(),
        ] {
            assert!(!message.contains("argon2"));
            assert!(!message.contains("1234"));
        }
        assert!(secretish.challenge_message().contains("CLI"));
    }

    // ── Validation ────────────────────────────────────────

    #[test]
    fn expiry_rejects_blank() {
        assert_eq!(parse_expiry(false, ""), Err(ExpiryError::Blank));
        assert_eq!(parse_expiry(false, "   "), Err(ExpiryError::Blank));
    }

    #[test]
    fn expiry_rejects_both_supplied() {
        assert_eq!(parse_expiry(true, "30"), Err(ExpiryError::BothSupplied));
        assert_eq!(parse_expiry(true, " 30 "), Err(ExpiryError::BothSupplied));
    }

    #[test]
    fn expiry_accepts_never_and_the_documented_bounds() {
        assert_eq!(parse_expiry(true, ""), Ok(KeyExpiry::Never));
        assert_eq!(parse_expiry(true, "  "), Ok(KeyExpiry::Never));
        assert_eq!(parse_expiry(false, "1"), Ok(KeyExpiry::Days { days: 1 }));
        assert_eq!(
            parse_expiry(false, "3650"),
            Ok(KeyExpiry::Days { days: 3650 })
        );
        assert_eq!(
            parse_expiry(false, " 3650 "),
            Ok(KeyExpiry::Days { days: 3650 })
        );
    }

    #[test]
    fn expiry_rejects_out_of_range() {
        assert_eq!(parse_expiry(false, "0"), Err(ExpiryError::OutOfRange));
        assert_eq!(parse_expiry(false, "3651"), Err(ExpiryError::OutOfRange));
        assert_eq!(parse_expiry(false, "-1"), Err(ExpiryError::NotADayCount));
        assert_eq!(
            parse_expiry(false, "99999999999"),
            Err(ExpiryError::NotADayCount)
        );
    }

    #[test]
    fn expiry_rejects_non_day_counts() {
        assert_eq!(
            parse_expiry(false, "thirty"),
            Err(ExpiryError::NotADayCount)
        );
        assert_eq!(parse_expiry(false, "3.5"), Err(ExpiryError::NotADayCount));
        assert_eq!(parse_expiry(false, "1e2"), Err(ExpiryError::NotADayCount));
        assert_eq!(parse_expiry(false, "+30"), Err(ExpiryError::NotADayCount));
    }

    #[test]
    fn name_trims_outer_whitespace() {
        assert_eq!(validate_name("  Acme  "), Ok("Acme".to_owned()));
        assert_eq!(validate_name("\tAcme\n"), Ok("Acme".to_owned()));
        // The limit applies to the trimmed value.
        let padded = format!("  {}  ", "a".repeat(MAX_NAME_SCALARS));
        assert_eq!(validate_name(&padded), Ok("a".repeat(MAX_NAME_SCALARS)));
    }

    #[test]
    fn name_rejects_empty() {
        assert_eq!(validate_name(""), Err(NameError::Empty));
        assert_eq!(validate_name("    "), Err(NameError::Empty));
        assert_eq!(validate_name("\u{200b}"), Ok("\u{200b}".to_owned()));
    }

    #[test]
    fn name_rejects_control_characters() {
        assert_eq!(
            validate_name("Acme\u{7}"),
            Err(NameError::ControlCharacters)
        );
        assert_eq!(
            validate_name("Ac\u{0}me"),
            Err(NameError::ControlCharacters)
        );
        assert_eq!(
            validate_name("Acme\u{9}Inc"),
            Err(NameError::ControlCharacters)
        );
    }

    #[test]
    fn name_rejects_over_100_scalars() {
        assert_eq!(
            validate_name(&"a".repeat(101)),
            Err(NameError::TooManyScalars)
        );
        assert!(validate_name(&"a".repeat(100)).is_ok());
    }

    #[test]
    fn name_rejects_over_400_bytes() {
        // Every scalar an operator types is at most four bytes, so breaching the
        // byte budget means the name is far past 100 characters too; the byte
        // limit is reported first because that is the backend's own message.
        let wide = "🦀".repeat(101);
        assert_eq!(wide.len(), 404);
        assert_eq!(validate_name(&wide), Err(NameError::TooManyBytes));
        // 100 three-byte scalars stay inside both budgets.
        assert!(validate_name(&"€".repeat(100)).is_ok());
        assert_eq!(
            validate_name(&"€".repeat(101)),
            Err(NameError::TooManyScalars)
        );
        assert_eq!(
            validate_name(&"€".repeat(134)),
            Err(NameError::TooManyBytes)
        );
    }

    // ── Response handling ─────────────────────────────────

    #[test]
    fn no_content_is_accepted_without_json_decoding() {
        // A 204 has no body at all, so a JSON decoder cannot succeed on it...
        assert!(parse_json_body::<SessionResponse>(204, "").is_err());
        assert_eq!(
            parse_json_body::<SessionResponse>(204, ""),
            Err(AdminApiError::empty_body(204))
        );
        // ...which is why bodyless endpoints accept the status instead.
        assert_eq!(accept_no_content(204), Ok(()));
        assert_eq!(accept_no_content(200), Ok(()));
        assert_eq!(accept_no_content(201), Ok(()));
        assert!(accept_no_content(400).is_err());
        assert_eq!(accept_no_content(404).unwrap_err().status, 404);
    }

    #[test]
    fn the_key_secret_requires_an_exact_created_status() {
        assert_eq!(accept_status(201, 201), Ok(()));
        assert!(accept_status(200, 201).is_err());
        assert!(accept_status(204, 201).is_err());
        assert!(accept_status(202, 201).is_err());
        assert_eq!(
            accept_status(200, 201).unwrap_err().code,
            "unexpected_status"
        );
        assert_eq!(
            accept_status(200, 201).unwrap_err().user_message(),
            "The service answered in a way this page did not expect. Try again."
        );
    }

    #[test]
    fn json_bodies_are_decoded_from_the_status_and_text() {
        let page: Page<ApiKeyMeta> =
            parse_json_body(200, r#"{"items":[],"next_cursor":null}"#).expect("page");
        assert!(page.items.is_empty());
        assert_eq!(
            parse_json_body::<Page<ApiKeyMeta>>(200, "<html/>"),
            Err(AdminApiError::malformed_body(200))
        );
    }

    #[test]
    fn page_limit_is_clamped() {
        assert_eq!(page_limit(0), MIN_PAGE_LIMIT);
        assert_eq!(page_limit(1), 1);
        assert_eq!(page_limit(50), 50);
        assert_eq!(page_limit(100), MAX_PAGE_LIMIT);
        assert_eq!(page_limit(1000), MAX_PAGE_LIMIT);
    }

    #[test]
    fn path_segments_are_encoded() {
        assert_eq!(encode_path_segment("account:1"), "account%3a1");
        assert_eq!(encode_path_segment("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode_path_segment("../../x"), "..%2f..%2fx");
        assert_eq!(
            client_path("account:1"),
            "/api/v1/admin/clients/account%3a1"
        );
        assert_eq!(
            keys_path("account:1"),
            "/api/v1/admin/clients/account%3a1/keys"
        );
        assert_eq!(
            key_path("account:1", "key/2"),
            "/api/v1/admin/clients/account%3a1/keys/key%2f2"
        );
        assert_eq!(
            state_path("account:1", ClientStateAction::Suspend),
            "/api/v1/admin/clients/account%3a1/suspend"
        );
        assert_eq!(
            state_path("account:1", ClientStateAction::Resume),
            "/api/v1/admin/clients/account%3a1/resume"
        );
    }

    #[test]
    fn sanitize_note_strips_control_characters_and_bounds_length() {
        assert_eq!(sanitize_note("failed\nstep 4"), "failed step 4");
        assert_eq!(sanitize_note("  spaced  "), "spaced");
        assert_eq!(sanitize_note("a\u{7}b"), "a b");
        assert_eq!(sanitize_note("a\u{0}b"), "a b");
        assert_eq!(
            sanitize_note(&"x".repeat(500)).chars().count(),
            MAX_NOTE_SCALARS
        );
    }

    #[test]
    fn idempotency_ids_survive_only_unknown_outcomes() {
        // Unknown outcome: the retry must reuse the operation id.
        assert!(keeps_operation_id(
            &AdminApiError::transport_error_for_test()
        ));
        assert!(keeps_operation_id(&AdminApiError::from_response(
            503,
            r#"{"error":{"code":"temporarily_unavailable","message":"temporarily unavailable"}}"#
        )));
        assert!(keeps_operation_id(&AdminApiError::from_response(
            500,
            r#"{"error":{"code":"internal_error","message":"internal error"}}"#
        )));

        // Definite refusals: the next attempt is a fresh action.
        assert!(!keeps_operation_id(&AdminApiError::from_response(
            400,
            r#"{"error":{"code":"bad_request","message":"malformed JSON"}}"#
        )));
        assert!(!keeps_operation_id(&AdminApiError::from_response(
            409,
            r#"{"error":{"code":"conflict","message":"idempotency conflict"}}"#
        )));
        assert!(!keeps_operation_id(&AdminApiError::from_response(
            403,
            r#"{"error":{"code":"reauth_required","message":"recent authentication required"}}"#
        )));
        assert!(!keeps_operation_id(&AdminApiError::missing_session_csrf()));
    }

    // ── Operation ids ─────────────────────────────────────

    #[test]
    fn operation_id_accepts_v4_and_rejects_other_shapes() {
        let valid = "9f1b7c2e-4d3a-4b6f-9a1c-8e2d5f7a0b3c";
        assert_eq!(
            OperationId::parse(valid).map(|id| id.as_str().to_owned()),
            Some(valid.to_owned())
        );
        // Wrong version, wrong variant, uppercase, wrong length, non-hex.
        assert!(OperationId::parse("9f1b7c2e-4d3a-3b6f-9a1c-8e2d5f7a0b3c").is_none());
        assert!(OperationId::parse("9f1b7c2e-4d3a-4b6f-1a1c-8e2d5f7a0b3c").is_none());
        assert!(OperationId::parse("9F1B7C2E-4D3A-4B6F-9A1C-8E2D5F7A0B3C").is_none());
        assert!(OperationId::parse("9f1b7c2e4d3a4b6f9a1c8e2d5f7a0b3c").is_none());
        assert!(OperationId::parse("zf1b7c2e-4d3a-4b6f-9a1c-8e2d5f7a0b3c").is_none());
        assert!(OperationId::parse("").is_none());
    }

    // ── Timestamps ────────────────────────────────────────

    #[test]
    fn rfc3339_parser_accepts_the_backend_format() {
        assert_eq!(parse_rfc3339_millis("1970-01-01T00:00:00+00:00"), Some(0));
        assert_eq!(parse_rfc3339_millis("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_millis("2026-09-19T09:00:00+00:00"),
            Some(1_789_808_400_000)
        );
        assert_eq!(
            parse_rfc3339_millis("2026-09-19T10:00:00.250+00:00"),
            Some(1_789_812_000_250)
        );
        // Offsets are honoured.
        assert_eq!(
            parse_rfc3339_millis("2026-09-19T11:00:00+02:00"),
            Some(1_789_808_400_000)
        );
        // Leap day in a leap year.
        assert!(parse_rfc3339_millis("2028-02-29T00:00:00Z").is_some());
    }

    #[test]
    fn rfc3339_parser_rejects_malformed_input() {
        for value in [
            "",
            "not a date",
            "2026-09-19",
            "2026-09-19T09:00:00",
            "2026-13-01T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2026-09-19T25:00:00Z",
            "2026-09-19T09:61:00Z",
            "2026-09-19T09:00:00+25:00",
            "2026-09-19T09:00:00.Z",
        ] {
            assert_eq!(parse_rfc3339_millis(value), None, "value: {value}");
        }
        assert!(parse_rfc3339_millis("2027-02-29T00:00:00Z").is_none());
    }

    // ── Client surface ────────────────────────────────────

    #[test]
    fn client_without_a_session_refuses_session_mutations() {
        let api = AdminApi::new();
        assert_eq!(
            api.session_csrf().unwrap_err(),
            AdminApiError::missing_session_csrf()
        );

        let api = AdminApi::new().with_session_csrf(SessionCsrf::new("session.token"));
        assert_eq!(api.session_csrf().expect("token"), "session.token");
        assert!(!format!("{api:?}").contains("session.token"));
    }

    #[test]
    fn session_csrf_is_redacted_and_forgettable() {
        let csrf = SessionCsrf::new("session.token");
        assert_eq!(format!("{csrf:?}"), "SessionCsrf(<redacted>)");
        let mut api = AdminApi::new().with_session_csrf(csrf);
        assert_eq!(api.session_csrf().expect("token"), "session.token");
        api.forget_session();
        assert_eq!(
            api.session_csrf().unwrap_err(),
            AdminApiError::missing_session_csrf()
        );
    }
}
