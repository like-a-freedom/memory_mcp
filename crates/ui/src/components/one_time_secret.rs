//! The panel that shows a secret exactly once.
//!
//! Three rules are enforced here rather than at each call site, because each one
//! is the difference between a credential that was delivered and one that was
//! lost with no second chance:
//!
//! * the value is rendered once, and the console never fetches it again;
//! * closing is gated on delivery: the clipboard accepting the value is proof
//!   enough to leave in one step, and an operator who captured the value by
//!   hand says so — so a stray Escape cannot lose a secret nobody has taken;
//! * the copy action reports whether the clipboard actually accepted the value,
//!   instead of leaving the operator to assume it did.
//!
//! No control is ever swapped for another one here. The armed-confirm pattern
//! this replaces unmounted the button that held focus, focus fell to the
//! document body, and Escape — which reaches the frame only from inside it —
//! went dead in the one state where losing the secret mattered most.

use dioxus::prelude::*;

use crate::admin_api::copy_to_clipboard;
use crate::components::alert::{Alert, AlertTone};
use crate::components::modal::{Modal, ModalKind, claim_initial_focus};

/// The one sentence that must accompany every one-time secret.
const WARNING: &str = "Copy this key now and store it somewhere safe. It cannot be shown again.";

/// The recovery path, stated beside the value: a lost secret is replaced, not
/// recovered.
const RECOVERY: &str = "If you lose it, revoke this key and create a new one.";

/// Shown in the panel once the clipboard has accepted the value.
const COPIED: &str = "Secret copied to the clipboard.";

/// Shown when closing is attempted before the secret is known to be taken.
const SAVE_FIRST: &str = "Save this key before closing: it cannot be shown again.";

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
    /// What the secret belongs to, shown under the value.
    #[props(default)]
    detail: Option<String>,
    /// Called when the operator has released the secret.
    on_dismiss: EventHandler<()>,
) -> Element {
    // Delivery evidence: the clipboard accepting the value, or the operator
    // saying they took it some other way. A successful copy checks the box
    // because the value has left the panel toward the operator's own storage;
    // the checkbox is the same claim made by hand for the select-and-copy path.
    let mut saved = use_signal(|| false);
    let mut warned = use_signal(|| false);
    let mut notice = use_signal(|| None::<String>);

    // Escape and Done take the same path. An untaken secret does not leave the
    // panel on a stray keypress: the attempt is answered with the warning, and
    // because no control is swapped, focus stays where it is and Escape keeps
    // working. Held as an `EventHandler` rather than a closure because two
    // handlers need it, and an `EventHandler` is `Copy` where a closure is not.
    let request_close = EventHandler::new(move |_: ()| {
        if *saved.peek() {
            on_dismiss.call(());
        } else {
            warned.set(true);
        }
    });

    let copyable = secret.clone();
    let copy = move |_| {
        let value = copyable.clone();
        notice.set(Some("Copying…".to_owned()));
        spawn(async move {
            match copy_to_clipboard(&value).await {
                Ok(()) => {
                    notice.set(Some(COPIED.to_owned()));
                    saved.set(true);
                }
                Err(failure) => notice.set(Some(failure.user_message().to_owned())),
            }
        });
    };

    rsx! {
        Modal {
            id,
            kind: ModalKind::Alert,
            title,
            description: WARNING.to_owned(),
            on_dismiss: move |_| request_close.call(()),
            code { class: "secret-value", "{secret}" }
            if let Some(detail) = detail {
                p { "{detail}" }
            }
            p { class: "hint", "{RECOVERY}" }
            // The panel's primary action, not the frame: this is the control the
            // operator opened the panel for.
            button {
                r#type: "button",
                onmounted: move |event: MountedEvent| claim_initial_focus(&event),
                onclick: copy,
                "Copy secret"
            }
            Alert { tone: AlertTone::Status, message: notice.read().clone() }
            label { class: "secret-confirm",
                input {
                    r#type: "checkbox",
                    checked: *saved.read(),
                    onchange: move |event| saved.set(event.checked()),
                }
                "I have saved this key"
            }
            if *warned.read() && !*saved.read() {
                Alert { tone: AlertTone::Warning, message: Some(SAVE_FIRST.to_owned()) }
            }
            button {
                r#type: "button",
                disabled: !*saved.read(),
                onclick: move |_| request_close.call(()),
                "Done"
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{RECOVERY, SAVE_FIRST, WARNING};

    #[test]
    fn the_warning_states_the_one_time_property() {
        // The delivery contract the whole panel exists for: the value is
        // rendered exactly once, and the copy is what says so out loud.
        assert!(WARNING.contains("cannot be shown again"));
    }

    #[test]
    fn the_recovery_line_names_the_way_out() {
        // A lost secret has one recovery — a replacement key — and the operator
        // is told which action produces it rather than left to assume.
        assert!(RECOVERY.contains("revoke"));
    }

    #[test]
    fn the_close_refusal_names_the_consequence() {
        assert!(SAVE_FIRST.contains("cannot be shown again"));
    }
}
