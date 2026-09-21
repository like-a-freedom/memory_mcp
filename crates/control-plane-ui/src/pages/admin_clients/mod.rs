//! `/admin/clients` — create clients and watch them provision.
//!
//! Three things here are easy to get wrong, and each is stated once:
//!
//! * the session comes from the console layout, so this page and the detail page
//!   cannot disagree about who is signed in;
//! * the page read is [`use_paged_query`], which owns the generation guard, the
//!   session-ended rule and the poll's "do not move the pager" rule;
//! * creating a client is idempotent by operation id, and the id is generated
//!   once per deliberate create action so a lost response cannot create two.
//!
//! The table itself lives in `clients_table.rs`, beside the page that renders
//! it: it has one consumer, so it does not belong in `components/`.

use dioxus::prelude::*;
use dioxus_router::hooks::use_navigator;

use crate::admin_api::{
    AdminApi, ClientView, OperationId, fresh_operation_id, keeps_operation_id, sleep_ms,
    validate_name,
};
use crate::components::alert::{Alert, AlertTone};
use crate::components::session_bar::AdminSessionBar;
use crate::presentation::KEY_PRIVILEGE_NOTE;
use crate::state::admin_session::{
    SESSION_ENDED, SESSION_ENDED_BEFORE_MUTATION, end_session, use_console_session,
};
use crate::state::paged_query::use_paged_query;
use crate::state::polling::{POLL_INTERVAL_MS, next_backoff};

mod clients_table;

use clients_table::ClientsTable;

/// Clients requested per page; the backend documents 50 with a maximum of 100.
const CLIENT_PAGE_LIMIT: u16 = 50;

