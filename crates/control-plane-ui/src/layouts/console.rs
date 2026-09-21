//! Chrome shared by the signed-in administrator routes.
//!
//! The client list and the client detail page are two views of one console, and
//! the operator moves between them constantly. Everything they must both show —
//! the page frame, and the one place a dead or unconfirmed session is reported —
//! is rendered here, so neither page can forget it and neither can phrase it
//! differently.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::Outlet;
use dioxus_router::hooks::use_navigator;

use crate::components::alert::{Alert, AlertTone};
use crate::routes::Route;
use crate::state::admin_session::{end_session, provide_admin_session};

/// The signed-in console: one session, one page frame, one session notice.
///
/// The session is loaded here and published through context, so both child
/// routes read the same instance: drilling from the list into one client does
/// not re-read it, and a page cannot display a different session from the notice
/// rendered directly above it.
#[component]
pub fn ConsoleLayout() -> Element {
    let navigator = use_navigator();
    let session = provide_admin_session();
    let retry_sign_out = move |_| end_session(session, navigator);

    rsx! {
        div { class: "container",
            if session.read().has_ended() {
                div { class: "error", role: "alert",
                    p {
                        if let Some(message) = session.read().error() {
                            "{message}"
                        } else {
                            "Your session ended. Sign in again to manage clients."
                        }
                    }
                    Link { class: "button", to: Route::Login {}, "Go to sign-in" }
                }
            } else if session.read().sign_out_unconfirmed() {
                // The cookie may still be live, so this is not "signed out":
                // it is an unfinished action with a retry right beside it.
                div { class: "warning", role: "alert",
                    p {
                        if let Some(message) = session.read().error() {
                            "{message}"
                        } else {
                            "The server did not confirm sign-out."
                        }
                    }
                    button { r#type: "button", onclick: retry_sign_out, "Try signing out again" }
                }
            } else {
                Alert {
                    tone: AlertTone::Error,
                    message: session.read().error().map(ToOwned::to_owned),
                }
            }
            Outlet::<Route> {}
        }
    }
}
