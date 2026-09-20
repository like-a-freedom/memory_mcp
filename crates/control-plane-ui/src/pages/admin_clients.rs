//! Local admin client list.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::admin_api::{
    AdminApi, ClientView, OperationId, fresh_operation_id, keeps_operation_id, sleep_ms,
    validate_name,
};
use crate::pages::admin_paging::{PageDirection, Paged, next_generation};
use crate::pages::admin_session::{AdminSessionBar, end_session, use_admin_session};
use crate::presentation::{status_badge_class, status_label};
use crate::router::Route;

/// Clients requested per page; the backend documents 50 with a maximum of 100.
const CLIENT_PAGE_LIMIT: u16 = 50;

#[derive(Clone, PartialEq)]
struct ClientPageRequest {
    cursor: Option<String>,
    direction: PageDirection,
    generation: u64,
}

/// Poll interval while a visible client is still provisioning.
pub const POLL_INTERVAL_MS: u32 = 2_000;

/// Ceiling for the poll backoff after a failed poll.
pub const POLL_MAX_BACKOFF_MS: u32 = 30_000;

/// Next poll delay after a failed poll: double, capped at 30 seconds.
pub const fn next_backoff(current: u32) -> u32 {
    if current == 0 {
        POLL_INTERVAL_MS * 2
    } else if current >= POLL_MAX_BACKOFF_MS {
        POLL_MAX_BACKOFF_MS
    } else {
        let doubled = current * 2;
        if doubled > POLL_MAX_BACKOFF_MS {
            POLL_MAX_BACKOFF_MS
        } else {
            doubled
        }
    }
}

