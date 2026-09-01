//! API key management page.

use dioxus::prelude::*;

use crate::api::ApiClient;

#[component]
pub fn KeysPage() -> Element {
    let mut keys = use_signal(Vec::<crate::api::ApiKeyMeta>::new);
    let mut new_key_secret = use_signal(|| None::<String>);
    let mut new_key_name = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);

    use_effect(move || {
        let api = ApiClient::new("/".to_string());
        spawn(async move {
            match api.list_keys().await {
                Ok(list) => keys.set(list),
                Err(e) => error.set(Some(e.message)),
            }
        });
    });

    let create_key = move |_| {
        let api = ApiClient::new("/".to_string());
        let name = new_key_name.read().clone();
        spawn(async move {
            match api.create_key(name).await {
                Ok(resp) => {
                    new_key_secret.set(Some(resp.secret));
                    // Refresh list
                    if let Ok(list) = api.list_keys().await {
                        keys.set(list);
                    }
                }
                Err(e) => error.set(Some(e.message)),
            }
        });
    };

    rsx! {
        div { class: "container",
            h1 { "API Keys" }
            if let Some(err) = error.read().as_ref() {
                p { class: "error", "Error: {err}" }
            }
            if let Some(secret) = new_key_secret.read().as_ref() {
                div { class: "alert",
                    p { "Your new API key (shown once):" }
                    code { "{secret}" }
                    p { "Copy this now. It won't be shown again." }
                }
            }
            div { class: "create-key",
                input {
                    placeholder: "Key name",
                    value: "{new_key_name}",
                    oninput: move |e| new_key_name.set(e.value()),
                }
                button { onclick: create_key, "Create Key" }
            }
            table {
                thead {
                    tr { th { "Name" } th { "Status" } th { "Created" } th { "Expires" } th { "Actions" } }
                }
                tbody {
                    for key in keys.read().iter() {
                        tr {
                            td { "{key.name}" }
                            td { "{key.status}" }
                            td { "{key.created_at}" }
                            td { "{key.expires_at.as_deref().unwrap_or(\"never\")}" }
                            td {
                                button {
                                    onclick: {
                                        let id = key.id.clone();
                                        move |_| {
                                            let api = ApiClient::new("/".to_string());
                                            let id = id.clone();
                                            spawn(async move {
                                                let _ = api.revoke_key(id).await;
                                            });
                                        }
                                    },
                                    "Revoke"
                                }
                            }
                        }
                    }
                }
            }
            nav {
                a { href: "/", "Back to Status" }
            }
        }
    }
}
