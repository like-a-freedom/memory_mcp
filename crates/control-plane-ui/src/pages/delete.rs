//! Account deletion page.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::api::{ApiClient, DeleteChallenge};
use crate::presentation::compact_timestamp;
use crate::router::Route;

#[component]
pub fn DeletePage() -> Element {
    let mut challenge = use_signal(|| None::<DeleteChallenge>);
    let mut phrase = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut complete = use_signal(|| false);

    let start_delete = move |event: FormEvent| {
        event.prevent_default();
        let mut error = error;
        error.set(None);
        spawn(async move {
            match ApiClient::new("/".to_owned()).start_delete().await {
                Ok(value) => challenge.set(Some(value)),
                Err(value) => error.set(Some(value.message)),
            }
        });
    };

    let confirm_delete = move |event: FormEvent| {
        event.prevent_default();
        let Some(value) = challenge.read().clone() else {
            error.set(Some("Start the deletion confirmation first.".to_owned()));
            return;
        };
        let typed_phrase = phrase.read().trim().to_owned();
        if typed_phrase.is_empty() {
            error.set(Some("Type the confirmation phrase.".to_owned()));
            return;
        }
        let mut error = error;
        error.set(None);
        spawn(async move {
            match ApiClient::new("/".to_owned())
                .confirm_delete(value.confirmation_token, typed_phrase)
                .await
            {
                Ok(()) => complete.set(true),
                Err(value) => error.set(Some(value.message)),
            }
        });
    };

    rsx! {
        div { class: "container",
            h1 { "Delete account" }
            p { "Deletion is irreversible. No export and no recovery is available." }
            if let Some(value) = error.read().as_ref() {
                p { class: "error", role: "alert", "aria-live": "assertive", "{value}" }
            }
            if *complete.read() {
                div { class: "success", role: "status", "aria-live": "polite",
                    p { "Deletion requested. Access has been revoked." }
                }
            } else if let Some(value) = challenge.read().as_ref() {
                form { onsubmit: confirm_delete,
                    p { "Type the phrase below exactly to confirm." }
                    code { "{value.typed_phrase}" }
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
                    button { r#type: "submit", "Confirm deletion" }
                    p { class: "hint",
                        "This confirmation expires at "
                        time {
                            class: "timestamp",
                            datetime: "{value.expires_at}",
                            title: "{value.expires_at}",
                            "{compact_timestamp(&value.expires_at)}"
                        }
                        "."
                    }
                }
            } else {
                form { onsubmit: start_delete,
                    button { r#type: "submit", "Start deletion" }
                }
            }
            nav { class: "actions", "aria-label": "Account",
                Link { class: "button", to: Route::Status {}, "Back to status" }
            }
        }
    }
}
