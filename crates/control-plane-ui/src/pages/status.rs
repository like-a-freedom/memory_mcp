//! The account status page.
//!
//! This is the OIDC/account surface: it authenticates with a browser cookie and
//! holds no administrator session, so it neither loads the console session nor
//! shares the console layout.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::api::ApiClient;
use crate::components::alert::{Alert, AlertTone};
use crate::components::status_badge::StatusBadge;
use crate::components::timestamp::Timestamp;
use crate::routes::Route;
use crate::state::account_session::end_account_session;

/// `/` — who the account is, and where else it can go.
#[component]
pub fn StatusPage() -> Element {
    let navigator = use_navigator();
    // `use_resource` owns loading, cancellation and the latest result, and
    // `restart` gives the operator an explicit retry.
    let mut account = use_resource(|| async { ApiClient::new("/".to_owned()).me().await });
    let signing_out = use_signal(|| false);
    let logout_error = use_signal(|| None::<String>);
    let sign_out = move |_| end_account_session(navigator, signing_out, logout_error);
    let signing_out_now = *signing_out.read();

    rsx! {
        div { class: "container container--narrow",
            h1 { "Account status" }
            if signing_out_now {
                Alert { tone: AlertTone::Status, message: Some("Signing out…".to_owned()) }
            }
            Alert { tone: AlertTone::Warning, message: logout_error.read().clone() }
            // The signed-out state hides the cached account rather than leaving
            // a stale copy on screen behind the navigation.
            if !signing_out_now {
                match account.read().as_ref() {
                    None => rsx! {
                        Alert {
                            tone: AlertTone::Status,
                            message: Some("Loading account…".to_owned()),
                        }
                    },
                    Some(Err(err)) => rsx! {
                        Alert { tone: AlertTone::Error, message: Some(err.message.clone()) }
                        // Recovery first: an account page without a session is
                        // one sign-in away from working, not one reload away.
                        div { class: "actions",
                            Link { class: "button", to: Route::Login {}, "Sign in" }
                            button { r#type: "button", onclick: move |_| account.restart(), "Try again" }
                        }
                    },
                    Some(Ok(meta)) => rsx! {
                        table {
                            caption { class: "visually-hidden", "Account metadata" }
                            tbody {
                                tr { th { scope: "row", "ID" } td { code { "{meta.id}" } } }
                                tr {
                                    th { scope: "row", "Status" }
                                    td { StatusBadge { value: meta.status.clone() } }
                                }
                                tr { th { scope: "row", "Tenant" } td { code { "{meta.tenant_id}" } } }
                                tr {
                                    th { scope: "row", "Created" }
                                    td { Timestamp { value: meta.created_at.clone() } }
                                }
                            }
                        }
                        // Account actions are shown only to a loaded account:
                        // leading a sessionless visitor to `/delete` is how the
                        // destructive flow used to dead-end on "not found".
                        nav { class: "actions", "aria-label": "Account",
                            Link { class: "button", to: Route::Keys {}, "API keys" }
                            // Destructive, so it must not look like its neighbour: the
                            // colour is the only warning an operator gets before the page
                            // that asks them to type a confirmation phrase.
                            Link {
                                class: "button button--danger",
                                to: Route::Delete {},
                                "Delete account"
                            }
                            button { r#type: "button", onclick: sign_out, "Sign out" }
                        }
                    },
                }
            }
        }
    }
}
