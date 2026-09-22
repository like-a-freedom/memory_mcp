//! The keys section of the client detail page.
//!
//! Colocated with the page because only that page lists one client's keys. It
//! renders and reports; the page owns the mutation, because it shares the page's
//! in-flight slot and compare-and-set version.
//!
//! The one thing the panel owns is *which* row is being confirmed. That is a
//! question about this panel's rows, and the answer is only ever read here.

use dioxus::prelude::*;

use crate::admin_api::{ApiKeyMeta, MAX_EXPIRY_DAYS, MIN_EXPIRY_DAYS};
use crate::components::alert::{Alert, AlertTone};
use crate::state::query::Paged;

use super::keys_table::KeysTable;
use super::state::KeyIssueFields;

/// Whether this client can be issued keys, and why not when it cannot.
///
/// One value rather than two booleans because the two questions have one answer
/// between them, and the order they are asked in decides which sentence the
/// operator reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum KeysAvailability {
    /// The session can mutate and the client is ready for data access.
    Open,
    /// The session cannot mutate: the operator must sign in again.
    SessionEnded,
    /// The client is not ready for data access.
    ClientNotReady,
}

impl KeysAvailability {
    /// What the panel says instead of offering the form.
    const fn closed_reason(self) -> Option<&'static str> {
        match self {
            Self::Open => None,
            Self::SessionEnded => Some("Sign in again before issuing keys for this client."),
            Self::ClientNotReady => {
                Some("Keys can be issued only while the client is ready for data access.")
            }
        }
    }
}

