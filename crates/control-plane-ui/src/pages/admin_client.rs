//! Local admin client detail: metadata, readiness, keys and state changes.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::admin_api::{
    AdminApi, ApiKeyMeta, ClientStateAction, ClientView, CreatedKey, KeyDisplayStatus, OperationId,
    SessionCsrf, browser_now_millis, copy_to_clipboard, fresh_operation_id, keeps_operation_id,
    parse_expiry, sanitize_note, validate_name,
};
use crate::pages::admin_auth::AdminReauthDialog;
use crate::pages::admin_clients::{POLL_INTERVAL_MS, next_backoff};
use crate::pages::admin_paging::{PageDirection, Paged, next_generation};
use crate::pages::admin_session::{AdminSession, AdminSessionBar, end_session, use_admin_session};
use crate::presentation::{compact_timestamp, status_badge_class, status_label};
use crate::router::Route;

/// Keys requested per page; the backend documents 50 with a maximum of 100.
const KEY_PAGE_LIMIT: u16 = 50;

#[derive(Clone, PartialEq)]
struct KeyPageRequest {
    cursor: Option<String>,
    direction: PageDirection,
    generation: u64,
}

/// A mutation the backend refused with `reauth_required`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RefusedMutation {
    Suspend,
    Resume,
    /// Key issuance is never retried automatically: the operator confirms the
    /// password, then presses the button again with the same operation id.
    IssueKey,
}

impl RefusedMutation {
    fn state_action(self) -> Option<ClientStateAction> {
        match self {
            Self::Suspend => Some(ClientStateAction::Suspend),
            Self::Resume => Some(ClientStateAction::Resume),
            Self::IssueKey => None,
        }
    }

    fn reason(self) -> &'static str {
        match self {
            Self::Suspend => "suspend this client",
            Self::Resume => "resume this client",
            Self::IssueKey => "issue this key",
        }
    }
}

