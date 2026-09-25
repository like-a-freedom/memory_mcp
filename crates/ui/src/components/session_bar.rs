//! The banner describing the signed-in administrator.
//!
//! Used by both console routes, so the operator sees the same session facts and
//! the same control on each. The session is read-only here: the page that owns
//! the session performs the mutation through `on_sign_out`, which keeps the data
//! flow one-way and lets that page clear its own secrets before the session is
//! dropped.

use dioxus::prelude::*;

use crate::components::timestamp::Timestamp;
use crate::state::admin_session::AdminSession;

#[component]
pub fn AdminSessionBar(
    session: ReadSignal<AdminSession>,
    on_sign_out: EventHandler<()>,
) -> Element {
    let session = session.read();
    let summary = session.summary();
    let expiry = session.absolute_expiry().map(ToOwned::to_owned);
    let can_sign_out = session.can_sign_out();

    rsx! {
        div { class: "session-bar",
            span { "{summary}" }
            if let Some(expiry) = expiry {
                span { class: "session-expiry",
                    " Session ends "
                    Timestamp { value: expiry }
                }
            }
            if can_sign_out {
                button {
                    r#type: "button",
                    onclick: move |_| on_sign_out.call(()),
                    "Sign out"
                }
            }
        }
    }
}
