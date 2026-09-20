//! Account status page.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::api::ApiClient;
use crate::router::Route;

#[component]
pub fn StatusPage() -> Element {
    let navigator = use_navigator();
    let mut account = use_signal(|| None::<crate::api::AccountMeta>);
    let mut error = use_signal(|| None::<String>);

    use_effect(move || {
        let api = ApiClient::new("/".to_string());
        spawn(async move {
            match api.me().await {
                Ok(meta) => account.set(Some(meta)),
                Err(e) => error.set(Some(e.message)),
            }
        });
    });

    let sign_out = move |_| {
        // Drop the cached account so the SPA stops displaying the previous
        // identity before the navigation completes. The server-side
        // `/auth/oidc/logout` clears the cookie and invalidates the session.
        account.set(None);
        spawn(async move {
            let api = ApiClient::new("/".to_string());
            let _ = api.logout().await;
            navigator.replace(Route::Login {});
        });
    };

    rsx! {
        div { class: "container",
            h1 { "Account status" }
            if let Some(err) = error.read().as_ref() {
                p { class: "error", role: "alert", "aria-live": "assertive", "{err}" }
            }
            if let Some(meta) = account.read().as_ref() {
                table {
                    caption { class: "visually-hidden", "Account metadata" }
                    tbody {
                        tr { th { scope: "row", "ID" } td { code { "{meta.id}" } } }
                        tr { th { scope: "row", "Status" } td { "{meta.status}" } }
                        tr { th { scope: "row", "Tenant" } td { code { "{meta.tenant_id}" } } }
                        tr { th { scope: "row", "Created" } td { "{meta.created_at}" } }
                    }
                }
            } else if error.read().is_none() {
                p { class: "status", role: "status", "aria-live": "polite", "Loading account…" }
            }
            nav { class: "actions", "aria-label": "Account",
                Link { class: "button", to: Route::Keys {}, "API keys" }
                Link { class: "button", to: Route::Delete {}, "Delete account" }
                button { r#type: "button", onclick: sign_out, "Sign out" }
            }
        }
    }
}
