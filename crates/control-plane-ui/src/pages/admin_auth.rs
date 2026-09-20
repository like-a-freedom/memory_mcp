//! Local administrator authentication pages.
//!
//! Credential material — one-time codes, passwords and the session CSRF token
//! — is held in component signals only. It is never written to `localStorage`,
//! `sessionStorage` or `IndexedDB`, never placed in a URL, query or fragment,
//! and is dropped when the component unmounts, because nothing here outlives
//! the scope that owns the signal.
//!
//! Failures are reported with the fixed copy from `AdminApiError`: backend
//! `message` text is never rendered, so the forms cannot leak internals.

use dioxus::prelude::*;
use dioxus_router::hooks::use_navigator;

use crate::admin_api::{AdminApi, ChallengeKind, ChallengeResponse, SessionCsrf, sleep_ms};
use crate::pages::admin_session::use_admin_session;
use crate::router::Route;

/// How long the success panel stays up before the sign-in redirect.
const SUCCESS_REDIRECT_MS: u32 = 1_200;

/// `/admin/login` — administrator sign-in for local mode.
#[component]
pub fn AdminLoginPage() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Administrator sign-in" }
            AdminLoginForm {}
        }
    }
}

/// Local username/password sign-in form.
///
/// Rendered by `/login` when `GET /api/v1/auth/config` reports local mode, and
/// by `/admin/login` on direct navigation.
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
            div {
                class: "error",
                role: "alert",
                "aria-live": "assertive",
                if let Some(message) = error.read().as_ref() {
                    "{message}"
                }
            }
            button { r#type: "submit", disabled: *pending.read(),
                if *pending.read() { "Signing in…" } else { "Sign in" }
            }
            p { class: "status", role: "status", "aria-live": "polite",
                if *pending.read() { "Signing in…" }
            }
        }
    }
}

/// `/admin/activate` — set the first administrator password.
#[component]
pub fn AdminActivationPage() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Set administrator password" }
            ChallengeFinishFlow { kind: ChallengeKind::Activate }
        }
    }
}

/// `/admin/reset` — replace an administrator password.
#[component]
pub fn AdminResetPage() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Reset administrator password" }
            ChallengeFinishFlow { kind: ChallengeKind::Reset }
        }
    }
}

