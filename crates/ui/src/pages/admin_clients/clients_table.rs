//! One page of clients, as a table.
//!
//! Colocated with the list page because only that page lists clients. It is a
//! pure function of the rows it is handed: nothing here reads or writes page
//! state, so it re-renders only when a page of rows actually changes.
//!
//! The status columns answer different questions on purpose. "Account" and
//! "Tenant" report what the backend said, word for word; "Provisioning"
//! answers only whether provisioning is still running, which is the console's
//! own summary of several backend fields. Repeating the tenant state in both
//! places is how a reader learns to distrust a table.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::admin_api::ClientView;
use crate::components::status_badge::{StatusBadge, ToneBadge};
use crate::presentation::StatusTone;
use crate::routes::Route;

/// The columns one page of clients is shown in.
#[component]
pub fn ClientsTable(items: Vec<ClientView>) -> Element {
    rsx! {
        div {
            class: "table-scroll",
            role: "region",
            "aria-label": "Clients table",
            tabindex: "0",
            table { class: "client-list data-table",
                caption { class: "visually-hidden", "Clients on this page" }
                thead {
                    tr {
                        th { scope: "col", "Name" }
                        th { scope: "col", "Client id" }
                        th { scope: "col", "Account" }
                        th { scope: "col", "Tenant" }
                        th { scope: "col", "Provisioning" }
                        th { scope: "col", "Plan" }
                        th { scope: "col", "Schema" }
                        th { scope: "col", "Version" }
                    }
                }
                tbody {
                    for client in items {
                        tr { key: "{client.account_id}",
                            td {
                                Link {
                                    to: Route::AdminClientDetail {
                                        account_id: client.account_id.clone(),
                                    },
                                    "{client.display_name}"
                                }
                            }
                            td { code { "{client.account_id}" } }
                            td { StatusBadge { value: client.account_status.clone() } }
                            td {
                                div { class: "status-stack",
                                    StatusBadge { value: client.tenant_status.clone() }
                                    if let Some(reason) = client.safe_provisioning_reason() {
                                        span { class: "status-detail", "{reason}" }
                                    }
                                }
                            }
                            td {
                                if client.is_provisioning() {
                                    ToneBadge {
                                        tone: StatusTone::Warning,
                                        label: "Provisioning".to_owned(),
                                    }
                                } else {
                                    span { class: "status-detail", "—" }
                                }
                            }
                            td { "{client.plan_version}" }
                            td { "{client.schema_version}" }
                            td { "{client.version}" }
                        }
                    }
                }
            }
        }
    }
}
