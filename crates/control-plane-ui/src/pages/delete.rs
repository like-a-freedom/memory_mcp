//! Account deletion page.
//!
//! Both steps are irreversible in effect, so both refuse a second submission
//! while one is in flight: a double-click on "Confirm deletion" must not send
//! two confirmations. Both are also rendered as destructive controls, because a
//! destructive action that looks like a safe one is how an operator deletes an
//! account by muscle memory.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::api::{ApiClient, DeleteChallenge};
use crate::components::alert::{Alert, AlertTone};
use crate::components::timestamp::Timestamp;
use crate::routes::Route;

/// `/delete` — start and confirm account deletion.
#[component]
pub fn DeletePage() -> Element {
    let mut challenge = use_signal(|| None::<DeleteChallenge>);
    let mut phrase = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut pending = use_signal(|| false);
    let mut complete = use_signal(|| false);

    let start_delete = move |event: FormEvent| {
        event.prevent_default();
        if *pending.peek() {
            return;
        }
        error.set(None);
        pending.set(true);
        spawn(async move {
            match ApiClient::new("/".to_owned()).start_delete().await {
                Ok(value) => challenge.set(Some(value)),
                Err(value) => error.set(Some(value.message)),
            }
            pending.set(false);
        });
    };

    let confirm_delete = move |event: FormEvent| {
        event.prevent_default();
        if *pending.peek() {
            return;
        }
        let Some(value) = challenge.read().clone() else {
            error.set(Some("Start the deletion confirmation first.".to_owned()));
            return;
        };
        let typed_phrase = phrase.read().trim().to_owned();
        if typed_phrase.is_empty() {
            error.set(Some("Type the confirmation phrase.".to_owned()));
            return;
        }
        error.set(None);
        pending.set(true);
        spawn(async move {
            match ApiClient::new("/".to_owned())
                .confirm_delete(value.confirmation_token, typed_phrase)
                .await
            {
                Ok(()) => complete.set(true),
                Err(value) => error.set(Some(value.message)),
            }
            pending.set(false);
        });
    };

    let loaded_challenge = challenge.read().clone();
    let pending_now = *pending.read();

    rsx! {
        div { class: "container container--narrow",
            h1 { "Delete account" }
            // A plain panel rather than a live region: nothing has happened yet,
            // and an alert that fires on load reports a problem the operator does
            // not have.
            div { class: "warning",
                p { "Deletion is irreversible. No export and no recovery is available." }
            }
            Alert { tone: AlertTone::Error, message: error.read().clone() }
            if *complete.read() {
                Alert {
                    tone: AlertTone::Success,
                    message: Some("Deletion requested. Access has been revoked.".to_owned()),
                }
            } else if let Some(value) = loaded_challenge.as_ref() {
                form { onsubmit: confirm_delete,
                    p { "Type the phrase below exactly to confirm." }
                    // Selectable as one unit: the phrase is meant to be copied or
                    // retyped, and splitting it across a line break is how that
                    // goes wrong.
                    div { class: "confirm-phrase",
                        code { "{value.typed_phrase}" }
                    }
                    div { class: "field",
                        label { r#for: "delete-confirmation-phrase", "Confirmation phrase" }
                        input {
                            id: "delete-confirmation-phrase",
                            name: "confirmation-phrase",
                            r#type: "text",
                            autocomplete: "off",
                            "autocapitalize": "none",
                            spellcheck: "false",
                            required: true,
                            value: "{phrase}",
                            oninput: move |event| phrase.set(event.value()),
                        }
                    }
                    div { class: "actions",
                        button { r#type: "submit", class: "danger", disabled: pending_now,
                            if pending_now { "Confirming…" } else { "Confirm deletion" }
                        }
                        Link { class: "button", to: Route::Status {}, "Back to status" }
                    }
                    p { class: "hint",
                        "This confirmation expires at "
                        Timestamp { value: value.expires_at.clone() }
                        "."
                    }
                }
            } else {
                form { onsubmit: start_delete,
                    div { class: "actions",
                        button { r#type: "submit", class: "danger", disabled: pending_now,
                            if pending_now { "Starting…" } else { "Start deletion" }
                        }
                        Link { class: "button", to: Route::Status {}, "Back to status" }
                    }
                }
            }
        }
    }
}
