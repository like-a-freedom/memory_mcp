//! Suspend and resume, with their confirmation step.
//!
//! Colocated with the detail page because only that page offers the controls.
//! The component renders and reports; the page owns the mutation, because the
//! page is what holds the session and the compare-and-set version.

use dioxus::prelude::*;

use crate::admin_api::{ClientStateAction, ClientView};
use crate::components::modal::claim_initial_focus;

/// The state controls for one ready or suspended client.
#[component]
pub fn ClientStateControls(
    view: ClientView,
    /// A mutation is in flight, so the controls are locked.
    pending: ReadSignal<bool>,
    /// The action the operator asked to confirm, if any.
    confirming: ReadSignal<Option<ClientStateAction>>,
    on_request: EventHandler<ClientStateAction>,
    on_confirm: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        section { class: "client-state",
            h2 { "State" }
            if let Some(action) = *confirming.read() {
                div {
                    class: "confirm",
                    role: "group",
                    "aria-labelledby": "client-state-question",
                    p { id: "client-state-question", "{question(action)}" }
                    button {
                        r#type: "button",
                        disabled: *pending.read(),
                        onclick: move |_| on_confirm.call(()),
                        "Confirm"
                    }
                    // The safe exit takes initial focus: a stray Enter must not
                    // suspend a client before the question has been read.
                    button {
                        r#type: "button",
                        onmounted: move |event: MountedEvent| claim_initial_focus(&event),
                        onclick: move |_| on_cancel.call(()),
                        "Cancel"
                    }
                }
            } else if view.can_suspend() {
                button {
                    r#type: "button",
                    disabled: *pending.read(),
                    onclick: move |_| on_request.call(ClientStateAction::Suspend),
                    "Suspend client"
                }
            } else if view.can_resume() {
                button {
                    r#type: "button",
                    disabled: *pending.read(),
                    onclick: move |_| on_request.call(ClientStateAction::Resume),
                    "Resume client"
                }
            } else {
                p { class: "hint",
                    "Suspend and resume are offered only for a ready client, or for a client this workflow suspended."
                }
            }
        }
    }
}

/// The consequence of the action, stated before it is taken.
fn question(action: ClientStateAction) -> &'static str {
    match action {
        ClientStateAction::Suspend => {
            "Suspend this client? Every request made with its keys is rejected until it is resumed."
        }
        ClientStateAction::Resume => "Resume this client? Its existing keys become usable again.",
    }
}

#[cfg(test)]
mod tests {
    use super::question;
    use crate::admin_api::ClientStateAction;

    #[test]
    fn each_confirmation_states_its_consequence() {
        assert!(question(ClientStateAction::Suspend).contains("rejected until it is resumed"));
        assert!(question(ClientStateAction::Resume).contains("usable again"));
    }
}