#[component]
pub fn AdminClientListPage() -> Element {
    let mut session = use_console_session();
    let navigator = use_navigator();
    let sign_out = move |_| end_session(session, navigator);

    let mut query = use_paged_query(|cursor| async move {
        AdminApi::new()
            .clients(cursor.as_deref(), CLIENT_PAGE_LIMIT)
            .await
    });

    let mut display_name = use_signal(String::new);
    let mut create_pending = use_signal(|| false);
    let mut create_error = use_signal(|| None::<String>);
    // One operation id per deliberate create action, kept for manual retries of
    // that same request so a lost response cannot create a second client.
    let mut operation = use_signal(|| None::<OperationId>);

    // Poll the visible page while any row is still provisioning, backing off on
    // failure. The task belongs to this scope, so unmounting cancels it.
    use_future(move || {
        let mut query = query;
        async move {
            let mut backoff = 0_u32;
            loop {
                let delay = if backoff == 0 {
                    POLL_INTERVAL_MS
                } else {
                    backoff
                };
                sleep_ms(delay).await;
                let (cursor, poll_now) = {
                    let current = query.page.peek();
                    (
                        current.cursor().map(ToOwned::to_owned),
                        !current.is_loading()
                            && current
                                .items()
                                .iter()
                                .any(ClientView::stays_visible_when_polling),
                    )
                };
                if !poll_now {
                    continue;
                }
                let generation = query.generation();
                match AdminApi::new()
                    .clients(cursor.as_deref(), CLIENT_PAGE_LIMIT)
                    .await
                {
                    Ok(rows) if query.is_current(generation) => {
                        backoff = 0;
                        query.accept_poll(rows, generation);
                    }
                    Ok(_) => {
                        // A manual request won while this poll was in flight; its
                        // answer is no longer the one on screen.
                        backoff = 0;
                    }
                    Err(failure) => {
                        // A poll a manual request overtook must not report its own
                        // failure over the newer page: the rows on screen are no
                        // longer the ones this answer was about.
                        query.fail_poll(generation, failure.user_message().to_owned());
                        backoff = next_backoff(backoff);
                    }
                }
            }
        }
    });

    let mut create = move |name: String| {
        if *create_pending.peek() {
            return;
        }
        create_pending.set(true);
        create_error.set(None);
        spawn(async move {
            let existing = operation.peek().clone();
            let id = match existing {
                Some(id) => id,
                None => match fresh_operation_id() {
                    Ok(id) => {
                        operation.set(Some(id.clone()));
                        id
                    }
                    Err(failure) => {
                        create_pending.set(false);
                        create_error.set(Some(failure.user_message().to_owned()));
                        return;
                    }
                },
            };
            let Some(api) = session.peek().mutation_client() else {
                create_pending.set(false);
                create_error.set(Some(SESSION_ENDED_BEFORE_MUTATION.to_owned()));
                session.write().mark_ended(SESSION_ENDED.to_owned());
                return;
            };
            match api.create_client(&name, &id).await {
                Ok(_created) => {
                    operation.set(None);
                    display_name.set(String::new());
                    create_pending.set(false);
                    query.first_page();
                }
                Err(failure) => {
                    create_pending.set(false);
                    if !keeps_operation_id(&failure) {
                        // A definite refusal: the retry is a fresh action.
                        operation.set(None);
                    }
                    create_error.set(Some(failure.user_message().to_owned()));
                }
            }
        });
    };

    let create_submit = move |event: FormEvent| {
        event.prevent_default();
        if *create_pending.peek() {
            return;
        }
        match validate_name(&display_name.read()) {
            Ok(name) => create(name),
            Err(rejection) => create_error.set(Some(rejection.message().to_owned())),
        }
    };
    let retry = move |_| query.first_page();

    let loaded = query.page.read().clone();
    let session_ready = session.read().is_ready();
    let session_loading = session.read().is_loading();
    // Position within the walk, stated once so the pager and the footnote cannot
    // describe different pages.
    let page_summary = if loaded.has_next() {
        format!("Page {}. More clients are available.", loaded.page_number())
    } else {
        format!("Page {}. This is the last page.", loaded.page_number())
    };

    rsx! {
        header { class: "page-header",
            h1 { "Clients" }
            button {
                r#type: "button",
                onclick: retry,
                disabled: loaded.is_loading(),
                "Refresh"
            }
        }
        AdminSessionBar { session, on_sign_out: sign_out }
        p { class: "privilege-note", "{KEY_PRIVILEGE_NOTE}" }
        // The form is offered only once the session is known to be usable, so a
        // signed-out operator is never invited to fill in something that cannot
        // be submitted.
        if session_ready {
            form { class: "create-client", onsubmit: create_submit,
                div { class: "field",
                    label { r#for: "client-display-name", "New client name" }
                    input {
                        id: "client-display-name",
                        name: "display-name",
                        r#type: "text",
                        autocomplete: "off",
                        required: true,
                        "aria-describedby": "client-display-name-hint",
                        value: "{display_name}",
                        oninput: move |event| display_name.set(event.value()),
                    }
                    p { id: "client-display-name-hint", class: "hint",
                        "Up to 100 characters. This is the name every console page shows."
                    }
                }
                Alert { tone: AlertTone::Error, message: create_error.read().clone() }
                button { r#type: "submit", disabled: *create_pending.read(),
                    if *create_pending.read() { "Creating…" } else { "Create client" }
                }
                Alert {
                    tone: AlertTone::Status,
                    message: create_pending.read().then(|| "Creating the client…".to_owned()),
                }
            }
        } else if !session_loading {
            p { class: "hint", "Sign in again to create clients." }
        }
        if let Some(message) = loaded.error() {
            Alert { tone: AlertTone::Error, message: Some(message.to_owned()) }
            div { class: "actions",
                button {
                    r#type: "button",
                    onclick: retry,
                    disabled: loaded.is_loading(),
                    "Try again"
                }
            }
        }
        // The rows keep their place while a newer page is fetched, so a refresh
        // or a page step does not blank the table the operator is reading. A
        // failure without rows says so, and a failure with rows shows the rows
        // and the reason they may be stale.
        if !loaded.items().is_empty() {
            ClientsTable { items: loaded.items().to_vec() }
            footer { class: "table-footer",
                p { class: "hint", "{page_summary}" }
                if loaded.has_previous() || loaded.has_next() {
                    nav { class: "actions", "aria-label": "Client pages",
                        button {
                            r#type: "button",
                            onclick: move |_| query.previous(),
                            disabled: !loaded.has_previous() || loaded.is_loading(),
                            "Previous page"
                        }
                        button {
                            r#type: "button",
                            onclick: move |_| query.next(),
                            disabled: !loaded.has_next() || loaded.is_loading(),
                            "Next page"
                        }
                    }
                }
            }
        } else if loaded.error().is_none() {
            if loaded.is_loaded() {
                p { class: "empty", "No clients yet. Create the first one above." }
            } else {
                Alert { tone: AlertTone::Status, message: Some("Loading clients…".to_owned()) }
            }
        }
        p { class: "hint",
            "Client ids are stable. Failed clients are shown with the reason the backend reported, and are never retried from here."
        }
    }
}
