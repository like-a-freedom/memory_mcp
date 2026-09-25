//! The two-phase challenge flow behind `/admin/activate` and `/admin/reset`.
//!
//! The code is validated without consuming it, and the confirmed username is
//! shown read-only. The password is only sent once the operator submits the
//! second form; the backend enforces the real policy. The two phases are
//! separate components because only one of them is mounted at a time and they
//! share no state beyond the validated code and the account it resolved to.

use dioxus::prelude::*;
use dioxus_router::Link;
use dioxus_router::hooks::use_navigator;

use crate::admin_api::{AdminApi, ChallengeKind, ChallengeResponse, sleep_ms};
use crate::components::alert::{Alert, AlertTone};
use crate::components::timestamp::Timestamp;
use crate::routes::Route;

/// How long the success panel stays up before the sign-in redirect.
const SUCCESS_REDIRECT_MS: u32 = 1_200;

/// Both phases of the flow, with the code and the account in between them.
#[component]
pub(super) fn ChallengeFinishFlow(kind: ChallengeKind) -> Element {
    let navigator = use_navigator();
    let mut code = use_signal(String::new);
    let mut challenge = use_signal(|| None::<ChallengeResponse>);
    let mut done = use_signal(|| false);

    let saved = move |_| {
        // The code is spent and the password was dropped by the form.
        code.set(String::new());
        challenge.set(None);
        done.set(true);
        spawn(async move {
            sleep_ms(SUCCESS_REDIRECT_MS).await;
            navigator.replace(Route::Login {});
        });
    };

    rsx! {
        if *done.read() {
            div { class: "success", role: "status", "aria-live": "polite",
                p { "Password saved. Redirecting to sign-in…" }
                Link { to: Route::Login {}, "Go to sign-in" }
            }
        } else if let Some(view) = challenge.read().as_ref() {
            ChallengePasswordForm {
                kind,
                code: code.read().clone(),
                challenge: view.clone(),
                on_saved: saved,
            }
        } else {
            ChallengeCodeForm {
                kind,
                on_validated: move |(pasted, view): (String, ChallengeResponse)| {
                    code.set(pasted);
                    challenge.set(Some(view));
                },
            }
        }
    }
}

/// First phase: check a one-time code without consuming it.
#[component]
fn ChallengeCodeForm(
    kind: ChallengeKind,
    /// The code, and the account it resolved to, once the backend accepted it.
    on_validated: EventHandler<(String, ChallengeResponse)>,
) -> Element {
    let mut code = use_signal(String::new);
    let mut pending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

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
        pending.set(true);
        error.set(None);
        spawn(async move {
            // Validation does not consume the code and is not retried
            // automatically.
            let outcome = AdminApi::new().inspect(&pasted, kind).await;
            pending.set(false);
            match outcome {
                Ok(view) => on_validated.call((pasted, view)),
                Err(failure) => error.set(Some(failure.challenge_message().to_owned())),
            }
        });
    };

    rsx! {
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
            Alert { tone: AlertTone::Error, message: error.read().clone() }
            button { r#type: "submit", disabled: *pending.read(),
                if *pending.read() { "Checking…" } else { "Check code" }
            }
            Alert {
                tone: AlertTone::Status,
                message: pending.read().then(|| "Checking…".to_owned()),
            }
        }
    }
}

/// Second phase: choose the password for the account the code resolved to.
#[component]
fn ChallengePasswordForm(
    kind: ChallengeKind,
    /// The code, already validated, that authorises this change.
    code: String,
    /// The account the code resolves to, and when it stops being valid.
    challenge: ChallengeResponse,
    on_saved: EventHandler<()>,
) -> Element {
    let mut password = use_signal(String::new);
    let mut confirmation = use_signal(String::new);
    let mut pending = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

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
        // Cloned into the task: the form stays mounted, so the closure must not
        // consume the code it was given.
        let code = code.clone();
        spawn(async move {
            // The password is never retried automatically, and the real policy
            // is enforced by the backend.
            let api = AdminApi::new();
            let outcome = match kind {
                ChallengeKind::Activate => api.activate(&code, &secret).await,
                ChallengeKind::Reset => api.reset(&code, &secret).await,
            };
            // Drop password material as soon as the attempt returns. The code is
            // kept by the parent so a rejected password can be corrected without
            // asking an operator for a new one.
            password.set(String::new());
            confirmation.set(String::new());
            pending.set(false);
            match outcome {
                Ok(()) => on_saved.call(()),
                Err(failure) => error.set(Some(failure.challenge_message().to_owned())),
            }
        });
    };

    rsx! {
        form { class: "challenge-finish", onsubmit: finish,
            div { class: "field",
                label { r#for: "challenge-username", "Account" }
                input {
                    id: "challenge-username",
                    r#type: "text",
                    readonly: true,
                    value: "{challenge.username}",
                }
            }
            p { class: "hint",
                "This code expires at "
                Timestamp { value: challenge.expires_at.clone() }
                "."
            }
            div { class: "field",
                label { r#for: "challenge-password", "New password" }
                input {
                    id: "challenge-password",
                    name: "new-password",
                    r#type: "password",
                    autocomplete: "new-password",
                    required: true,
                    "aria-describedby": "challenge-password-hint",
                    value: "{password}",
                    oninput: move |event| password.set(event.value()),
                }
                // The backend enforces this range; stating it here is the
                // difference between a correctable typo and a round trip spent
                // guessing.
                p { id: "challenge-password-hint", class: "hint",
                    "15 to 128 characters."
                }
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
            Alert { tone: AlertTone::Error, message: error.read().clone() }
            button { r#type: "submit", disabled: *pending.read(),
                if *pending.read() { "Saving…" } else { "Save password" }
            }
            Alert {
                tone: AlertTone::Status,
                message: pending.read().then(|| "Saving…".to_owned()),
            }
        }
    }
}
