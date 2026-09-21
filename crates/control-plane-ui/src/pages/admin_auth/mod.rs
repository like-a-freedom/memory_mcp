//! Local administrator authentication routes.
//!
//! `/admin/login` renders the shared sign-in form; `/admin/activate` and
//! `/admin/reset` render the two-phase challenge flow in `challenge.rs`; and
//! `/admin/reauth` renders the standalone password confirmation in
//! `reauth_page.rs`.
//!
//! Credential material — one-time codes, passwords and the session CSRF token —
//! is held in component signals only. It is never written to `localStorage`,
//! `sessionStorage` or `IndexedDB`, never placed in a URL, query or fragment,
//! and is dropped when the component unmounts, because nothing here outlives the
//! scope that owns the signal.
//!
//! Failures are reported with the fixed copy from `AdminApiError`: backend
//! `message` text is never rendered, so the forms cannot leak internals. The
//! sign-in form and the re-authentication dialog live in `components/`, because
//! each is used by a route outside this module as well.

use dioxus::prelude::*;

use crate::admin_api::ChallengeKind;
use crate::components::admin_auth::AdminLoginForm;

mod challenge;
mod reauth_page;

use challenge::ChallengeFinishFlow;

pub use reauth_page::AdminReauthPage;

/// `/admin/login` — administrator sign-in for local mode.
#[component]
pub fn AdminLoginPage() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Administrator sign-in" }
            AdminLoginForm {}
        }
    }
}

/// `/admin/activate` — set the first administrator password.
#[component]
pub fn AdminActivationPage() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Set administrator password" }
            ChallengeFinishFlow { kind: ChallengeKind::Activate }
        }
    }
}

/// `/admin/reset` — replace an administrator password.
#[component]
pub fn AdminResetPage() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Reset administrator password" }
            ChallengeFinishFlow { kind: ChallengeKind::Reset }
        }
    }
}