/// `/admin/clients/:account_id` — one client's metadata, keys and state.
#[component]
pub fn AdminClientDetailPage(account_id: String) -> Element {
    let navigator = use_navigator();
    let mut session = use_admin_session();
    let mut client = use_signal(|| None::<ClientView>);
    let mut client_generation = use_signal(|| 0_u64);
    let mut keys = use_signal(Paged::<ApiKeyMeta>::default);
    let mut key_request = use_signal(|| KeyPageRequest {
        cursor: None,
        direction: PageDirection::Replace,
        generation: 0,
    });
    // The browser clock labels expired keys but never influences an
    // authorization decision. Initialize it once for this page.
    let now_millis = use_signal(browser_now_millis);
    let mut error = use_signal(|| None::<String>);
    let mut notice = use_signal(|| None::<String>);
    let mut secret = use_signal(|| None::<CreatedKey>);
    let mut discard_armed = use_signal(|| false);
    let mut key_name = use_signal(String::new);
    let mut key_never = use_signal(|| false);
    let mut key_days = use_signal(String::new);
    let mut operation = use_signal(|| None::<OperationId>);
    let mut pending = use_signal(|| false);
    let mut confirm_action = use_signal(|| None::<ClientStateAction>);
    let mut revoke_target = use_signal(|| None::<ApiKeyMeta>);
    let mut refused = use_signal(|| None::<RefusedMutation>);
    let sign_out = move |_| {
        // Do not wait for navigation to unmount the page: the one-time secret
        // must disappear as soon as the operator starts signing out.
        secret.set(None);
        discard_armed.set(false);
        operation.set(None);
        key_name.set(String::new());
        key_days.set(String::new());
        pending.set(false);
        end_session(session, navigator);
    };

    let mut client_resource = use_resource({
        let account_id = account_id.clone();
        move || {
            let id = account_id.clone();
            async move { AdminApi::new().client(&id).await }
        }
    });
    let key_resource = use_resource({
        let account_id = account_id.clone();
        move || {
            let request = key_request.read().clone();
            let id = account_id.clone();
            async move {
                match AdminApi::new()
                    .keys(&id, request.cursor.as_deref(), KEY_PAGE_LIMIT)
                    .await
                {
                    Ok(page) => Ok((page, request)),
                    Err(failure) => Err((request, failure)),
                }
            }
        }
    });

    use_effect(move || {
        let outcome = client_resource.read().clone();
        let Some(outcome) = outcome else {
            return;
        };
        match outcome {
            Ok(view) => client.set(Some(view)),
            Err(failure) => record_failure(session, error, failure),
        }
    });
    use_effect(move || {
        let Some(outcome) = key_resource.read().clone() else {
            keys.write().begin();
            return;
        };
        let current_generation = key_request.peek().generation;
        match outcome {
            Ok((page, request)) if request.generation == current_generation => {
                keys.write().accept(page, request.cursor, request.direction)
            }
            Ok(_) => {}
            Err((request, failure)) if request.generation == current_generation => {
                if failure.ends_session() {
                    session
                        .write()
                        .mark_ended("Your session ended. Sign in again.".to_owned());
                } else {
                    keys.write().fail(failure.user_message().to_owned());
                }
            }
            Err(_) => {}
        }
    });

    // Keep the readiness display current while the client is provisioning. The
    // task belongs to this scope, so unmounting cancels it.
    use_future({
        let account_id = account_id.clone();
        move || {
            let id = account_id.clone();
            async move {
                let mut backoff = 0_u32;
                loop {
                    let delay = if backoff == 0 {
                        POLL_INTERVAL_MS
                    } else {
                        backoff
                    };
                    crate::admin_api::sleep_ms(delay).await;
                    if !client
                        .peek()
                        .as_ref()
                        .is_some_and(ClientView::is_provisioning)
                    {
                        continue;
                    }
                    let generation = *client_generation.peek();
                    match AdminApi::new().client(&id).await {
                        Ok(view) if *client_generation.peek() == generation => {
                            backoff = 0;
                            client.set(Some(view));
                        }
                        Ok(_) => {
                            // A manual refresh won while this poll was in
                            // flight; its result is no longer authoritative.
                            backoff = 0;
                        }
                        Err(_) => backoff = next_backoff(backoff),
                    }
                }
            }
        }
    });

    let mut refresh = move || {
        bump_generation(&mut client_generation);
        client_resource.restart();
        request_keys(&mut key_request, keys, None, PageDirection::Replace);
    };

    let mut leave_page = {
        let account_id = account_id.clone();
        move || {
            // A secret that has not been discarded must not be lost silently.
            if secret.peek().is_some() && !*discard_armed.peek() {
                discard_armed.set(true);
                return;
            }
            secret.set(None);
            let _ = account_id;
            navigator.push(Route::AdminClientList {});
        }
    };

    let close_secret = move |_| {
        if secret.peek().is_none() {
            return;
        }
        if *discard_armed.peek() {
            secret.set(None);
            discard_armed.set(false);
            notice.set(None);
        } else {
            discard_armed.set(true);
        }
    };

    let keep_secret = move |_| discard_armed.set(false);
    let escape_secret = move |event: KeyboardEvent| {
        if event.key() == Key::Escape {
            if *discard_armed.peek() {
                secret.set(None);
                discard_armed.set(false);
                notice.set(None);
            } else {
                discard_armed.set(true);
            }
        }
    };

    let copy_secret = move |_| {
        let Some(value) = secret.peek().as_ref().map(|created| created.secret.clone()) else {
            return;
        };
        notice.set(Some("Copying…".to_owned()));
        spawn(async move {
            match copy_to_clipboard(&value).await {
                Ok(()) => notice.set(Some(
                    "Secret copied to the clipboard. Copy it somewhere safe now.".to_owned(),
                )),
                Err(failure) => notice.set(Some(failure.user_message().to_owned())),
            }
        });
    };

    let issue_key = {
        let account_id = account_id.clone();
        move |event: FormEvent| {
            event.prevent_default();
            if *pending.peek() {
                return;
            }
            let name = match validate_name(&key_name.read()) {
                Ok(value) => value,
                Err(rejection) => {
                    error.set(Some(rejection.message().to_owned()));
                    return;
                }
            };
            let expiry = match parse_expiry(*key_never.peek(), &key_days.read()) {
                Ok(value) => value,
                Err(rejection) => {
                    error.set(Some(rejection.message().to_owned()));
                    return;
                }
            };
            pending.set(true);
            error.set(None);
            notice.set(None);
            let id = account_id.clone();
            spawn(async move {
                // One operation id per deliberate issuance, reused by a manual
                // retry so the backend can answer `secret_already_issued`
                // instead of returning a second secret.
                let existing = operation.peek().clone();
                let operation_id = match existing {
                    Some(existing) => existing,
                    None => match fresh_operation_id() {
                        Ok(fresh) => {
                            operation.set(Some(fresh.clone()));
                            fresh
                        }
                        Err(failure) => {
                            pending.set(false);
                            error.set(Some(failure.user_message().to_owned()));
                            return;
                        }
                    },
                };
                let Some(api) = session.peek().mutation_client() else {
                    pending.set(false);
                    error.set(Some(
                        "Your session ended. Sign in again to continue.".to_owned(),
                    ));
                    return;
                };
                let outcome = api.issue_key(&id, &name, &expiry, &operation_id).await;
                pending.set(false);
                match outcome {
                    Ok(created) => {
                        // The idempotency record is finished with.
                        operation.set(None);
                        key_name.set(String::new());
                        key_days.set(String::new());
                        key_never.set(false);
                        discard_armed.set(false);
                        notice.set(None);
                        secret.set(Some(created));
                        request_keys(&mut key_request, keys, None, PageDirection::Replace);
                    }
                    Err(failure) if failure.is_reauth_required() => {
                        // Confirm the password first; issuance is retried by the
                        // operator, never by this code.
                        refused.set(Some(RefusedMutation::IssueKey));
                    }
                    Err(failure) if failure.code == "secret_already_issued" => {
                        operation.set(None);
                        let key_hint = failure
                            .key_id
                            .as_deref()
                            .map(sanitize_note)
                            .filter(|id| !id.is_empty());
                        error.set(Some(match key_hint {
                            Some(key_id) => format!(
                                "A secret was already issued for key {key_id}. Revoke that key, then issue a new one."
                            ),
                            None => failure.user_message().to_owned(),
                        }));
                        request_keys(&mut key_request, keys, None, PageDirection::Replace);
                    }
                    Err(failure) => {
                        if !keeps_operation_id(&failure) {
                            operation.set(None);
                        }
                        if failure.code == "conflict" {
                            // The client moved under us: reload before the
                            // operator tries again.
                            bump_generation(&mut client_generation);
                            client_resource.restart();
                        }
                        error.set(Some(failure.user_message().to_owned()));
                    }
                }
            });
        }
    };

    let revoke_key = {
        let account_id = account_id.clone();
        move |_| {
            let Some(target) = revoke_target.peek().clone() else {
                return;
            };
            if *pending.peek() {
                return;
            }
            let Some(api) = session.peek().mutation_client() else {
                error.set(Some(
                    "Your session ended. Sign in again to continue.".to_owned(),
                ));
                return;
            };
            pending.set(true);
            error.set(None);
            let id = account_id.clone();
            let key_id = target.id.clone();
            spawn(async move {
                match api.revoke_key(&id, &key_id).await {
                    Ok(()) => {
                        pending.set(false);
                        revoke_target.set(None);
                        notice.set(Some("Key revoked.".to_owned()));
                        request_keys(&mut key_request, keys, None, PageDirection::Replace);
                    }
                    Err(failure) => {
                        pending.set(false);
                        revoke_target.set(None);
                        if failure.ends_session() {
                            session
                                .write()
                                .mark_ended("Your session ended. Sign in again.".to_owned());
                        } else {
                            error.set(Some(failure.user_message().to_owned()));
                            request_keys(&mut key_request, keys, None, PageDirection::Replace);
                        }
                    }
                }
            });
        }
    };

    let mut run_state_change = {
        let account_id = account_id.clone();
        move |action: ClientStateAction| {
            if *pending.peek() {
                return;
            }
            let Some(api) = session.peek().mutation_client() else {
                error.set(Some(
                    "Your session ended. Sign in again to continue.".to_owned(),
                ));
                return;
            };
            let Some(version) = client.peek().as_ref().map(|view| view.version) else {
                error.set(Some(
                    "Reload the client before changing its state.".to_owned(),
                ));
                return;
            };
            pending.set(true);
            error.set(None);
            notice.set(None);
            confirm_action.set(None);
            let id = account_id.clone();
            spawn(async move {
                match api.set_state(&id, version, action).await {
                    Ok(()) => {
                        pending.set(false);
                        notice.set(Some(match action {
                            ClientStateAction::Suspend => "Client suspended.".to_owned(),
                            ClientStateAction::Resume => "Client resumed.".to_owned(),
                        }));
                        bump_generation(&mut client_generation);
                        client_resource.restart();
                        request_keys(&mut key_request, keys, None, PageDirection::Replace);
                    }
                    Err(failure) if failure.is_reauth_required() => {
                        pending.set(false);
                        refused.set(Some(match action {
                            ClientStateAction::Suspend => RefusedMutation::Suspend,
                            ClientStateAction::Resume => RefusedMutation::Resume,
                        }));
                    }
                    Err(failure) => {
                        pending.set(false);
                        if failure.code == "conflict" {
                            // Stale expected_version: show the current state
                            // instead of retrying with a guessed version.
                            bump_generation(&mut client_generation);
                            client_resource.restart();
                            request_keys(&mut key_request, keys, None, PageDirection::Replace);
                        }
                        error.set(Some(failure.user_message().to_owned()));
                    }
                }
            });
        }
    };

    let confirmed_reauth = {
        let account_id = account_id.clone();
        move |_: ()| {
            let Some(action) = *refused.peek() else {
                return;
            };
            refused.set(None);
            pending.set(true);
            let id = account_id.clone();
            spawn(async move {
                // Reauthentication rotated the session, so the new CSRF token
                // has to be read before anything else is attempted — including a
                // later key issuance, which reuses the same operation id.
                let response = match AdminApi::new().session().await {
                    Ok(response) => response,
                    Err(failure) => {
                        pending.set(false);
                        error.set(Some(failure.user_message().to_owned()));
                        return;
                    }
                };
                let api = AdminApi::new()
                    .with_session_csrf(SessionCsrf::new(response.csrf_token.clone()));
                session.write().adopt(response);
                let Some(state_action) = action.state_action() else {
                    // Key issuance is never retried automatically: the operator
                    // presses the button again, reusing the operation id.
                    pending.set(false);
                    notice.set(Some(
                        "Password confirmed. Press “Issue key” again to send that same request."
                            .to_owned(),
                    ));
                    return;
                };
                let version = client.peek().as_ref().map(|view| view.version).unwrap_or(0);
                match api.set_state(&id, version, state_action).await {
                    Ok(()) => {
                        pending.set(false);
                        notice.set(Some(match state_action {
                            ClientStateAction::Suspend => "Client suspended.".to_owned(),
                            ClientStateAction::Resume => "Client resumed.".to_owned(),
                        }));
                        bump_generation(&mut client_generation);
                        client_resource.restart();
                        request_keys(&mut key_request, keys, None, PageDirection::Replace);
                    }
                    Err(failure) => {
                        pending.set(false);
                        error.set(Some(failure.user_message().to_owned()));
                    }
                }
            });
        }
    };

    let cancel_confirm = move |_| confirm_action.set(None);
    let refused_reason = (*refused.read())
        .map(RefusedMutation::reason)
        .unwrap_or_default();
    let key_next = move |_| {
        let cursor = keys.peek().next_cursor();
        request_keys(&mut key_request, keys, cursor, PageDirection::Next);
    };
    let modal_open = secret.read().is_some() || refused.read().is_some();
    let key_previous = move |_| {
        if let Some(cursor) = keys.peek().previous_cursor() {
            request_keys(&mut key_request, keys, cursor, PageDirection::Previous);
        }
    };
    let retry_keys = move |_| request_keys(&mut key_request, keys, None, PageDirection::Replace);

    rsx! {
        div { class: "container",
            div { class: "page-surface", inert: modal_open,
                h1 {
                if let Some(view) = client.read().as_ref() {
                    "{view.display_name}"
                } else {
                    "Client detail"
                }
            }
            // The way out of this page belongs above the fold, not only after a
            // long key table.
            div { class: "actions",
                button { r#type: "button", onclick: move |_| leave_page(), "Back to clients" }
            }
            AdminSessionBar { session, on_sign_out: sign_out }
            if session.read().has_ended() {
                div { class: "error", role: "alert",
                    p {
                        if let Some(message) = session.read().error() {
                            "{message}"
                        } else {
                            "Your session ended. Sign in again to continue."
                        }
                    }
                    Link { class: "button", to: Route::Login {}, "Go to sign-in" }
                }
            } else if let Some(message) = session.read().error() {
                p { class: "error", role: "alert", "{message}" }
            }
            p { class: "privilege-note",
                "Administrators can issue keys that access this client's memory. Issuance is audited."
            }
            if let Some(message) = error.read().as_ref() {
                p { class: "error", role: "alert", "aria-live": "assertive", "{message}" }
            }
            p { class: "status", role: "status", "aria-live": "polite",
                if let Some(message) = notice.read().as_ref() {
                    "{message}"
                } else if *pending.read() {
                    "Working…"
                }
            }
            if let Some(view) = client.read().as_ref() {
                section { class: "client-metadata",
                    h2 { "Client" }
                    table {
                        caption { class: "visually-hidden", "Client metadata" }
                        tbody {
                            tr { th { scope: "row", "Client id" } td { code { "{view.account_id}" } } }
                            tr { th { scope: "row", "Tenant id" } td { code { "{view.tenant_id}" } } }
                            tr {
                                th { scope: "row", "Account status" }
                                td {
                                    span {
                                        class: "{status_badge_class(&view.account_status)}",
                                        "{status_label(&view.account_status)}"
                                    }
                                }
                            }
                            tr {
                                th { scope: "row", "Tenant status" }
                                td {
                                    span {
                                        class: "{status_badge_class(&view.tenant_status)}",
                                        "{status_label(&view.tenant_status)}"
                                    }
                                }
                            }
                            tr { th { scope: "row", "Plan version" } td { "{view.plan_version}" } }
                            tr { th { scope: "row", "Schema version" } td { "{view.schema_version}" } }
                            tr { th { scope: "row", "Version" } td { "{view.version}" } }
                        }
                    }
                    p { class: "readiness", "{readiness(&view)}" }
                    if view.is_failed() {
                        p { class: "error", role: "alert",
                            "Provisioning failed: {view.safe_provisioning_reason().unwrap_or_else(|| \"no reason reported\".to_owned())}"
                        }
                    }
                }
                section { class: "client-state",
                    h2 { "State" }
                    if let Some(action) = *confirm_action.read() {
                        div {
                            class: "confirm",
                            role: "group",
                            "aria-labelledby": "client-state-question",
                            p { id: "client-state-question",
                                match action {
                                    ClientStateAction::Suspend => "Suspend this client? Every request made with its keys is rejected until it is resumed.",
                                    ClientStateAction::Resume => "Resume this client? Its existing keys become usable again.",
                                }
                            }
                            button {
                                r#type: "button",
                                autofocus: true,
                                disabled: *pending.read(),
                                onclick: move |_| run_state_change(action),
                                "Confirm"
                            }
                            button { r#type: "button", onclick: cancel_confirm, "Cancel" }
                        }
                    } else {
                        if view.can_suspend() {
                            button {
                                r#type: "button",
                                disabled: *pending.read(),
                                onclick: move |_| confirm_action.set(Some(ClientStateAction::Suspend)),
                                "Suspend client"
                            }
                        } else if view.can_resume() {
                            button {
                                r#type: "button",
                                disabled: *pending.read(),
                                onclick: move |_| confirm_action.set(Some(ClientStateAction::Resume)),
                                "Resume client"
                            }
                        } else {
                            p { class: "hint",
                                "Suspend and resume are offered only for a ready client, or for a client this workflow suspended."
                            }
                        }
                    }
                }
                section { class: "keys",
                    h2 { "API keys" }
                    p { class: "hint", "Expired and revoked keys no longer authenticate." }
                    if view.can_issue_keys() && session.read().is_ready() {
                        form { class: "issue-key", onsubmit: issue_key,
                            div { class: "field",
                                label { r#for: "key-name", "Key name" }
                                input {
                                    id: "key-name",
                                    name: "key-name",
                                    r#type: "text",
                                    autocomplete: "off",
                                    required: true,
                                    value: "{key_name}",
                                    oninput: move |event| key_name.set(event.value()),
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
                                        min: "1",
                                        max: "3650",
                                        step: "1",
                                        value: "{key_days}",
                                        oninput: move |event| key_days.set(event.value()),
                                    }
                                    span { " days (1–3650)" }
                                }
                                div {
                                    label { r#for: "key-expiry-never", "Never expires" }
                                    input {
                                        id: "key-expiry-never",
                                        name: "expiry-never",
                                        r#type: "checkbox",
                                        checked: *key_never.read(),
                                        onchange: move |event| key_never.set(event.checked()),
                                    }
                                }
                                p { class: "hint", "Choose exactly one. There is no default." }
                            }
                            button { r#type: "submit", disabled: *pending.read(),
                                if *pending.read() { "Issuing…" } else { "Issue key" }
                            }
                            if operation.read().is_some() {
                                p { class: "hint",
                                    "This request already has an operation id. Retrying reuses it, so a lost response cannot issue a second key."
                                }
                            }
                        }
                    } else if !session.read().is_ready() {
                        p { class: "hint", "Sign in again before issuing keys for this client." }
                    } else {
                        p { class: "hint",
                            "Keys can be issued only while the client is ready for data access."
                        }
                    }

                    if let Some(message) = keys.read().error() {
                        p { class: "error", role: "alert", "{message}" }
                        button {
                            r#type: "button",
                            onclick: retry_keys,
                            disabled: keys.read().is_loading(),
                            "Retry keys"
                        }
                    }
                    if keys.read().is_loading() {
                        p { class: "status", role: "status", "aria-live": "polite", "Loading keys…" }
                    } else if !keys.read().is_loaded() {
                        p { class: "hint", "No page of keys has loaded yet." }
                    } else if keys.read().items().is_empty() {
                        p { class: "empty", "No keys have been issued for this client." }
                    } else {
                        div {
                            class: "table-scroll",
                            role: "region",
                            "aria-label": "API keys table",
                            tabindex: "0",
                            table {
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
                                    for key in keys.read().items().to_vec() {
                                        tr { key: "{key.id}",
                                            td { "{key.name}" }
                                            td {
                                                span {
                                                    class: "{status_badge_class(key.display_status(*now_millis.read()).label())}",
                                                    "{status_label(key.display_status(*now_millis.read()).label())}"
                                                }
                                            }
                                            td {
                                                time {
                                                    class: "timestamp",
                                                    datetime: "{key.created_at}",
                                                    title: "{key.created_at}",
                                                    "{compact_timestamp(&key.created_at)}"
                                                }
                                            }
                                            td {
                                                if let Some(value) = key.expires_at.as_deref() {
                                                    time {
                                                        class: "timestamp",
                                                        datetime: "{value}",
                                                        title: "{value}",
                                                        "{compact_timestamp(value)}"
                                                    }
                                                } else {
                                                    "Never"
                                                }
                                            }
                                            td {
                                                if let Some(value) = key.last_used_at.as_deref() {
                                                    time {
                                                        class: "timestamp",
                                                        datetime: "{value}",
                                                        title: "{value}",
                                                        "{compact_timestamp(value)}"
                                                    }
                                                } else {
                                                    "Not used"
                                                }
                                            }
                                            td {
                                                if key.display_status(*now_millis.read()) == KeyDisplayStatus::Revoked {
                                                    span { "revoked" }
                                                } else {
                                                    button {
                                                        r#type: "button",
                                                        disabled: *pending.read(),
                                                        onclick: {
                                                            let target = key.clone();
                                                            move |_| revoke_target.set(Some(target.clone()))
                                                        },
                                                        "Revoke…"
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        div { class: "actions",
                            button {
                                r#type: "button",
                                onclick: key_previous,
                                disabled: !keys.read().has_previous() || keys.read().is_loading(),
                                "Previous keys"
                            }
                            button {
                                r#type: "button",
                                onclick: key_next,
                                disabled: !keys.read().has_next() || keys.read().is_loading(),
                                "Next keys"
                            }
                        }
                    }
                    if let Some(target) = revoke_target.read().as_ref() {
                        div {
                            class: "confirm",
                            role: "group",
                            "aria-labelledby": "revoke-key-question",
                            p { id: "revoke-key-question",
                                "Revoke key {target.name} ({target.id})? Requests already using it stop working."
                            }
                            button {
                                r#type: "button",
                                autofocus: true,
                                disabled: *pending.read(),
                                onclick: revoke_key,
                                "Confirm revoke"
                            }
                            button { r#type: "button", onclick: move |_| revoke_target.set(None), "Cancel" }
                        }
                    }
                }
            } else if session.read().is_loading() {
                p { class: "status", role: "status", "aria-live": "polite", "Loading client…" }
            } else {
                p { class: "hint", "The client could not be loaded." }
                button { r#type: "button", onclick: move |_| refresh(), "Try again" }
            }
                if secret.read().is_some() {
                    div { class: "actions",
                        p { "A key secret is still on screen and will be lost when you leave." }
                    }
                }
            }
            if let Some(created) = secret.read().as_ref() {
                dialog {
                    class: "modal-layer",
                    open: true,
                    role: "alertdialog",
                    "aria-modal": "true",
                    "aria-labelledby": "new-key-secret-title",
                    "aria-describedby": "new-key-secret-warning",
                    tabindex: "-1",
                    onkeydown: escape_secret,
                    div { class: "secret",
                        h3 { id: "new-key-secret-title", "New key secret" }
                        p { id: "new-key-secret-warning", class: "warning",
                            "Save this now; it cannot be shown again. Deliver it outside this service."
                        }
                        code { class: "secret-value", "{created.secret}" }
                        p { "Key {created.name} ({created.id})" }
                        button { r#type: "button", autofocus: true, onclick: copy_secret, "Copy secret" }
                        if *discard_armed.read() {
                            p { role: "alert",
                                "The secret will be lost when this panel closes. It cannot be shown again."
                            }
                            button { r#type: "button", onclick: close_secret, "Discard the secret" }
                            button { r#type: "button", onclick: keep_secret, "Keep it" }
                        } else {
                            button { r#type: "button", onclick: close_secret, "Close" }
                        }
                    }
                }
            }
            {if refused.read().is_some() {
                rsx! {
                    AdminReauthDialog {
                        csrf: session.read().csrf(),
                        reason: refused_reason,
                        on_confirmed: confirmed_reauth,
                        on_cancel: move |_| refused.set(None),
                    }
                }
            } else {
                rsx! {}
            }}
        }
    }
}

/// Short readiness summary for the client header.
fn readiness(client: &ClientView) -> String {
    if client.is_provisioning() {
        "Provisioning. This page refreshes every 2 seconds.".to_owned()
    } else if client.is_failed() {
        "Provisioning failed. Keys cannot be issued.".to_owned()
    } else if client.is_ready() {
        "Ready for data access; keys can be issued.".to_owned()
    } else if client.is_suspended() {
        "Suspended by this workflow; resume to restore access.".to_owned()
    } else {
        format!("State: {}.", status_label(client.state_label()))
    }
}

/// Record a failure: session-wide problems end the page's session, everything
/// else is shown in place.
fn record_failure(
    mut session: Signal<AdminSession>,
    mut error: Signal<Option<String>>,
    failure: crate::admin_api::AdminApiError,
) {
    if failure.ends_session() {
        session
            .write()
            .mark_ended(failure.user_message().to_owned());
    } else {
        error.set(Some(failure.user_message().to_owned()));
    }
}

fn bump_generation(generation: &mut Signal<u64>) {
    let next = {
        let current = *generation.peek();
        next_generation(current)
    };
    generation.set(next);
}

/// Request one page of key metadata. The generation makes a pagination or
/// refresh action restart the resource even when the cursor is unchanged.
fn request_keys(
    request: &mut Signal<KeyPageRequest>,
    mut keys: Signal<Paged<ApiKeyMeta>>,
    cursor: Option<String>,
    direction: PageDirection,
) {
    keys.write().begin();
    let generation = {
        let current = request.peek().generation;
        next_generation(current)
    };
    request.set(KeyPageRequest {
        cursor,
        direction,
        generation,
    });
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
    fn refused_mutations_map_to_their_state_action() {
        assert_eq!(
            RefusedMutation::Suspend.state_action(),
            Some(ClientStateAction::Suspend)
        );
        assert_eq!(
            RefusedMutation::Resume.state_action(),
            Some(ClientStateAction::Resume)
        );
        assert_eq!(RefusedMutation::IssueKey.state_action(), None);
    }
}
