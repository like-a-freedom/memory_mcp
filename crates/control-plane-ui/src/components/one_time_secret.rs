//! The panel that shows a secret exactly once.
//!
//! Three rules are enforced here rather than at each call site, because each one
//! is the difference between a credential that was delivered and one that was
//! lost with no second chance:
//!
//! * the value is rendered once, and the console never fetches it again;
//! * closing is confirmed the first time, so a stray Escape cannot discard a
//!   secret the operator has not saved;
//! * the copy action reports whether the clipboard actually accepted the value,
//!   instead of leaving the operator to assume it did.

use dioxus::prelude::*;

use crate::admin_api::copy_to_clipboard;
use crate::components::alert::{Alert, AlertTone};
use crate::components::modal::{Modal, ModalKind, claim_initial_focus};

/// The one sentence that must accompany every one-time secret.
const WARNING: &str = "Save this now; it cannot be shown again. Deliver it outside this service.";

/// Shown in the panel once the clipboard has accepted the value.
const COPIED: &str = "Secret copied to the clipboard. Copy it somewhere safe now.";

/// Shown while the copy is in flight and after a second close request.
const CLOSING_WARNING: &str =
    "The secret will be lost when this panel closes. It cannot be shown again.";

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
    let mut armed = use_signal(|| false);
    let mut notice = use_signal(|| None::<String>);

    // The first close request only arms the confirmation: the operator asked to
    // close a panel, and must not lose a secret by mistyping Escape.
    //
    // Held as an `EventHandler` rather than a closure because three separate
    // handlers need it, and an `EventHandler` is `Copy` where a closure is not.
    let dismiss = EventHandler::new(move |_: ()| {
        if *armed.peek() {
            on_dismiss.call(());
        } else {
            armed.set(true);
        }
    });
    let keep = move |_| armed.set(false);
    let dismiss_click = move |_| dismiss.call(());

    let copyable = secret.clone();
    let copy = move |_| {
        let value = copyable.clone();
        notice.set(Some("Copying…".to_owned()));
        spawn(async move {
            match copy_to_clipboard(&value).await {
                Ok(()) => notice.set(Some(COPIED.to_owned())),
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
            on_dismiss: move |_| dismiss.call(()),
            code { class: "secret-value", "{secret}" }
            if let Some(detail) = detail {
                p { "{detail}" }
            }
            // The panel's primary action, not the frame: this is the control the
            // operator opened the panel for.
            button {
                r#type: "button",
                onmounted: move |event: MountedEvent| claim_initial_focus(&event),
                onclick: copy,
                "Copy secret"
            }
            Alert { tone: AlertTone::Status, message: notice.read().clone() }
            if *armed.read() {
                p { role: "alert", "{CLOSING_WARNING}" }
                button { r#type: "button", class: "danger", onclick: dismiss_click, "Discard the secret" }
                button { r#type: "button", onclick: keep, "Keep it" }
            } else {
                button { r#type: "button", onclick: dismiss_click, "Close" }
            }
        }
    }
}
