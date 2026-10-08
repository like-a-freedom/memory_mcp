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
use crate::components::sign_out_confirm::SignOutConfirm;
use crate::components::status_badge::StatusBadge;
use crate::components::timestamp::Timestamp;
use crate::inert;
use crate::routes::Route;
use crate::state::account_session::end_account_session;

/// `/` — who the account is, and where else it can go.
#[component]
pub fn StatusPage() -> Element {
    let navigator = use_navigator();
    // `use_resource` owns loading, cancellation and the latest result, and
    // `restart` gives the operator an explicit retry.
    let mut account = use_resource(|| async { ApiClient::same_origin().me().await });
    let signing_out = use_signal(|| false);
    let logout_error = use_signal(|| None::<String>);
    let mut sign_out_confirm = use_signal(|| false);
    let request_sign_out = move |_| sign_out_confirm.set(true);
    let confirm_sign_out = move |_| {
        sign_out_confirm.set(false);
        end_account_session(navigator, signing_out, logout_error);
    };
    let cancel_sign_out = move |_| sign_out_confirm.set(false);
    let signing_out_now = *signing_out.read();

    rsx! {
        // `inert` sits on the container because it holds the whole page, and
        // the sign-out question renders outside it as a sibling root.
        div {
            class: "container container--narrow",
            inert: inert::attr(*sign_out_confirm.read()),
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
                        div {
                            class: "table-scroll",
                            role: "region",
                            "aria-label": "Account metadata table",
                            tabindex: "0",
                            table {
                                caption { class: "visually-hidden", "Account metadata" }
                                tbody {
                                    tr { th { scope: "row", "ID" } td { code { "{meta.id}" } } }
                                    // Shown only when the provider asserted a name: an
                                    // unconditional row would leave an empty cell for
                                    // every account without one.
                                    if let Some(name) = meta.display_name.as_deref() {
                                        tr {
                                            th { scope: "row", "Display name" }
                                            td { "{name}" }
                                        }
                                    }
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
                        }
                        // Account actions are shown only to a loaded account:
                        // leading a sessionless visitor to `/delete` is how the
                        // destructive flow used to dead-end on "not found".
                        nav { class: "actions actions--split", "aria-label": "Account",
                            Link { class: "button", to: Route::Keys {}, "API keys" }
                            // Destructive, so it must not look like its neighbour: the
                            // colour is the only warning an operator gets before the page
                            // that asks them to type a confirmation phrase.
                            Link {
                                class: "button button--danger",
                                to: Route::Delete {},
                                "Delete account"
                            }
                            button { r#type: "button", onclick: request_sign_out, "Sign out" }
                        }
                    },
                }
            }
        }
        if *sign_out_confirm.read() {
            SignOutConfirm {
                on_confirm: confirm_sign_out,
                on_cancel: cancel_sign_out,
            }
        }
    }
}

/// The account page has no DOM renderer, so markup is asserted by source scan.
///
/// The scan stops at the test module. Scanning the whole file would match this
/// test's own assertion strings and pass before any row existed — a scan test
/// that can satisfy itself is worse than no test, because it looks green.
///
/// The assertion that matters is the guard: a name row rendered unconditionally
/// would leave an empty labelled cell for every account whose provider asserted
/// none, which reads as a defect rather than as an absence.
#[cfg(test)]
mod tests {
    /// The component's own source, with this test module cut off.
    fn component_source() -> &'static str {
        let source = include_str!("status.rs");
        let before_tests = source
            .find("\n#[cfg(test)]\nmod tests {")
            .expect("the test module is the last thing in the file");
        &source[..before_tests]
    }

    #[test]
    fn the_display_name_row_is_rendered_only_when_there_is_a_name() {
        let source = component_source();
        assert!(
            source.contains(r#"if let Some(name) = meta.display_name"#),
            "the name row must be behind a check on the name, so an account \
             without one shows no row"
        );
        assert!(
            source.contains(r#"th { scope: "row", "Display name" }"#),
            "the row must be labelled as the display name"
        );
        assert!(
            source.contains(r#"td { "{name}" }"#),
            "the cell must render the name itself"
        );
    }

    #[test]
    fn the_account_page_shows_no_row_when_there_is_no_name() {
        // The inverse, stated so the two cannot both pass by accident: the
        // guarded row must not also exist as an unconditional one.
        let source = component_source();
        assert_eq!(
            source.matches(r#""Display name""#).count(),
            1,
            "the label must appear exactly once, inside the guarded row"
        );
    }
}
