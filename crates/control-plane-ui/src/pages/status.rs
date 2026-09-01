//! Account status page.

use dioxus::prelude::*;

use crate::api::ApiClient;

#[component]
pub fn StatusPage() -> Element {
    let mut account = use_signal(|| None::<crate::api::AccountMeta>);
    let error = use_signal(|| None::<String>);

    use_effect(move || {
        let api = ApiClient::new("/".to_string());
        spawn(async move {
            match api.me().await {
                Ok(meta) => account.set(Some(meta)),
                Err(e) => error.set(Some(e.message)),
            }
        });
    });

    rsx! {
        div { class: "container",
            h1 { "Account Status" }
            if let Some(err) = error.read().as_ref() {
                p { class: "error", "Error: {err}" }
            }
            if let Some(meta) = account.read().as_ref() {
                table {
                    tr { td { "ID" } td { "{meta.id}" } }
                    tr { td { "Status" } td { "{meta.status}" } }
                    tr { td { "Tenant" } td { "{meta.tenant_id}" } }
                    tr { td { "Created" } td { "{meta.created_at}" } }
                }
            } else {
                p { "Loading..." }
            }
            nav {
                a { href: "/keys", "API Keys" }
                a { href: "/delete", "Delete Account" }
            }
        }
    }
}
