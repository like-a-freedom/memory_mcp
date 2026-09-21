//! Authentication widgets shared by the sign-in route and the console routes.
//!
//! These live in `components/` rather than next to a page because each has two
//! consumers: the sign-in form is rendered by `/login` (when the deployment
//! reports local mode) and by `/admin/login`, and the re-authentication dialog
//! is rendered by `/admin/reauth` and by the client detail page when a mutation
//! is refused with `reauth_required`.
//!
//! Credential material — passwords and the session CSRF token — is held in
//! component signals only, and dropped when the component unmounts. Failures are
//! reported with the fixed copy from `AdminApiError`: backend `message` text is
//! never rendered, so the forms cannot leak internals.

use dioxus::prelude::*;
use dioxus_router::hooks::use_navigator;

use crate::admin_api::{AdminApi, SessionCsrf};
use crate::components::alert::{Alert, AlertTone};
use crate::components::modal::{Modal, ModalKind};
use crate::routes::Route;

/// Local username/password sign-in form.
#[component]
pub fn AdminLoginForm() -> Element {
    let navigator = use_navigator();
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut pending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let submit = move |event: FormEvent| {
        event.prevent_default();
        if *pending.peek() {
            return;
        }
        let name = username.read().trim().to_owned();
        let secret = password.read().clone();
        if name.is_empty() || secret.is_empty() {
            error.set(Some("Enter your username and password.".to_owned()));
            return;
        }
        pending.set(true);
        error.set(None);
        spawn(async move {
            // One deliberate attempt: a rejected credential POST is never
            // retried automatically.
            let outcome = AdminApi::new().login(&name, &secret).await;
            // The password is dropped as soon as the attempt returns.
            password.set(String::new());
            pending.set(false);
            match outcome {
                Ok(()) => {
                    navigator.replace(Route::AdminClientList {});
                }
                Err(failure) => error.set(Some(failure.login_message().to_owned())),
            }
        });
    };

    rsx! {
        form { class: "admin-login", onsubmit: submit,
            p { "Sign in with the administrator account created with the CLI." }
            div { class: "field",
                label { r#for: "admin-username", "Username" }
                input {
                    id: "admin-username",
                    name: "username",
                    r#type: "text",
                    autocomplete: "username",
                    "autocapitalize": "none",
                    spellcheck: "false",
                    required: true,
                    value: "{username}",
                    oninput: move |event| username.set(event.value()),
                }
            }
            div { class: "field",
                label { r#for: "admin-password", "Password" }
                input {
                    id: "admin-password",
                    name: "password",
                    r#type: "password",
                    autocomplete: "current-password",
                    required: true,
                    value: "{password}",
                    oninput: move |event| password.set(event.value()),
                }
            }
            Alert { tone: AlertTone::Error, message: error.read().clone() }
            button { r#type: "submit", disabled: *pending.read(),
                if *pending.read() { "Signing in…" } else { "Sign in" }
            }
            Alert {
                tone: AlertTone::Status,
                message: pending.read().then(|| "Signing in…".to_owned()),
            }
        }
    }
}

/// Re-authentication dialog.
///
/// The operator re-enters the current password after a `reauth_required`
/// refusal. Nothing is retried until that submission is accepted and
/// `on_confirmed` runs, and the dialog itself never retries the refused
/// operation.
#[component]
pub fn AdminReauthDialog(
    #[props(default)] csrf: Option<SessionCsrf>,
    /// What the refused operation was, for the sentence that explains the panel.
    #[props(default = String::new())]
    reason: String,
    on_confirmed: EventHandler<()>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut password = use_signal(String::new);
    let mut pending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let confirm = move |event: FormEvent| {
        event.prevent_default();
        if *pending.peek() {
            return;
        }
        let secret = password.read().clone();
        if secret.is_empty() {
            error.set(Some("Enter your password.".to_owned()));
            return;
        }
        let Some(token) = csrf.clone() else {
            error.set(Some("Your session ended. Sign in again.".to_owned()));
            return;
        };
        pending.set(true);
        error.set(None);
        spawn(async move {
            // A single explicit attempt: the refused operation is only retried
            // by the caller after this call succeeds.
            let outcome = AdminApi::new()
                .with_session_csrf(token)
                .reauth(&secret)
                .await;
            password.set(String::new());
            pending.set(false);
            match outcome {
                Ok(()) => on_confirmed.call(()),
                Err(failure) => error.set(Some(failure.user_message().to_owned())),
            }
        });
    };

    let description = if reason.is_empty() {
        "Recent authentication is required for this action.".to_owned()
    } else {
        format!("Recent authentication is required to {reason}.")
    };

    rsx! {
        Modal {
            id: "reauth",
            kind: ModalKind::Question,
            title: "Confirm your password",
            description,
            on_dismiss: move |_| on_cancel.call(()),
            p { class: "hint", "Nothing is retried until you confirm." }
            form { onsubmit: confirm,
                div { class: "field",
                    label { r#for: "reauth-password", "Password" }
                    input {
                        id: "reauth-password",
                        name: "password",
                        r#type: "password",
                        autocomplete: "current-password",
                        autofocus: true,
                        required: true,
                        value: "{password}",
                        oninput: move |event| password.set(event.value()),
                    }
                }
                Alert { tone: AlertTone::Error, message: error.read().clone() }
                button { r#type: "submit", disabled: *pending.read(),
                    if *pending.read() { "Confirming…" } else { "Confirm" }
                }
                button { r#type: "button", onclick: move |_| on_cancel.call(()), "Cancel" }
            }
        }
    }
}
