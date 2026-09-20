//! API key management page.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::api::ApiClient;
use crate::presentation::{compact_timestamp, status_badge_class, status_label};
use crate::router::Route;

#[component]
pub fn KeysPage() -> Element {
    let navigator = use_navigator();
    let mut keys = use_resource(|| async { ApiClient::new("/".to_string()).list_keys().await });
    let mut new_key_secret = use_signal(|| None::<String>);
    let mut new_key_name = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut pending = use_signal(|| false);
    let mut logout_error = use_signal(|| None::<String>);

    let create_key = move |event: FormEvent| {
        event.prevent_default();
        if *pending.peek() {
            return;
        }
        let api = ApiClient::new("/".to_string());
        let name = new_key_name.read().trim().to_owned();
        if name.is_empty() {
            error.set(Some("Enter a key name.".to_owned()));
            return;
        }
        error.set(None);
        pending.set(true);
        spawn(async move {
            match api.create_key(name).await {
                Ok(resp) => {
                    new_key_name.set(String::new());
                    new_key_secret.set(Some(resp.secret));
                    keys.restart();
                }
                Err(value) => error.set(Some(value.message)),
            }
            pending.set(false);
        });
    };

    let mut revoke = move |id: String| {
        if *pending.peek() {
            return;
        }
        error.set(None);
        pending.set(true);
        spawn(async move {
            match ApiClient::new("/".to_string()).revoke_key(id).await {
                Ok(()) => keys.restart(),
                Err(value) => error.set(Some(value.message)),
            }
            pending.set(false);
        });
    };

    let close_secret = move |_| new_key_secret.set(None);
    let escape_secret = move |event: KeyboardEvent| {
        if event.key() == Key::Escape {
            new_key_secret.set(None);
        }
    };
    let secret_open = new_key_secret.read().is_some();
    let sign_out = move |_| {
        new_key_secret.set(None);
        logout_error.set(None);
        spawn(async move {
            let api = ApiClient::new("/".to_owned());
            match api.logout().await {
                Ok(()) => {
                    navigator.replace(Route::Login {});
                }
                Err(_value) => logout_error.set(Some(
                    "The console could not confirm sign-out. Close this tab or try again."
                        .to_owned(),
                )),
            }
        });
    };

    rsx! {
        div { class: "container",
            div { class: "page-surface", inert: secret_open,
                h1 { "API keys" }
            if let Some(err) = error.read().as_ref() {
                p { class: "error", role: "alert", "aria-live": "assertive", "{err}" }
            }
            if let Some(message) = logout_error.read().as_ref() {
                p { class: "warning", role: "alert", "aria-live": "assertive", "{message}" }
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
                button { r#type: "submit", disabled: *pending.read(),
                    if *pending.read() { "Creating…" } else { "Create key" }
                }
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
                    div {
                        class: "table-scroll",
                        role: "region",
                        "aria-label": "Account API keys table",
                        tabindex: "0",
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
                                        td {
                                            span {
                                                class: "{status_badge_class(&key.status)}",
                                                "{status_label(&key.status)}"
                                            }
                                        }
                                        td {
                                            time {
                                                class: "timestamp",
                                                datetime: "{key.created_at}",
                                                title: "{key.created_at}",
                                                "{compact_timestamp(&key.created_at)}"
                                            }
                                        }
                                        td {
                                            if let Some(value) = key.expires_at.as_deref() {
                                                time {
                                                    class: "timestamp",
                                                    datetime: "{value}",
                                                    title: "{value}",
                                                    "{compact_timestamp(value)}"
                                                }
                                            } else {
                                                "Never"
                                            }
                                        }
                                        td {
                                            button {
                                                r#type: "button",
                                                disabled: *pending.read(),
                                                onclick: {
                                                    let id = key.id.clone();
                                                    move |_| revoke(id.clone())
                                                },
                                                if *pending.read() { "Revoking…" } else { "Revoke" }
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
        if let Some(secret) = new_key_secret.read().as_ref() {
            dialog {
                class: "modal-layer",
                open: true,
                role: "alertdialog",
                "aria-modal": "true",
                "aria-labelledby": "new-account-key-title",
                "aria-describedby": "new-account-key-warning",
                tabindex: "-1",
                onkeydown: escape_secret,
                div { class: "secret",
                    h2 { id: "new-account-key-title", "New API key" }
                    p { id: "new-account-key-warning", class: "warning",
                        "Save this now; it cannot be shown again."
                    }
                    code { class: "secret-value", "{secret}" }
                    p { "Copy it somewhere safe now; it cannot be shown again." }
                    button { r#type: "button", autofocus: true, onclick: close_secret, "Close" }
                }
            }
        }
    }
}
