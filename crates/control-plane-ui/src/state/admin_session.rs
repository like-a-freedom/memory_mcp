//! Administrator session state for the local-admin surface.
//!
//! The session CSRF token lives inside this state, which lives in a signal
//! owned by the console layout. It is never written to `localStorage`,
//! `sessionStorage`, `IndexedDB`, a URL, or a `Debug` rendering, and it
//! disappears with the component that owns it.

use dioxus::prelude::*;
use dioxus_router::Navigator;

use crate::admin_api::{AdminApi, SessionCsrf, SessionResponse};
use crate::routes::Route;

/// What a page says when a read or a poll proved the session is gone.
pub const SESSION_ENDED: &str = "Your session ended. Sign in again.";

/// What a page says when a mutation could not even be attempted for the same
/// reason. It names what the missing session blocked, because the operator's
/// next step depends on whether anything was sent.
pub const SESSION_ENDED_BEFORE_MUTATION: &str = "Your session ended. Sign in again to continue.";

/// What an authenticated page knows about the current administrator session.
#[derive(Clone, PartialEq, Default)]
pub struct AdminSession {
    csrf: Option<SessionCsrf>,
    username: Option<String>,
    absolute_expiry: Option<String>,
    loading: bool,
    ended: bool,
    sign_out_unconfirmed: bool,
    error: Option<String>,
}

impl AdminSession {
    /// Whether session mutations can be attempted.
    pub fn is_ready(&self) -> bool {
        self.csrf.is_some()
    }

    /// The signed-in administrator, when the backend reported one.
    pub fn username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    /// Absolute session deadline, when known.
    pub fn absolute_expiry(&self) -> Option<&str> {
        self.absolute_expiry.as_deref()
    }

    /// Still waiting for `GET /api/v1/admin/session`.
    pub fn is_loading(&self) -> bool {
        self.loading
    }

    /// The session is gone and the operator must sign in again.
    pub fn has_ended(&self) -> bool {
        self.ended
    }

    /// Local session data was dropped, but the server never confirmed the
    /// revocation, so the cookie on this browser may still be live.
    pub fn sign_out_unconfirmed(&self) -> bool {
        self.sign_out_unconfirmed
    }

    /// Non-credential failure copy for the page to display.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// One-line description of the session for the page header.
    ///
    /// Every branch states something the console actually knows: it never
    /// claims to be signed in as an unknown administrator, which reads as a
    /// successful sign-in while the session is in fact unusable.
    pub fn summary(&self) -> String {
        if self.loading {
            "Checking your session…".to_owned()
        } else if self.ended {
            "Signed out".to_owned()
        } else if self.csrf.is_none() {
            // Covers the unconfirmed sign-out too: whatever the server may still
            // hold, this console has no usable session, and the retry lives with
            // the explanation rather than being duplicated here.
            "No administrator session".to_owned()
        } else {
            match self.username() {
                Some(username) => format!("Signed in as {username}"),
                None => "Signed in".to_owned(),
            }
        }
    }

    /// Whether the bar should offer to sign out.
    ///
    /// False for both dead ends: an ended session has nothing left to revoke,
    /// and an unconfirmed one offers its retry beside the explanation the layout
    /// renders, where two controls for one action cannot disagree.
    pub fn can_sign_out(&self) -> bool {
        !self.ended && !self.sign_out_unconfirmed
    }

    /// The session CSRF token, cloned out of component state for a dialog prop.
    pub fn csrf(&self) -> Option<SessionCsrf> {
        self.csrf.clone()
    }

    /// A client that can perform session mutations.
    pub fn mutation_client(&self) -> Option<AdminApi> {
        self.csrf
            .clone()
            .map(|csrf| AdminApi::new().with_session_csrf(csrf))
    }

