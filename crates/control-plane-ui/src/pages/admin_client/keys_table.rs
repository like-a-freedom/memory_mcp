//! The table of keys already issued for one client.
//!
//! Colocated with the detail page because only that page lists a client's keys.
//! It renders rows and reports intent; the panel above it owns which row is being
//! confirmed, and the page owns the mutation.
//!
//! The revoke confirmation is a full-width row *inside* the table, directly under
//! the row it asks about. It has to be there: this table can be fifty rows long,
//! and a panel rendered after it appears off screen for every row above the fold
//! — the operator clicks "Revoke…" and nothing seems to happen. Inside the row it
//! is where the operator is already looking, and it stays in the keyboard order
//! after the button that opened it.
//!
//! The status is derived against a clock the caller passes in, so an expired key
//! is labelled from the same instant the rest of the page rendered at.

use dioxus::prelude::*;

use crate::admin_api::{ApiKeyMeta, KeyDisplayStatus};
use crate::components::status_badge::StatusBadge;
use crate::components::timestamp::Timestamp;

/// One page of key metadata.
#[component]
pub fn KeysTable(
    items: Vec<ApiKeyMeta>,
    /// The browser clock, used only to label an expired key.
    now_millis: Option<i64>,
    /// A mutation is in flight, so revoke is locked.
    pending: bool,
    /// The key whose revoke confirmation is open, if any.
    confirming: Option<String>,
    /// The operator asked to revoke this row's key.
    on_revoke: EventHandler<ApiKeyMeta>,
    /// The confirmation was accepted, for this key id.
    on_confirm: EventHandler<String>,
    on_cancel: EventHandler<()>,
) -> Element {
    rsx! {
        div {
            class: "table-scroll",
            role: "region",
            "aria-label": "API keys table",
            tabindex: "0",
            table { class: "data-table",
                caption { class: "visually-hidden", "API keys issued for this client" }
                thead {
                    tr {
                        th { scope: "col", "Name" }
                        th { scope: "col", "Status" }
                        th { scope: "col", "Created" }
                        th { scope: "col", "Expires" }
                        th { scope: "col", "Last used" }
                        th { scope: "col", "Actions" }
                    }
                }
                tbody {
                    for key in items {
                        tr { key: "{key.id}",
                            td { "{key.name}" }
                            td {
                                StatusBadge {
                                    value: key.display_status(now_millis).label().to_owned(),
                                }
                            }
                            td { Timestamp { value: key.created_at.clone() } }
                            td {
                                if let Some(value) = key.expires_at.clone() {
                                    Timestamp { value }
                                } else {
                                    "Never"
                                }
                            }
                            td {
                                if let Some(value) = key.last_used_at.clone() {
                                    Timestamp { value }
                                } else {
                                    "Not used"
                                }
                            }
                            td {
                                // A revoked key has no action left, and the status
                                // column already says so: an em dash reads as
                                // "nothing here" rather than repeating the word.
                                if key.display_status(now_millis) == KeyDisplayStatus::Revoked {
                                    span { class: "status-detail", "—" }
                                } else {
                                    button {
                                        r#type: "button",
                                        disabled: pending,
                                        onclick: {
                                            let target = key.clone();
                                            move |_| on_revoke.call(target.clone())
                                        },
                                        "Revoke…"
                                    }
                                }
                            }
                        }
                        if confirming.as_deref() == Some(key.id.as_str()) {
                            tr { class: "confirm-row",
                                td { colspan: "6",
                                    div {
                                        class: "confirm",
                                        role: "group",
                                        "aria-labelledby": "revoke-key-question",
                                        p { id: "revoke-key-question",
                                            "Revoke key {key.name} ({key.id})? Requests already using it stop working."
                                        }
                                        button {
                                            r#type: "button",
                                            autofocus: true,
                                            disabled: pending,
                                            onclick: {
                                                let id = key.id.clone();
                                                move |_| on_confirm.call(id.clone())
                                            },
                                            if pending { "Working…" } else { "Confirm revoke" }
                                        }
                                        button {
                                            r#type: "button",
                                            onclick: move |_| on_cancel.call(()),
                                            "Cancel"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