/// The keys one client holds, the form that adds another, and the pager.
#[component]
pub(super) fn KeysPanel(
    availability: KeysAvailability,
    /// The form's fields, owned by the page: a manual retry of a lost issuance
    /// resends them with the same operation id.
    fields: KeyIssueFields,
    /// One page of key metadata. Subscribed to here rather than read by the page,
    /// so a poll or a page change re-renders this panel and nothing else.
    keys: ReadSignal<Paged<ApiKeyMeta>>,
    /// The browser clock, for labelling an expired key. A value, not a signal:
    /// it is read fresh on every render, so a key that expires while the page is
    /// open is relabelled rather than frozen as it was when the page opened.
    now_millis: Option<i64>,
    /// A mutation is in flight, so every control that starts one is locked.
    pending: bool,
    /// An operation id is already held for a key issuance, which is what a retry
    /// of that same request reuses.
    retrying: bool,
    on_issue: EventHandler<()>,
    /// The key id to revoke, once the operator has confirmed it.
    on_revoke: EventHandler<String>,
    on_retry: EventHandler<()>,
    on_next: EventHandler<()>,
    on_previous: EventHandler<()>,
) -> Element {
    let page = keys.read().clone();
    // A local binding because the fields are written from two event handlers, and
    // a parameter is not a mutable place. `KeyIssueFields` is `Copy`, so this
    // copies three handles rather than cloning the values they hold.
    let mut fields = fields;
    let mut confirming = use_signal(|| None::<ApiKeyMeta>);
    let page_summary = if page.has_next() {
        format!("Page {}. More keys are available.", page.page_number())
    } else {
        format!("Page {}. This is the last page.", page.page_number())
    };

    rsx! {
        section { class: "keys",
            h2 { "API keys" }
            p { class: "hint", "Expired and revoked keys no longer authenticate." }
            if let Some(reason) = availability.closed_reason() {
                p { class: "hint", "{reason}" }
            } else {
                form {
                    class: "issue-key",
                    onsubmit: move |event: FormEvent| {
                        event.prevent_default();
                        on_issue.call(());
                    },
                    div { class: "field",
                        label { r#for: "key-name", "Key name" }
                        input {
                            id: "key-name",
                            name: "key-name",
                            r#type: "text",
                            autocomplete: "off",
                            required: true,
                            value: "{fields.name}",
                            oninput: move |event| fields.name.set(event.value()),
                        }
                    }
                    fieldset { class: "expiry",
                        legend { "Expiry" }
                        div {
                            label { r#for: "key-expiry-days", "Expires after" }
                            input {
                                id: "key-expiry-days",
                                name: "expiry-days",
                                r#type: "number",
                                min: "{MIN_EXPIRY_DAYS}",
                                max: "{MAX_EXPIRY_DAYS}",
                                step: "1",
                                value: "{fields.days}",
                                oninput: move |event| fields.enter_days(event.value()),
                            }
                            span { "days ({MIN_EXPIRY_DAYS}–{MAX_EXPIRY_DAYS})" }
                        }
                        div {
                            label { r#for: "key-expiry-never", "Never expires" }
                            input {
                                id: "key-expiry-never",
                                name: "expiry-never",
                                r#type: "checkbox",
                                checked: *fields.never.read(),
                                onchange: move |event| fields.toggle_never(event.checked()),
                            }
                        }
                        p { class: "hint", "Choose exactly one. There is no default." }
                    }
                    // What the form rejected is said beside the form.
                    Alert { tone: AlertTone::Error, message: fields.error.read().clone() }
                    button { r#type: "submit", disabled: pending,
                        if pending { "Issuing…" } else { "Issue key" }
                    }
                    if retrying {
                        p { class: "hint",
                            "Retrying this request cannot issue a second key."
                        }
                    }
                }
            }
            if let Some(message) = page.error() {
                Alert { tone: AlertTone::Error, message: Some(message.to_owned()) }
                div { class: "actions",
                    button {
                        r#type: "button",
                        onclick: move |_| on_retry.call(()),
                        disabled: page.is_loading(),
                        "Retry keys"
                    }
                }
            }
            // The rows keep their place while a newer page is fetched, so a
            // retry or a page step does not blank the table the operator is
            // reading. A failure without rows says so, and a failure with rows
            // shows the rows and the reason they may be stale.
            if !page.items().is_empty() {
                KeysTable {
                    items: page.items().to_vec(),
                    now_millis,
                    pending,
                    confirming: confirming.read().as_ref().map(|key| key.id.clone()),
                    on_revoke: move |target: ApiKeyMeta| confirming.set(Some(target)),
                    on_confirm: move |key_id: String| {
                        confirming.set(None);
                        on_revoke.call(key_id);
                    },
                    on_cancel: move |_| confirming.set(None),
                }
                footer { class: "table-footer",
                    p { class: "hint", "{page_summary}" }
                    nav { class: "actions", "aria-label": "Key pages",
                        button {
                            r#type: "button",
                            onclick: move |_| on_previous.call(()),
                            disabled: !page.has_previous() || page.is_loading(),
                            "Previous keys"
                        }
                        button {
                            r#type: "button",
                            onclick: move |_| on_next.call(()),
                            disabled: !page.has_next() || page.is_loading(),
                            "Next keys"
                        }
                    }
                }
            } else if page.error().is_none() {
                if page.is_loaded() {
                    p { class: "empty", "No keys have been issued for this client." }
                } else {
                    Alert { tone: AlertTone::Status, message: Some("Loading keys…".to_owned()) }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::KeysAvailability;

    #[test]
    fn only_an_open_client_offers_the_form() {
        assert_eq!(KeysAvailability::Open.closed_reason(), None);
    }

    #[test]
    fn a_closed_form_states_the_one_reason_it_is_closed() {
        // Two booleans would let a caller order the checks wrongly and tell a
        // signed-out operator that their client is not ready, which is a
        // different action to take.
        let ended = KeysAvailability::SessionEnded.closed_reason().unwrap();
        let not_ready = KeysAvailability::ClientNotReady.closed_reason().unwrap();
        assert!(ended.contains("Sign in again"));
        assert!(not_ready.contains("ready for data access"));
        assert_ne!(ended, not_ready);
    }
}
