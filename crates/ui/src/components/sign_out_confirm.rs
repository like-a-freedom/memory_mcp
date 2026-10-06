//! The question asked before a session ends.
//!
//! Signing out is reversible but not free: anything typed on the page is lost,
//! and getting back in can cost a full sign-in. So it is confirmed — and it is
//! confirmed by this one panel on every surface that can sign out, because two
//! pages that end the same session must not ask for different levels of
//! certainty. The safe answer takes initial focus, exactly like every other
//! confirmation in this console: a stray Enter must not end a session before
//! the question has been read.

use dioxus::prelude::*;

use crate::components::modal::{Modal, ModalKind, claim_initial_focus};

#[component]
pub fn SignOutConfirm(
    /// End the session. The caller clears its own secrets first.
    on_confirm: EventHandler<()>,
    /// Stay signed in; the caller closes the panel.
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        Modal {
            id: "sign-out",
            kind: ModalKind::Question,
            title: "Sign out?",
            description: "You will need to sign in again. Anything you have typed will be lost."
                .to_owned(),
            on_dismiss: move |_| on_cancel.call(()),
            div { class: "actions",
                button { r#type: "button", onclick: move |_| on_confirm.call(()), "Sign out" }
                button {
                    r#type: "button",
                    onmounted: move |event: MountedEvent| claim_initial_focus(&event),
                    onclick: move |_| on_cancel.call(()),
                    "Cancel"
                }
            }
        }
    }
}