    /// Adopt a freshly read session, for example after reauthentication
    /// rotated the CSRF token.
    pub fn adopt(&mut self, response: SessionResponse) {
        self.csrf = Some(SessionCsrf::new(response.csrf_token));
        self.username = Some(response.username);
        self.absolute_expiry = Some(response.absolute_expiry);
        self.loading = false;
        self.ended = false;
        self.sign_out_unconfirmed = false;
        self.error = None;
    }

    /// Record that the session can no longer be used.
    pub fn mark_ended(&mut self, message: String) {
        *self = Self {
            loading: false,
            ended: true,
            error: Some(message),
            ..Self::default()
        };
    }

    /// Record that the local session was dropped without the server confirming
    /// the revocation.
    ///
    /// The token is dropped — it was already sent to the server and must not be
    /// reused — but the session is *not* marked ended: a transport failure
    /// proves nothing about the cookie, and claiming the session ended when it
    /// may still be live would both mislead the operator and remove the only
    /// control that can retry the revocation.
    pub fn mark_sign_out_unconfirmed(&mut self, message: String) {
        self.csrf = None;
        self.loading = false;
        self.sign_out_unconfirmed = true;
        self.error = Some(message);
    }
}

/// Publish the console's one administrator session to the routes below.
///
/// Paired with [`use_console_session`] on purpose: the provider and the reader
/// are two halves of one contract, and keeping them in the same module is what
/// stops one from being changed without the other.
pub fn provide_admin_session() -> Signal<AdminSession> {
    let session = use_admin_session();
    provide_context(session);
    session
}

/// Load `GET /api/v1/admin/session` once for the calling component.
///
/// The request is a client-side derived value, so `use_resource` owns its
/// lifecycle and cancels it with the component. The writable session signal is
/// retained because reauthentication and sign-out are explicit page actions,
/// not derived values.
///
/// Called by the console layout, not by individual pages: see
/// [`use_console_session`].
pub fn use_admin_session() -> Signal<AdminSession> {
    let mut session = use_signal(AdminSession::default);
    let resource = use_resource(|| async { AdminApi::new().session().await });

    use_effect(move || {
        let Some(outcome) = resource.read().clone() else {
            session.write().loading = true;
            return;
        };

        match outcome {
            Ok(response) => session.write().adopt(response),
            Err(failure) => {
                let message = failure.user_message().to_owned();
                if failure.ends_session() {
                    session.write().mark_ended(message);
                } else {
                    let mut current = session.write();
                    current.loading = false;
                    current.error = Some(message);
                }
            }
        }
    });
    session
}

/// The administrator session owned by the console layout.
///
/// Reading it from context rather than loading it per page means both console
/// routes share one session: drilling from the client list into one client does
/// not re-read it, and a page cannot display a different session from the
/// notice the layout renders directly above it.
///
/// # Panics
///
/// Panics when called outside the console layout. That can only be a routing
/// mistake — the layout is the only provider — and it is better to fail loudly
/// at mount than to render a page whose session is silently unknown.
pub fn use_console_session() -> Signal<AdminSession> {
    use_context::<Signal<AdminSession>>()
}

/// Revoke the session, drop every in-memory secret the page holds, and return to
/// the sign-in page.
///
/// Local state is cleared before the request: the cookie is invalidated
/// server-side by that request either way, and no secret may stay on screen
/// while it is in flight. The outcome decides what the operator is told, and
/// only a response that proves the session is unusable navigates away.
pub fn end_session(mut session: Signal<AdminSession>, navigator: Navigator) {
    let client = session.read().mutation_client();
    // Clear local state first: the cookie is invalidated server-side by the
    // request below either way, and nothing must stay on screen while it is in
    // flight.
    session.set(AdminSession::default());
    spawn(async move {
        let mut api = client.unwrap_or_default();
        match api.logout().await {
            Ok(()) => {
                navigator.replace(Route::Login {});
            }
            Err(failure) if failure.is_unauthenticated() => {
                // There is no session left to revoke: the backend already
                // rejected the cookie, so signing in again is the whole fix.
                navigator.replace(Route::Login {});
            }
            Err(_) => {
                // A transport failure or an unexpected status says nothing about
                // whether the cookie is still live. Keep the operator on the
                // page with a retry rather than claiming a sign-out that may not
                // have happened.
                session.write().mark_sign_out_unconfirmed(
                    "This console cleared its local session data, but the server did not confirm sign-out. Try again, or close this tab."
                        .to_owned(),
                );
            }
        }
        // The token is dropped whatever the server answered.
        api.forget_session();
    });
}