/// Two-phase challenge flow: paste a code, validate it, then choose a password.
///
/// The code is validated without consuming it, and the confirmed username is
/// shown read-only. The password is only sent once the operator submits the
/// second form; the backend enforces the real policy.
#[component]
fn ChallengeFinishFlow(kind: ChallengeKind) -> Element {
    let navigator = use_navigator();
    let mut code = use_signal(String::new);
    let mut challenge = use_signal(|| None::<ChallengeResponse>);
    let mut password = use_signal(String::new);
    let mut confirmation = use_signal(String::new);
    let mut pending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut done = use_signal(|| false);
    let login_route = Route::Login {}.to_string();

    let inspect = move |event: FormEvent| {
        event.prevent_default();
        if *pending.peek() {
            return;
        }
        let pasted = code.read().trim().to_owned();
        if pasted.is_empty() {
            error.set(Some("Paste the one-time code you were given.".to_owned()));
            return;
        }
        code.set(pasted.clone());
        pending.set(true);
        error.set(None);
        spawn(async move {
            // Validation does not consume the code and is not retried
            // automatically.
            let outcome = AdminApi::new().inspect(&pasted, kind).await;
            pending.set(false);
            match outcome {
                Ok(view) => challenge.set(Some(view)),
                Err(failure) => error.set(Some(failure.challenge_message().to_owned())),
            }
        });
    };

    let finish = move |event: FormEvent| {
        event.prevent_default();
        if *pending.peek() {
            return;
        }
        let secret = password.read().clone();
        let repeated = confirmation.read().clone();
        if secret.is_empty() {
            error.set(Some("Enter the new password.".to_owned()));
            return;
        }
        if secret != repeated {
            error.set(Some("The two passwords do not match.".to_owned()));
            return;
        }
        pending.set(true);
        error.set(None);
        let pasted = code.read().clone();
        spawn(async move {
            // The password is never retried automatically, and the real policy
            // is enforced by the backend.
            let api = AdminApi::new();
            let outcome = match kind {
                ChallengeKind::Activate => api.activate(&pasted, &secret).await,
                ChallengeKind::Reset => api.reset(&pasted, &secret).await,
            };
            // Drop password material as soon as the attempt returns. The code
            // is kept on failure so a rejected password can be corrected
            // without asking an operator for a new code.
            password.set(String::new());
            confirmation.set(String::new());
            pending.set(false);
            match outcome {
                Ok(()) => {
                    code.set(String::new());
                    challenge.set(None);
                    done.set(true);
                    spawn(async move {
                        let _ = sleep_ms(SUCCESS_REDIRECT_MS).await;
                        navigator.replace(Route::Login {});
                    });
                }
                Err(failure) => error.set(Some(failure.challenge_message().to_owned())),
            }
        });
    };

    rsx! {
        if *done.read() {
            div { class: "success", role: "status", "aria-live": "polite",
                p { "Password saved. Redirecting to sign-in…" }
                a { href: "{login_route}", "Go to sign-in" }
            }
        } else if let Some(view) = challenge.read().as_ref() {
            form { class: "challenge-finish", onsubmit: finish,
                div { class: "field",
                    label { r#for: "challenge-username", "Account" }
                    input {
                        id: "challenge-username",
                        r#type: "text",
                        readonly: true,
                        value: "{view.username}",
                    }
                }
                p { class: "hint", "This code expires at {view.expires_at}." }
                div { class: "field",
                    label { r#for: "challenge-password", "New password" }
                    input {
                        id: "challenge-password",
                        name: "new-password",
                        r#type: "password",
                        autocomplete: "new-password",
                        required: true,
                        value: "{password}",
                        oninput: move |event| password.set(event.value()),
                    }
                    // The backend enforces 15-128 characters; stating it here is
                    // the difference between a correctable typo and a round trip
                    // spent guessing.
                    p { class: "hint", "15 to 128 characters." }
                }
                div { class: "field",
                    label { r#for: "challenge-confirmation", "Repeat new password" }
                    input {
                        id: "challenge-confirmation",
                        name: "confirm-password",
                        r#type: "password",
                        autocomplete: "new-password",
                        required: true,
                        value: "{confirmation}",
                        oninput: move |event| confirmation.set(event.value()),
                    }
                }
                div { class: "error", role: "alert", "aria-live": "assertive",
                    if let Some(message) = error.read().as_ref() {
                        "{message}"
                    }
                }
                button { r#type: "submit", disabled: *pending.read(),
                    if *pending.read() { "Saving…" } else { "Save password" }
                }
                p { class: "status", role: "status", "aria-live": "polite",
                    if *pending.read() { "Saving…" }
                }
            }
        } else {
            form { class: "challenge-inspect", onsubmit: inspect,
                p { "Paste the one-time code issued by the CLI. The code is checked before it is used." }
                div { class: "field",
                    label { r#for: "challenge-code", "One-time code" }
                    input {
                        id: "challenge-code",
                        name: "one-time-code",
                        r#type: "text",
                        autocomplete: "one-time-code",
                        "autocapitalize": "none",
                        spellcheck: "false",
                        required: true,
                        value: "{code}",
                        oninput: move |event| code.set(event.value()),
                    }
                }
                div { class: "error", role: "alert", "aria-live": "assertive",
                    if let Some(message) = error.read().as_ref() {
                        "{message}"
                    }
                }
                button { r#type: "submit", disabled: *pending.read(),
                    if *pending.read() { "Checking…" } else { "Check code" }
                }
                p { class: "status", role: "status", "aria-live": "polite",
                    if *pending.read() { "Checking…" }
                }
            }
        }
    }
}

/// Re-authentication dialog.
///
/// The operator re-enters the current password after a `reauth_required`
/// refusal. Nothing is retried until that submission is accepted and
/// `on_confirmed` runs, and the dialog itself never retries a key issuance.
#[component]
pub fn AdminReauthDialog(
    #[props(default)] csrf: Option<SessionCsrf>,
    #[props(default = String::new())] reason: String,
    #[props(default)] on_confirmed: Option<EventHandler<()>>,
    #[props(default)] on_cancel: Option<EventHandler<()>>,
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
                Ok(()) => {
                    if let Some(callback) = on_confirmed {
                        callback.call(());
                    }
                }
                Err(failure) => error.set(Some(failure.user_message().to_owned())),
            }
        });
    };

    rsx! {
        div {
            class: "dialog",
            role: "dialog",
            "aria-labelledby": "reauth-title",
            "aria-describedby": "reauth-reason",
            h2 { id: "reauth-title", "Confirm your password" }
            p { id: "reauth-reason",
                if reason.is_empty() {
                    "Recent authentication is required for this action."
                } else {
                    "Recent authentication is required to {reason}."
                }
            }
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
                div { class: "error", role: "alert", "aria-live": "assertive",
                    if let Some(message) = error.read().as_ref() {
                        "{message}"
                    }
                }
                button { r#type: "submit", disabled: *pending.read(),
                    if *pending.read() { "Confirming…" } else { "Confirm" }
                }
                if let Some(cancel) = on_cancel {
                    button { r#type: "button", onclick: move |_| cancel.call(()), "Cancel" }
                }
            }
        }
    }
}

/// `/admin/reauth` — standalone confirmation page.
///
/// The dialog is normally rendered in place by the page that hit
/// `reauth_required`; this route exists so that a reload in the middle of that
/// flow still lands on a working form.
#[component]
pub fn AdminReauthPage() -> Element {
    let navigator = use_navigator();
    let session = use_admin_session();
    let csrf = session.read().csrf();
    let login_route = Route::Login {}.to_string();

    rsx! {
        div { class: "container",
            h1 { "Confirm your password" }
            if session.read().is_loading() {
                p { class: "status", role: "status", "aria-live": "polite", "Checking your session…" }
            } else if session.read().is_ready() {
                AdminReauthDialog {
                    csrf,
                    on_confirmed: move |_| {
                        navigator.replace(Route::AdminClientList {});
                    },
                }
            } else {
                div { class: "error", role: "alert",
                    p { "Your session ended. Sign in again." }
                    a { href: "{login_route}", "Go to sign-in" }
                }
            }
        }
    }
}
