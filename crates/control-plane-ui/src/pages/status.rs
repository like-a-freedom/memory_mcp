//! Account status page.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::api::ApiClient;
use crate::router::Route;

#[component]
pub fn StatusPage() -> Element {
    let navigator = use_navigator();
    let account = use_resource(|| async { ApiClient::new("/".to_string()).me().await });
    let mut signing_out = use_signal(|| false);

    let sign_out = move |_| {
        // Hide the cached account before the navigation completes. The
        // server-side `/auth/oidc/logout` clears the cookie and invalidates the
        // session; the resource itself remains read-only.
        signing_out.set(true);
        spawn(async move {
            let api = ApiClient::new("/".to_string());
            let _ = api.logout().await;
            navigator.replace(Route::Login {});
        });
    };

    rsx! {
        div { class: "container",
            h1 { "Account status" }
            if *signing_out.read() {
                p { class: "status", role: "status", "aria-live": "polite", "Signing out…" }
            } else {
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
                                tr { th { scope: "row", "Status" } td { "{meta.status}" } }
                                tr { th { scope: "row", "Tenant" } td { code { "{meta.tenant_id}" } } }
                                tr { th { scope: "row", "Created" } td { "{meta.created_at}" } }
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