#[cfg(test)]
mod tests {
    use super::{AdminSession, SessionCsrf, SessionResponse};

    fn adopted() -> AdminSession {
        let mut session = AdminSession::default();
        session.adopt(SessionResponse {
            admin_id: "adm_test".to_owned(),
            username: "operator".to_owned(),
            auth_time: "2026-01-01T00:00:00Z".to_owned(),
            absolute_expiry: "2026-01-02T00:00:00Z".to_owned(),
            csrf_token: "token".to_owned(),
        });
        session
    }

    #[test]
    fn summary_never_claims_a_signed_in_unknown_administrator() {
        // The banner used to read "Signed in as an unknown administrator"
        // whenever the session was not usable, which reports success for a
        // state an operator must act on.
        let loading = AdminSession {
            loading: true,
            ..AdminSession::default()
        };
        assert_eq!(loading.summary(), "Checking your session…");

        assert_eq!(
            AdminSession::default().summary(),
            "No administrator session"
        );

        let ended = AdminSession {
            ended: true,
            ..AdminSession::default()
        };
        assert_eq!(ended.summary(), "Signed out");

        // A usable session with no reported name still must not claim a name.
        let unnamed = AdminSession {
            csrf: Some(SessionCsrf::new("token")),
            ..AdminSession::default()
        };
        assert_eq!(unnamed.summary(), "Signed in");

        assert_eq!(adopted().summary(), "Signed in as operator");
    }

    #[test]
    fn a_usability_ready_session_is_the_one_with_a_csrf_token() {
        assert!(!AdminSession::default().is_ready());
        assert!(adopted().is_ready());
        assert!(adopted().mutation_client().is_some());
    }

    #[test]
    fn an_unconfirmed_sign_out_is_not_reported_as_an_ended_session() {
        // A transport failure does not prove the cookie was revoked. Reporting
        // "Signed out" would be a claim the console cannot make, and it would
        // hide the control that retries the revocation.
        let mut session = adopted();
        session.mark_sign_out_unconfirmed("did not confirm".to_owned());

        assert!(!session.has_ended());
        assert!(session.sign_out_unconfirmed());
        assert_eq!(session.summary(), "No administrator session");
        // The token was sent, so it must not be reusable.
        assert!(!session.is_ready());
        assert!(session.csrf().is_none());
        // The bar must not offer a second control for the same action: the
        // retry belongs beside the explanation.
        assert!(!session.can_sign_out());
    }

    #[test]
    fn the_sign_out_control_is_offered_only_while_it_can_do_something() {
        assert!(adopted().can_sign_out());
        assert!(AdminSession::default().can_sign_out());
        assert!(
            !AdminSession {
                ended: true,
                ..AdminSession::default()
            }
            .can_sign_out()
        );
    }

    #[test]
    fn adopting_a_session_clears_a_previous_unconfirmed_sign_out() {
        let mut session = adopted();
        session.mark_sign_out_unconfirmed("did not confirm".to_owned());
        session.adopt(SessionResponse {
            admin_id: "adm_test".to_owned(),
            username: "operator".to_owned(),
            auth_time: "2026-01-01T00:00:00Z".to_owned(),
            absolute_expiry: "2026-01-02T00:00:00Z".to_owned(),
            csrf_token: "rotated".to_owned(),
        });

        assert!(!session.sign_out_unconfirmed());
        assert_eq!(session.summary(), "Signed in as operator");
    }
}
