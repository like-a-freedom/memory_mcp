//! The panel that shows a secret exactly once.
//!
//! The value stays in page memory and is never fetched again. Copy reports the
//! clipboard's actual result; Copy and close dismisses only after a successful
//! write. Close and Escape also support operators who copy the value manually,
//! without requiring a separate attestation checkbox. The one-time warning and
//! recovery path stay visible beside the value.

use dioxus::prelude::*;

use crate::admin_api::copy_to_clipboard;
use crate::components::alert::{Alert, AlertTone};
use crate::components::modal::{Modal, ModalKind, claim_initial_focus};

/// The one sentence that must accompany every one-time secret.
const WARNING: &str = "Copy this key now and store it somewhere safe. It cannot be shown again.";

/// The recovery path, stated beside the value: a lost secret is replaced, not
/// recovered.
const RECOVERY: &str = "If you lose it, revoke this key and create a new one.";

/// `secret` is the only occurrence of the value: it is never persisted, logged,
/// or placed in a URL.
#[component]
pub fn OneTimeSecret(
    /// Base for the panel's element ids; see [`Modal`].
    id: String,
    /// The panel's accessible name, e.g. "New key secret".
    title: String,
    /// The value to deliver.
    secret: String,
    /// What the secret belongs to.
    #[props(default)]
    detail: Option<String>,
    /// Called when the operator has released the secret.
    on_dismiss: EventHandler<()>,
) -> Element {
    let mut copying = use_signal(|| false);
    let mut copied = use_signal(|| false);
    let mut copy_error = use_signal(|| None::<String>);
    let value_id = format!("{id}-value");
    let description_id = format!("{id}-description");

    let copyable = secret.clone();
    let copy = EventHandler::new(move |close_after_copy: bool| {
        if *copying.peek() {
            return;
        }
        let value = copyable.clone();
        copying.set(true);
        copied.set(false);
        copy_error.set(None);
        spawn(async move {
            let result = copy_to_clipboard(&value).await;
            copying.set(false);
            match result {
                Ok(()) if close_after_copy => on_dismiss.call(()),
                Ok(()) => copied.set(true),
                Err(failure) => copy_error.set(Some(failure.user_message().to_owned())),
            }
        });
    });

    rsx! {
        Modal {
            id,
            kind: ModalKind::Alert,
            title,
            description: WARNING.to_owned(),
            on_dismiss,
            if let Some(detail) = detail {
                p { class: "secret-detail", "{detail}" }
            }
            div { class: "secret-field",
                label { r#for: "{value_id}", "API key" }
                div { class: "secret-copy-row",
                    input {
                        id: "{value_id}",
                        class: "secret-value",
                        r#type: "text",
                        readonly: true,
                        autocomplete: "off",
                        spellcheck: "false",
                        "aria-describedby": "{description_id}",
                        value: "{secret}",
                    }
                    button {
                        class: "secret-copy",
                        r#type: "button",
                        // Native disabled blurs the active button. Keep it in
                        // the focus order so Escape still reaches the frame;
                        // the copy handler's guard refuses another write.
                        "aria-disabled": "{copying}",
                        onmounted: move |event: MountedEvent| claim_initial_focus(&event),
                        onclick: move |_| copy.call(false),
                        if *copying.read() { "Copying…" }
                        else if *copied.read() { "Copied" }
                        else { "Copy" }
                    }
                }
                p { class: "hint", "{RECOVERY}" }
                p { class: "secret-copy-status", role: "status", "aria-live": "polite",
                    if *copied.read() { "API key copied to the clipboard." }
                }
                Alert { tone: AlertTone::Error, message: copy_error.read().clone() }
            }
            div { class: "secret-actions",
                button {
                    class: "button--primary",
                    r#type: "button",
                    "aria-disabled": "{copying}",
                    onclick: move |_| copy.call(true),
                    "Copy and close"
                }
                button {
                    r#type: "button",
                    onclick: move |_| on_dismiss.call(()),
                    "Close"
                }
            }
        }
    }
}
