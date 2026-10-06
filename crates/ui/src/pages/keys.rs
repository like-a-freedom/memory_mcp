//! API key management page for the signed-in account.
//!
//! This is the OIDC/account surface, so it uses the account client rather than
//! the administrator one. It shares the one-time-secret panel with the admin
//! client detail page: a secret that can only be shown once must not be
//! delivered by two different panels with different safeguards.
//!
//! Revoking is destructive and irreversible — any request already using the key
//! stops working — so it is confirmed here, exactly as it is on the
//! administrator surface: two pages that both revoke a key must not ask for
//! different levels of certainty.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::api::{ApiClient, ApiKeyMeta, CreateApiKeyResponse};
use crate::components::alert::{Alert, AlertTone};
use crate::components::modal::claim_initial_focus;
use crate::components::one_time_secret::OneTimeSecret;
use crate::components::sign_out_confirm::SignOutConfirm;
use crate::components::status_badge::StatusBadge;
use crate::components::timestamp::Timestamp;
use crate::inert;
use crate::routes::Route;
use crate::state::account_session::end_account_session;

/// `/keys` — issue and revoke this account's API keys.
#[component]
pub fn KeysPage() -> Element {
    let navigator = use_navigator();
    let mut keys = use_resource(|| async { ApiClient::same_origin().list_keys().await });
    let mut new_key = use_signal(|| None::<CreateApiKeyResponse>);
    let mut new_key_name = use_signal(String::new);
    let mut created_notice = use_signal(|| None::<String>);
    let mut error = use_signal(|| None::<String>);
    let mut pending = use_signal(|| false);
    let signing_out = use_signal(|| false);
    let logout_error = use_signal(|| None::<String>);
    let mut sign_out_confirm = use_signal(|| false);
    // Which key is about to be revoked, if any. Confirming is what clears it.
    let mut revoke_target = use_signal(|| None::<ApiKeyMeta>);

    let create_key = move |event: FormEvent| {
        event.prevent_default();
        if *pending.peek() {
            return;
        }
        let api = ApiClient::same_origin();
        let name = new_key_name.read().trim().to_owned();
        if name.is_empty() {
            error.set(Some("Enter a key name.".to_owned()));
            return;
        }
        error.set(None);
        pending.set(true);
        spawn(async move {
            match api.create_key(name).await {
                Ok(response) => {
                    new_key_name.set(String::new());
                    new_key.set(Some(response));
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
            match ApiClient::same_origin().revoke_key(id).await {
                Ok(()) => keys.restart(),
                Err(value) => error.set(Some(value.message)),
            }
            pending.set(false);
        });
    };

    let request_sign_out = move |_| sign_out_confirm.set(true);
    let confirm_sign_out = move |_| {
        sign_out_confirm.set(false);
        // A secret on screen must not outlive the sign-out request.
        new_key.set(None);
        end_account_session(navigator, signing_out, logout_error);
    };
    let cancel_sign_out = move |_| sign_out_confirm.set(false);
    let modal_open = new_key.read().is_some() || *sign_out_confirm.read();
    let signing_out_now = *signing_out.read();
    let pending_now = *pending.read();
    let confirming = revoke_target.read().as_ref().map(|key| key.id.clone());

    rsx! {
        div { class: "container container--narrow",
            div { class: "page-surface", inert: inert::attr(modal_open),
                h1 { "API keys" }
                if signing_out_now {
                    Alert { tone: AlertTone::Status, message: Some("Signing out…".to_owned()) }
                }
                Alert { tone: AlertTone::Error, message: error.read().clone() }
                Alert { tone: AlertTone::Warning, message: logout_error.read().clone() }
                Alert { tone: AlertTone::Success, message: created_notice.read().clone() }
                // The cached list is hidden rather than left on screen behind the
                // navigation: the session it was read with is already gone.
                if !signing_out_now {
                    if let Some(Err(value)) = keys.read().as_ref() {
                        Alert { tone: AlertTone::Error, message: Some(value.message.clone()) }
                        div { class: "actions",
                            Link { class: "button", to: Route::Login {}, "Sign in" }
                            button {
                                r#type: "button",
                                onclick: move |_| {
                                    error.set(None);
                                    keys.restart();
                                },
                                "Try again"
                            }
                        }
                    }
                    // The form is offered only while the account is known to be
                    // reachable — the same rule the client list applies. A form
                    // that cannot be submitted is not an invitation to type.
                    if !matches!(keys.read().as_ref(), Some(Err(_))) {
                    form { class: "create-key", onsubmit: create_key,
                        div { class: "field",
                            label { r#for: "new-key-name", "Key name" }
                            input {
                                id: "new-key-name",
                                name: "key-name",
                                r#type: "text",
                                autocomplete: "off",
                                required: true,
                                "aria-describedby": "new-key-name-hint",
                                value: "{new_key_name}",
                                oninput: move |event| new_key_name.set(event.value()),
                            }
                        }
                        button { r#type: "submit", disabled: pending_now,
                            if pending_now { "Creating…" } else { "Create key" }
                        }
                        // Beside the row, not inside the field: a hint inside the
                        // field makes the field's bottom edge the hint's, and the
                        // row's `align-items: flex-end` then drops the button a
                        // hint-line below the input it belongs to.
                        p { id: "new-key-name-hint", class: "hint",
                            "A label you will recognise. The secret is shown once."
                        }
                    }
                    }
                    match keys.read().as_ref() {
                        None => rsx! {
                            Alert {
                                tone: AlertTone::Status,
                                message: Some("Loading API keys…".to_owned()),
                            }
                        },
                        // The failure is reported above; the list is not shown.
                        Some(Err(_)) => rsx! {},
                        Some(Ok(values)) if values.is_empty() => rsx! {
                            p { class: "empty", "No API keys have been issued for this account." }
                        },
                        Some(Ok(values)) => rsx! {
                            div {
                                class: "table-scroll",
                                role: "region",
                                "aria-label": "Account API keys table",
                                tabindex: "0",
                                table { class: "data-table",
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
                                                td { StatusBadge { value: key.status.clone() } }
                                                td { Timestamp { value: key.created_at.clone() } }
                                                td {
                                                    if let Some(value) = key.expires_at.clone() {
                                                        Timestamp { value }
                                                    } else {
                                                        "Never"
                                                    }
                                                }
                                                td {
                                                    button {
                                                        r#type: "button",
                                                        class: "button--danger",
                                                        disabled: pending_now,
                                                        onclick: {
                                                            let target = key.clone();
                                                            move |_| revoke_target.set(Some(target.clone()))
                                                        },
                                                        "Revoke…"
                                                    }
                                                }
                                            }
                                            // The confirmation sits under the row it asks
                                            // about: this list can be long, and a panel
                                            // after it appears off screen for most rows.
                                            if confirming.as_deref() == Some(key.id.as_str()) {
                                                tr { class: "confirm-row",
                                                    td { colspan: "5",
                                                        div {
                                                            class: "confirm",
                                                            role: "group",
                                                            "aria-labelledby": "revoke-key-question",
                                                            p { id: "revoke-key-question",
                                                                "Revoke key {key.name}, created "
                                                                Timestamp {
                                                                    value: key.created_at.clone(),
                                                                }
                                                                "? Requests already using it stop working."
                                                            }
                                                            button {
                                                                r#type: "button",
                                                                disabled: pending_now,
                                                                onclick: {
                                                                    let id = key.id.clone();
                                                                    move |_| {
                                                                        revoke_target.set(None);
                                                                        revoke(id.clone());
                                                                    }
                                                                },
                                                                if pending_now { "Working…" } else { "Confirm revoke" }
                                                            }
                                                            // The safe exit takes initial focus: a
                                                            // stray Enter must not revoke a key
                                                            // before the question has been read.
                                                            button {
                                                                r#type: "button",
                                                                onmounted: move |event: MountedEvent| {
                                                                    claim_initial_focus(&event)
                                                                },
                                                                onclick: move |_| revoke_target.set(None),
                                                                "Cancel"
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                    }
                    nav { class: "actions actions--split", "aria-label": "Account",
                        Link { class: "button", to: Route::Status {}, "Back to status" }
                        button { r#type: "button", onclick: request_sign_out, "Sign out" }
                    }
                }
            }
        }
        if let Some(created) = new_key.read().as_ref() {
            OneTimeSecret {
                id: "new-account-key",
                title: "New API key",
                secret: created.secret.clone(),
                detail: Some(format!("Key {} ({})", created.name, created.id)),
                on_dismiss: move |_| {
                    let name = new_key.read().as_ref().map(|created| created.name.clone());
                    new_key.set(None);
                    if let Some(name) = name {
                        created_notice.set(Some(format!(
                            "Key {name} created. The secret cannot be shown again."
                        )));
                    }
                },
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
