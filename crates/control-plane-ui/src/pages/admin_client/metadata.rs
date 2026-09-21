//! The client's identity and lifecycle facts.
//!
//! Colocated with the page rather than hoisted into `components/`, because only
//! the detail page renders it. Extracted from the page body because it is a pure
//! function of the client view: nothing here reads or writes page state, so it
//! reruns only when the view itself changes.

use dioxus::prelude::*;

use crate::admin_api::ClientView;
use crate::components::alert::{Alert, AlertTone};
use crate::components::status_badge::StatusBadge;
use crate::presentation::Status;
use crate::state::polling::POLL_INTERVAL_SECONDS;

/// One client's metadata and whether it can serve data yet.
#[component]
pub fn ClientMetadata(view: ClientView) -> Element {
    let failed_reason = view
        .safe_provisioning_reason()
        .unwrap_or_else(|| "no reason reported".to_owned());
    let readiness = readiness(&view);

    rsx! {
        section { class: "client-metadata",
            h2 { "Client" }
            table {
                caption { class: "visually-hidden", "Client metadata" }
                tbody {
                    tr { th { scope: "row", "Client id" } td { code { "{view.account_id}" } } }
                    tr { th { scope: "row", "Tenant id" } td { code { "{view.tenant_id}" } } }
                    tr {
                        th { scope: "row", "Account status" }
                        td { StatusBadge { value: view.account_status.clone() } }
                    }
                    tr {
                        th { scope: "row", "Tenant status" }
                        td { StatusBadge { value: view.tenant_status.clone() } }
                    }
                    tr { th { scope: "row", "Plan version" } td { "{view.plan_version}" } }
                    tr { th { scope: "row", "Schema version" } td { "{view.schema_version}" } }
                    tr { th { scope: "row", "Version" } td { "{view.version}" } }
                }
            }
            p { class: "readiness", "{readiness}" }
            if view.is_failed() {
                Alert {
                    tone: AlertTone::Error,
                    message: Some(format!("Provisioning failed: {failed_reason}")),
                }
            }
        }
    }
}

/// Short readiness summary for the client header.
fn readiness(client: &ClientView) -> String {
    if client.is_provisioning() {
        format!("Provisioning. This page refreshes every {POLL_INTERVAL_SECONDS} seconds.")
    } else if client.is_failed() {
        "Provisioning failed. Keys cannot be issued.".to_owned()
    } else if client.is_ready() {
        "Ready for data access; keys can be issued.".to_owned()
    } else if client.is_suspended() {
        "Suspended by this workflow; resume to restore access.".to_owned()
    } else {
        format!("State: {}.", Status::new(client.state_label()).label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(account_status: &str, tenant_status: &str) -> ClientView {
        ClientView {
            account_id: "account:1".to_owned(),
            tenant_id: "tenant:1".to_owned(),
            display_name: "Acme".to_owned(),
            account_status: account_status.to_owned(),
            tenant_status: tenant_status.to_owned(),
            plan_version: 1,
            schema_version: 0,
            version: 3,
            provisioning_reason: None,
        }
    }

    #[test]
    fn readiness_copy_matches_the_client_state() {
        assert!(readiness(&view("active", "reserved")).contains("refreshes every 2 seconds"));
        assert!(readiness(&view("active", "failed")).contains("Keys cannot be issued"));
        assert!(readiness(&view("active", "ready")).contains("keys can be issued"));
        assert!(readiness(&view("suspended", "suspended")).contains("resume to restore"));
        assert!(readiness(&view("deleting", "deleting")).contains("Deleting"));
    }

    #[test]
    fn the_poll_interval_in_the_copy_tracks_the_interval_actually_used() {
        // The sentence an operator reads and the timer that drives it are the
        // same constant, so a change to the interval cannot leave the copy lying.
        assert_eq!(
            crate::state::polling::POLL_INTERVAL_SECONDS,
            crate::state::polling::POLL_INTERVAL_MS / 1_000
        );
    }
}
