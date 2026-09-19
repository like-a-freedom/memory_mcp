//! Administrator session state shared by the local admin pages.
//!
//! The session CSRF token lives inside this state, which lives in a signal
//! owned by whichever page mounted it. It is never written to `localStorage`,
//! `sessionStorage`, `IndexedDB`, a URL, or a `Debug` rendering, and it
//! disappears with the component.

use dioxus::prelude::*;
use dioxus_router::Navigator;
use dioxus_router::hooks::use_navigator;

use crate::admin_api::{AdminApi, SessionCsrf, SessionResponse};
use crate::router::Route;

/// What an authenticated page knows about the current administrator session.
#[derive(Clone, PartialEq, Default)]
pub struct AdminSession {
    csrf: Option<SessionCsrf>,
    username: Option<String>,
    absolute_expiry: Option<String>,
    loading: bool,
    ended: bool,
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

    /// Non-credential failure copy for the page to display.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// One-line description of the session for the page header.
    pub fn summary(&self) -> String {
        if self.loading {
            "Checking your session…".to_owned()
        } else if self.ended {
            "Signed out".to_owned()
        } else {
            format!(
                "Signed in as {}",
                self.username().unwrap_or("an unknown administrator")
            )
        }
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
}

fn load_session(mut session: Signal<AdminSession>) {
    session.set(AdminSession {
        loading: true,
        ..AdminSession::default()
    });
    spawn(async move {
        match AdminApi::new().session().await {
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
}

/// Load `GET /api/v1/admin/session` once for the calling component.
pub fn use_admin_session() -> Signal<AdminSession> {
    let session = use_signal(AdminSession::default);
    use_effect(move || load_session(session));
    session
}

/// Revoke the session, drop every in-memory secret this page holds, and return
/// to the sign-in page.
pub fn end_session(mut session: Signal<AdminSession>, navigator: Navigator) {
    let csrf = session.read().csrf();
    // Clear local state first: the cookie is invalidated server-side by the
    // request below either way, and nothing must stay on screen while it is in
    // flight.
    session.set(AdminSession::default());
    spawn(async move {
        let mut api = AdminApi::new();
        if let Some(csrf) = csrf {
            api = api.with_session_csrf(csrf);
        }
        let _ = api.logout().await;
        // The token is dropped whatever the server answered.
        api.forget_session();
        navigator.replace(Route::Login {});
    });
}

/// A small banner describing the signed-in administrator, with a sign-out
/// action. Renders nothing generic on failure: the caller shows errors.
#[component]
pub fn AdminSessionBar(session: Signal<AdminSession>) -> Element {
    let navigator = use_navigator();
    let summary = session.read().summary();
    let expiry = session.read().absolute_expiry().map(ToOwned::to_owned);

    rsx! {
        div { class: "session-bar",
            span { "{summary}" }
            if let Some(expiry) = expiry {
                span { class: "session-expiry", " Session ends {expiry}" }
            }
            button {
                r#type: "button",
                onclick: move |_| end_session(session, navigator),
                "Sign out"
            }
        }
    }
}
