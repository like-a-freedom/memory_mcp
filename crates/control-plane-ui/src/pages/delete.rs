//! Account deletion page.

use dioxus::prelude::*;

use crate::api::{ApiClient, DeleteChallenge};

#[component]
pub fn DeletePage() -> Element {
    let mut challenge = use_signal(|| None::<DeleteChallenge>);
    let mut phrase = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let mut complete = use_signal(|| false);

    let start_delete = move |_| {
        let mut error = error;
        spawn(async move {
            match ApiClient::new("/".to_owned()).start_delete().await {
                Ok(value) => challenge.set(Some(value)),
                Err(value) => error.set(Some(value.message)),
            }
        });
    };

    let confirm_delete = move |_| {
        let Some(value) = challenge.read().clone() else {
            error.set(Some("Start the deletion confirmation first.".to_owned()));
            return;
        };
        let typed_phrase = phrase.read().clone();
        let mut error = error;
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
            h1 { "Delete Account" }
            p { "Deletion is irreversible. No export or recovery is available." }
            if let Some(value) = error.read().as_ref() {
                p { class: "error", "Error: {value}" }
            }
            if *complete.read() {
                p { "Deletion requested. Access has been revoked." }
            } else if let Some(value) = challenge.read().as_ref() {
                p { "Type exactly: {value.typed_phrase}" }
                input {
                    value: "{phrase}",
                    oninput: move |event| phrase.set(event.value()),
                }
                button { onclick: confirm_delete, "Confirm deletion" }
                p { "Confirmation expires at {value.expires_at}." }
            } else {
                button { onclick: start_delete, "Start deletion" }
            }
            nav {
                a { href: "/", "Back to Status" }
            }
        }
    }
}
