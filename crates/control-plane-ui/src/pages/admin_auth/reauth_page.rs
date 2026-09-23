//! `/admin/reauth` — the standalone password confirmation.
//!
//! The dialog is normally rendered in place by the page that hit
//! `reauth_required`; this route exists so that a reload in the middle of that
//! flow still lands on a working form. It is outside the console layout, so it
//! loads its own session, and it has no account to return to: both leaving the
//! flow and completing it go to the client list.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::components::admin_auth::AdminReauthDialog;
use crate::components::alert::{Alert, AlertTone};
use crate::inert;
use crate::routes::Route;
use crate::state::admin_session::use_admin_session;

#[component]
pub fn AdminReauthPage() -> Element {
    let navigator = use_navigator();
    let session = use_admin_session();
    let (is_loading, is_ready, csrf, session_error) = {
        let state = session.read();
        (
            state.is_loading(),
            state.is_ready(),
            state.csrf(),
            state.error().map(str::to_owned),
        )
    };
    let go_to_console = move |_| {
        navigator.replace(Route::AdminClientList {});
    };

    rsx! {
        div { class: "container container--narrow",
            div { class: "page-surface", inert: inert::attr(is_ready),
                h1 { "Confirm your password" }
                if is_loading {
                    Alert {
                        tone: AlertTone::Status,
                        message: Some("Checking your session…".to_owned()),
                    }
                } else if !is_ready {
                    div { class: "error", role: "alert",
                        p {
                            if let Some(message) = session_error {
                                "{message}"
                            } else {
                                "Your session ended. Sign in again."
                            }
                        }
                        Link { class: "button", to: Route::Login {}, "Go to sign-in" }
                    }
                }
            }
            if is_ready {
                AdminReauthDialog {
                    csrf,
                    on_confirmed: go_to_console,
                    on_cancel: go_to_console,
                }
            }
        }
    }
}
