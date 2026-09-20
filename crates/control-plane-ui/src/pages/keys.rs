//! API key management page.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::api::ApiClient;
use crate::router::Route;

#[component]
pub fn KeysPage() -> Element {
    let navigator = use_navigator();
    let mut keys = use_resource(|| async { ApiClient::new("/".to_string()).list_keys().await });
    let mut new_key_secret = use_signal(|| None::<String>);
    let mut new_key_name = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);

    let create_key = move |event: FormEvent| {
        event.prevent_default();
        let api = ApiClient::new("/".to_string());
        let name = new_key_name.read().trim().to_owned();
        if name.is_empty() {
            error.set(Some("Enter a key name.".to_owned()));
            return;
        }
        error.set(None);
        spawn(async move {
            match api.create_key(name).await {
                Ok(resp) => {
                    new_key_name.set(String::new());
                    new_key_secret.set(Some(resp.secret));
                    keys.restart();
                }
                Err(value) => error.set(Some(value.message)),
            }
        });
    };

    let mut revoke = move |id: String| {
        error.set(None);
        spawn(async move {
            match ApiClient::new("/".to_string()).revoke_key(id).await {
                Ok(()) => keys.restart(),
                Err(value) => error.set(Some(value.message)),
            }
        });
    };

    let sign_out = move |_| {
        spawn(async move {
            let api = ApiClient::new("/".to_owned());
            let _ = api.logout().await;
            navigator.replace(Route::Login {});
        });
    };

    rsx! {
        div { class: "container",
            h1 { "API keys" }
            if let Some(err) = error.read().as_ref() {
                p { class: "error", role: "alert", "aria-live": "assertive", "{err}" }
            }
            if let Some(Err(value)) = keys.read().as_ref() {
                p { class: "error", role: "alert", "aria-live": "assertive", "{value.message}" }
                button {
                    r#type: "button",
                    onclick: move |_| {
                        error.set(None);
                        keys.restart();
                    },
                    "Try again"
                }
            }
            if let Some(secret) = new_key_secret.read().as_ref() {
                div { class: "alert", role: "status", "aria-live": "polite",
                    p { "Your new API key. It is shown once." }
                    code { "{secret}" }
                    p { "Copy it somewhere safe now; it cannot be shown again." }
                }
            }
            form { class: "create-key", onsubmit: create_key,
                div { class: "field",
                    label { r#for: "new-key-name", "Key name" }
                    input {
                        id: "new-key-name",
                        name: "key-name",
                        r#type: "text",
                        autocomplete: "off",
                        required: true,
                        value: "{new_key_name}",
                        oninput: move |event| new_key_name.set(event.value()),
                    }
                }
                button { r#type: "submit", "Create key" }
            }
            match keys.read().as_ref() {
                None => rsx! {
                    p { class: "status", role: "status", "aria-live": "polite", "Loading API keys…" }
                },
                Some(Err(_)) => rsx! {},
                Some(Ok(values)) if values.is_empty() => {
                    rsx! { p { class: "empty", "No API keys have been issued for this account." } }
                },
                Some(Ok(values)) => rsx! {
                    div { class: "table-scroll",
                        table {
                            caption { class: "visually-hidden", "API keys for this account" }
                            thead {
                                tr {
                                    th { scope: "col", "Name" }
                                    th { scope: "col", "Status" }
                                    th { scope: "col", "Created" }
                                    th { scope: "col", "Expires" }
                                    th { scope: "col", "Actions" }
                                }
                            }
                            tbody {
                                for key in values.iter() {
                                    tr { key: "{key.id}",
                                        td { "{key.name}" }
                                        td { "{key.status}" }
                                        td { "{key.created_at}" }
                                        td { "{key.expires_at.as_deref().unwrap_or(\"never\")}" }
                                        td {
                                            button {
                                                r#type: "button",
                                                onclick: {
                                                    let id = key.id.clone();
                                                    move |_| revoke(id.clone())
                                                },
                                                "Revoke"
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
            nav { class: "actions", "aria-label": "Account",
                Link { class: "button", to: Route::Status {}, "Back to status" }
                button { r#type: "button", onclick: sign_out, "Sign out" }
            }
        }
    }
}
