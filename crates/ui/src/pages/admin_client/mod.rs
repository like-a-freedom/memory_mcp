//! `/admin/clients/:account_id` — one client's metadata, keys and state.
//!
//! This module composes the page; the four things it needs are each somewhere
//! they can be read on their own:
//!
//! * `state.rs` loads the client, keeps it current while it provisions and pages
//!   its keys;
//! * `mutations.rs` holds the four mutations, which share one compare-and-set
//!   version, one in-flight flag and one re-authentication refusal;
//! * `keys_panel.rs` renders the keys section, with its form and its pager;
//! * `metadata.rs`, `state_controls.rs` and `keys_table.rs` render the remaining
//!   sections.
//!
//! Every section renders state and reports intent; none of them performs I/O.
//!
//! Key issuance is the one flow with a retry contract worth stating: the
//! operation id is generated once per deliberate issuance and reused by every
//! manual retry of that same request, so a lost response cannot mint a second
//! secret. Nothing on this page retries by itself.
//!
//! There is no "are you sure you want to leave" guard, and that is deliberate.
//! While a secret is on screen the page content is `inert` behind the secret's
//! own modal, so the way out of this page cannot be reached at all: the guard
//! could never fire, and a protection that cannot fire is worse than none,
//! because it reads as one. The modal's own two-stage close is the guard.

mod keys_panel;
mod keys_table;
mod metadata;
mod mutations;
mod state;
mod state_controls;

use dioxus::prelude::*;
use dioxus_router::hooks::use_navigator;

use crate::admin_api::{ClientStateAction, browser_now_millis};
use crate::components::admin_auth::AdminReauthDialog;
use crate::components::alert::{Alert, AlertTone};
use crate::components::one_time_secret::OneTimeSecret;
use crate::components::session_bar::AdminSessionBar;
use crate::inert;
use crate::presentation::KEY_PRIVILEGE_NOTE;
use crate::routes::Route;
use crate::state::admin_session::end_session;

use keys_panel::{KeysAvailability, KeysPanel};
use metadata::ClientMetadata;
use state::use_page_state;
use state_controls::ClientStateControls;

#[component]
pub fn AdminClientDetailPage(account_id: String) -> Element {
    let navigator = use_navigator();
    let mut state = use_page_state(account_id.clone());

    let sign_out = move |_| {
        // Do not wait for navigation to unmount the page: the one-time secret
        // must disappear as soon as the operator starts signing out.
        state.secret.set(None);
        state.operation.set(None);
        state.fields.clear();
        state.pending.set(false);
        end_session(state.session, navigator);
    };
    let leave_page = move |_| {
        navigator.push(Route::AdminClientList {});
    };
    let dismiss_secret = move |_| state.secret.set(None);

    let issue = {
        let id = account_id.clone();
        move |_| state.issue_key(&id)
    };
    let revoke = {
        let id = account_id.clone();
        move |key_id: String| state.revoke_key(&id, key_id)
    };
    let confirm_state_change = {
        let id = account_id.clone();
        move |_| {
            let action = *state.confirm_action.peek();
            if let Some(action) = action {
                state.run_state_change(&id, action);
            }
        }
    };
    let confirmed_reauth = {
        let id = account_id.clone();
        move |_| state.confirm_reauth(&id)
    };

    let loaded = state.client.read().clone();
    let session_ready = state.session.read().is_ready();
    let refused = *state.refused.read();
    let modal_open = state.secret.read().is_some() || refused.is_some();
    // The panel's availability is decided in one place, in the order that
    // matters: a session that cannot mutate is reported as such even when the
    // client itself is ready.
    let availability = if !session_ready {
        KeysAvailability::SessionEnded
    } else if loaded.as_ref().is_some_and(|view| view.can_issue_keys()) {
        KeysAvailability::Open
    } else {
        KeysAvailability::ClientNotReady
    };

    rsx! {
        div { class: "page-surface", inert: inert::attr(modal_open),
            header { class: "page-header",
                h1 {
                    if let Some(view) = loaded.as_ref() {
                        "{view.display_name}"
                    } else {
                        "Client detail"
                    }
                }
                // The way out belongs next to the title, not only after a long
                // key table.
                button { r#type: "button", onclick: leave_page, "Back to clients" }
            }
            AdminSessionBar { session: state.session, on_sign_out: sign_out }
            p { class: "privilege-note", "{KEY_PRIVILEGE_NOTE}" }
            Alert { tone: AlertTone::Error, message: state.error.read().clone() }
            if state.error.read().is_some() && loaded.is_none() {
                div { class: "actions",
                    button { r#type: "button", onclick: move |_| state.reload(), "Try again" }
                }
            }
            Alert {
                tone: AlertTone::Status,
                message: state
                    .notice
                    .read()
                    .clone()
                    .or_else(|| state.pending.read().then(|| "Working…".to_owned())),
            }

            if let Some(view) = loaded.as_ref() {
                ClientMetadata { view: view.clone() }
                ClientStateControls {
                    view: view.clone(),
                    pending: state.pending,
                    confirming: state.confirm_action,
                    on_request: move |action: ClientStateAction| state.confirm_action.set(Some(action)),
                    on_confirm: confirm_state_change,
                    on_cancel: move |_| state.confirm_action.set(None),
                }
                KeysPanel {
                    availability,
                    fields: state.fields,
                    keys: state.keys.page,
                    now_millis: browser_now_millis(),
                    pending: *state.pending.read(),
                    retrying: state.operation.read().is_some(),
                    on_issue: issue,
                    on_revoke: revoke,
                    on_retry: move |_| state.keys.first_page(),
                    on_next: move |_| state.keys.next(),
                    on_previous: move |_| state.keys.previous(),
                }
            } else {
                // Only claim to be loading when nothing has gone wrong: a failed
                // read that also says "Loading client…" reports two different
                // things about the same request, and the retry lives with the
                // error above rather than beside a claim of progress.
                if state.error.read().is_none() {
                    Alert { tone: AlertTone::Status, message: Some("Loading client…".to_owned()) }
                }
            }
        }
        if let Some(created) = state.secret.read().as_ref() {
            OneTimeSecret {
                id: "new-key-secret",
                title: "New key secret",
                secret: created.secret.clone(),
                detail: Some(format!("Key {} ({})", created.name, created.id)),
                on_dismiss: dismiss_secret,
            }
        }
        if let Some(refusal) = refused {
            AdminReauthDialog {
                csrf: state.session.read().csrf(),
                reason: refusal.reason().to_owned(),
                on_confirmed: confirmed_reauth,
                on_cancel: move |_| state.refused.set(None),
            }
        }
    }
}