/// `/admin/clients` — create clients and watch them provision.
#[component]
pub fn AdminClientListPage() -> Element {
    let mut session = use_admin_session();
    let navigator = use_navigator();
    let sign_out = move |_| end_session(session, navigator);
    let mut state = use_signal(Paged::<ClientView>::default);
    let mut page_request = use_signal(|| ClientPageRequest {
        cursor: None,
        direction: PageDirection::Replace,
        generation: 0,
    });
    let mut display_name = use_signal(String::new);
    let mut create_pending = use_signal(|| false);
    let mut create_error = use_signal(|| None::<String>);
    // One operation id per deliberate create action, kept for manual retries of
    // that same request so a lost response cannot create a second client.
    let mut operation = use_signal(|| None::<OperationId>);

    let page = use_resource(move || {
        let request = page_request.read().clone();
        async move {
            match AdminApi::new()
                .clients(request.cursor.as_deref(), CLIENT_PAGE_LIMIT)
                .await
            {
                Ok(page) => Ok((page, request)),
                Err(failure) => Err((request, failure)),
            }
        }
    });

    use_effect(move || {
        let Some(outcome) = page.read().clone() else {
            state.write().begin();
            return;
        };
        let current_generation = page_request.peek().generation;
        match outcome {
            Ok((page, request)) if request.generation == current_generation => state
                .write()
                .accept(page, request.cursor, request.direction),
            Ok(_) => {}
            Err((request, failure)) if request.generation == current_generation => {
                if failure.ends_session() {
                    session
                        .write()
                        .mark_ended("Your session ended. Sign in again.".to_owned());
                } else {
                    state.write().fail(failure.user_message().to_owned());
                }
            }
            Err(_) => {}
        }
    });

    // Poll the visible page while any row is still provisioning, backing off on
    // failure. The task belongs to this scope, so unmounting cancels it.
    use_future(move || async move {
        let mut backoff = 0_u32;
        loop {
            let delay = if backoff == 0 {
                POLL_INTERVAL_MS
            } else {
                backoff
            };
            sleep_ms(delay).await;
            let (cursor, poll_now) = {
                let current = state.peek();
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
            let generation = page_request.peek().generation;
            match AdminApi::new()
                .clients(cursor.as_deref(), CLIENT_PAGE_LIMIT)
                .await
            {
                Ok(page) if page_request.peek().generation == generation => {
                    backoff = 0;
                    state.write().accept(page, cursor, PageDirection::Replace);
                }
                Ok(_) => {
                    backoff = 0;
                }
                Err(failure) => {
                    // Keep the rows already on screen and retry with backoff.
                    state.write().fail(failure.user_message().to_owned());
                    backoff = next_backoff(backoff);
                }
            }
        }
    });

    let create = move |event: FormEvent| {
        event.prevent_default();
        if *create_pending.peek() {
            return;
        }
        let name = match validate_name(&display_name.read()) {
            Ok(value) => value,
            Err(rejection) => {
                create_error.set(Some(rejection.message().to_owned()));
                return;
            }
        };
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
                create_error.set(Some(
                    "Your session ended. Sign in again to continue.".to_owned(),
                ));
                session
                    .write()
                    .mark_ended("Your session ended. Sign in again.".to_owned());
                return;
            };
            match api.create_client(&name, &id).await {
                Ok(_created) => {
                    operation.set(None);
                    display_name.set(String::new());
                    create_pending.set(false);
                    request_clients(&mut page_request, state, None, PageDirection::Replace);
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

    let first_page =
        move |_| request_clients(&mut page_request, state, None, PageDirection::Replace);
    let next_page = move |_| {
        let cursor = state.peek().next_cursor();
        request_clients(&mut page_request, state, cursor, PageDirection::Next);
    };
    let previous_page = move |_| {
        if let Some(cursor) = state.peek().previous_cursor() {
            request_clients(&mut page_request, state, cursor, PageDirection::Previous);
        }
    };

    rsx! {
        div { class: "container",
            h1 { "Clients" }
            AdminSessionBar { session, on_sign_out: sign_out }
            if session.read().has_ended() {
                div { class: "error", role: "alert",
                    p {
                        if let Some(message) = session.read().error() {
                            "{message}"
                        } else {
                            "Your session ended. Sign in again to manage clients."
                        }
                    }
                    Link { class: "button", to: Route::Login {}, "Go to sign-in" }
                }
            } else {
                if let Some(message) = session.read().error() {
                    p { class: "error", role: "alert", "{message}" }
                }
                p { class: "privilege-note",
                    "Administrators can issue keys that access this client's memory. Issuance is audited."
                }
                // The form is offered only once the session is known to be
                // usable, so a signed-out operator is never invited to fill in
                // something that cannot be submitted.
                if session.read().is_ready() {
                    form { class: "create-client", onsubmit: create,
                        div { class: "field",
                            label { r#for: "client-display-name", "New client name" }
                            input {
                                id: "client-display-name",
                                name: "display-name",
                                r#type: "text",
                                autocomplete: "off",
                                required: true,
                                value: "{display_name}",
                                oninput: move |event| display_name.set(event.value()),
                            }
                        }
                        button { r#type: "submit", disabled: *create_pending.read(),
                            if *create_pending.read() { "Creating…" } else { "Create client" }
                        }
                        div { class: "error", role: "alert", "aria-live": "assertive",
                            if let Some(message) = create_error.read().as_ref() {
                                "{message}"
                            }
                        }
                        p { class: "status", role: "status", "aria-live": "polite",
                            if *create_pending.read() { "Creating the client…" }
                        }
                    }
                } else if !session.read().is_loading() {
                    p { class: "hint", "Sign in again to create clients." }
                }
                if let Some(message) = state.read().error() {
                    p { class: "error", role: "alert", "aria-live": "assertive", "{message}" }
                }
                div { class: "actions",
                    button { r#type: "button", onclick: first_page, disabled: state.read().is_loading(),
                        "Refresh"
                    }
                    button {
                        r#type: "button",
                        onclick: previous_page,
                        disabled: !state.read().has_previous() || state.read().is_loading(),
                        "Previous page"
                    }
                    button {
                        r#type: "button",
                        onclick: next_page,
                        disabled: !state.read().has_next() || state.read().is_loading(),
                        "Next page"
                    }
                }
                if state.read().is_loading() {
                    p { class: "status", role: "status", "aria-live": "polite", "Loading clients…" }
                } else if !state.read().is_loaded() {
                    p { class: "hint", "No page of clients has loaded yet." }
                } else if state.read().items().is_empty() {
                    p { class: "empty", "No clients yet. Create the first one above." }
                } else {
                    div {
                        class: "table-scroll",
                        role: "region",
                        "aria-label": "Clients table",
                        tabindex: "0",
                        table { class: "client-list",
                            caption { class: "visually-hidden", "Clients on this page" }
                            thead {
                                tr {
                                    th { scope: "col", "Name" }
                                    th { scope: "col", "Client id" }
                                    th { scope: "col", "Account" }
                                    th { scope: "col", "Tenant" }
                                    th { scope: "col", "Plan" }
                                    th { scope: "col", "Schema" }
                                    th { scope: "col", "Version" }
                                    th { scope: "col", "Provisioning" }
                                }
                            }
                            tbody {
                                for client in state.read().items() {
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
                                        td {
                                            span {
                                                class: "{status_badge_class(&client.account_status)}",
                                                "{status_label(&client.account_status)}"
                                            }
                                        }
                                        td {
                                            span {
                                                class: "{status_badge_class(&client.tenant_status)}",
                                                "{provisioning_label(&client)}"
                                            }
                                        }
                                        td { "{client.plan_version}" }
                                        td { "{client.schema_version}" }
                                        td { "{client.version}" }
                                        td {
                                            div { class: "status-stack",
                                                span {
                                                    class: "{status_badge_class(&client.tenant_status)}",
                                                    "{provisioning_label(&client)}"
                                                }
                                                if client.is_failed() {
                                                    span { class: "status-detail",
                                                        "{client.safe_provisioning_reason().unwrap_or_else(|| \"no reason reported\".to_owned())}"
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
                p { class: "hint",
                    "Client ids are stable. Failed clients are shown with the reason the backend reported, and are never retried from here."
                }
            }
        }
    }
}

/// Short label for a row's provisioning state, including the safe reason the
/// backend reported for a failure.
fn provisioning_label(client: &ClientView) -> String {
    if client.is_provisioning() {
        "Provisioning".to_owned()
    } else {
        status_label(client.state_label())
    }
}

/// Request one page of clients. The generation makes every deliberate request
/// distinct, so a resource restart cannot reuse a stale cursor request.
fn request_clients(
    request: &mut Signal<ClientPageRequest>,
    mut state: Signal<Paged<ClientView>>,
    cursor: Option<String>,
    direction: PageDirection,
) {
    state.write().begin();
    let generation = {
        let current = request.peek().generation;
        next_generation(current)
    };
    request.set(ClientPageRequest {
        cursor,
        direction,
        generation,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poll_backoff_doubles_and_caps_at_thirty_seconds() {
        assert_eq!(next_backoff(0), POLL_INTERVAL_MS * 2);
        assert_eq!(next_backoff(POLL_INTERVAL_MS * 2), POLL_INTERVAL_MS * 4);
        assert_eq!(next_backoff(POLL_INTERVAL_MS * 4), POLL_INTERVAL_MS * 8);
        // 4s, 8s, 16s, then the ceiling: 32s is clamped to 30s.
        assert_eq!(next_backoff(POLL_INTERVAL_MS * 8), POLL_MAX_BACKOFF_MS);
        assert_eq!(next_backoff(POLL_MAX_BACKOFF_MS), POLL_MAX_BACKOFF_MS);
        // Absurd input cannot overflow or exceed the ceiling.
        assert_eq!(next_backoff(u32::MAX), POLL_MAX_BACKOFF_MS);
    }
}
