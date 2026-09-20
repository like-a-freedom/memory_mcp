//! Account status page.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::api::ApiClient;
use crate::presentation::{compact_timestamp, status_badge_class, status_label};
use crate::router::Route;

#[component]
pub fn StatusPage() -> Element {
    let navigator = use_navigator();
    let account = use_resource(|| async { ApiClient::new("/".to_string()).me().await });
    let mut signing_out = use_signal(|| false);
    let mut logout_error = use_signal(|| None::<String>);

    let sign_out = move |_| {
        // Hide the cached account before the navigation completes. The
        // server-side `/auth/oidc/logout` clears the cookie and invalidates the
        // session; the resource itself remains read-only.
        signing_out.set(true);
        logout_error.set(None);
        spawn(async move {
            let api = ApiClient::new("/".to_owned());
            match api.logout().await {
                Ok(()) => {
                    navigator.replace(Route::Login {});
                }
                Err(_value) => {
                    signing_out.set(false);
                    logout_error.set(Some(
                        "The console could not confirm sign-out. Close this tab or try again."
                            .to_owned(),
                    ));
                }
            }
        });
    };

    rsx! {
        div { class: "container",
            h1 { "Account status" }
            if *signing_out.read() {
                p { class: "status", role: "status", "aria-live": "polite", "Signing out…" }
            }
            if let Some(message) = logout_error.read().as_ref() {
                p { class: "warning", role: "alert", "aria-live": "assertive", "{message}" }
            }
            if !*signing_out.read() {
                match account.read().as_ref() {
                    None => rsx! {
                        p { class: "status", role: "status", "aria-live": "polite", "Loading account…" }
                    },
                    Some(Err(err)) => rsx! {
                        p { class: "error", role: "alert", "aria-live": "assertive", "{err.message}" }
                    },
                    Some(Ok(meta)) => rsx! {
                        table {
                            caption { class: "visually-hidden", "Account metadata" }
                            tbody {
                                tr { th { scope: "row", "ID" } td { code { "{meta.id}" } } }
                                tr {
                                    th { scope: "row", "Status" }
                                    td {
                                        span {
                                            class: "{status_badge_class(&meta.status)}",
                                            "{status_label(&meta.status)}"
                                        }
                                    }
                                }
                                tr { th { scope: "row", "Tenant" } td { code { "{meta.tenant_id}" } } }
                                tr {
                                    th { scope: "row", "Created" }
                                    td {
                                        time {
                                            class: "timestamp",
                                            datetime: "{meta.created_at}",
                                            title: "{meta.created_at}",
                                            "{compact_timestamp(&meta.created_at)}"
                                        }
                                    }
                                }
                            }
                        }
                    },
                }
            }
            nav { class: "actions", "aria-label": "Account",
                Link { class: "button", to: Route::Keys {}, "API keys" }
                Link { class: "button", to: Route::Delete {}, "Delete account" }
                button { r#type: "button", onclick: sign_out, "Sign out" }
            }
        }
    }
}
