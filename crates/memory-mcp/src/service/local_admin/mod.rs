pub(crate) mod auth;
pub(crate) mod client;
pub(crate) mod contracts;
#[cfg(feature = "control-plane")]
pub(crate) mod password;
pub(crate) mod policy;
#[cfg(all(test, feature = "control-plane"))]
mod security_tests;
#[cfg(all(test, feature = "control-plane"))]
mod tests;

// ─── Public surface ───────────────────────────────────────
//
// The local administrator surface is exercised by the
// `tests/http_local_admin.rs` integration target, which drives the real
// router over a durable in-memory Registry. That target cannot see
// `pub(crate)` items, so the service types it needs are re-exported
// here. The re-export is deliberately narrow: the submodules stay
// crate-private, and only the composition entry points and the
// contract types a client of the service legitimately needs are
// exposed.

pub use auth::{AdminManagementService, LocalAdminAuthority, LocalAdminService};
pub use client::LocalClientService;
pub use contracts::{
    AdminFence, AdminLogin, AdminPrincipal, AttemptDecision, AttemptDomain, AttemptInput,
    AuthAttemptContext, BrowserPolicyFence, ChallengeFinish, ChallengeIssue, ChallengeKind,
    ChallengeView, ClientBundle, ClientCreate, ClientStateAction, ClientView, CredentialSnapshot,
    FailureAction, FailureAudit, FailureReason, IssuedChallenge, KeyExpiry, KeyInsertOutcome,
    LocalAdminError, LocalAdminStore, LocalKeyFingerprints, LocalResult, OneTimeChallenge, Page,
    PageRequest, RequestContext, SessionOpen, SessionRotate,
};
#[cfg(feature = "control-plane")]
pub use password::PasswordHasher;
pub use policy::{normalize_username, validate_password};
