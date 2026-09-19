//! Login page — the flow this deployment serves.
//!
//! `GET /api/v1/auth/config` decides: local mode renders the administrator
//! username/password form, OIDC mode keeps the existing provider redirect. An
//! unknown mode renders neither, so a misconfigured server cannot fall back to
//! a flow it does not actually offer.

use dioxus::prelude::*;

use crate::admin_api::{AdminApi, AuthConfig};
use crate::pages::admin_auth::AdminLoginForm;

#[component]
pub fn LoginPage() -> Element {
    let config = use_signal(|| None::<AuthConfig>);
    let error = use_signal(|| None::<String>);
    let loading = use_signal(|| true);

    use_effect(move || load_mode(config, loading, error));

    let retry = move |_| load_mode(config, loading, error);

    rsx! {
        div { class: "container",
            if *loading.read() {
                p { class: "status", role: "status", "aria-live": "polite", "Checking how to sign in…" }
            } else if let Some(message) = error.read().as_ref() {
                h1 { "Sign in" }
                p { class: "error", role: "alert", "aria-live": "assertive", "{message}" }
                button { r#type: "button", onclick: retry, "Try again" }
            } else if let Some(mode) = config.read().as_ref() {
                if mode.is_local() {
                    h1 { "Sign in" }
                    AdminLoginForm {}
                } else if mode.is_oidc() {
                    h1 { "Login" }
                    p { "You will be redirected to your identity provider." }
                    a { href: "/api/v1/auth/authorize", button { "Login with OIDC" } }
                } else {
                    h1 { "Sign in" }
                    p { class: "error", role: "alert",
                        "This deployment reported a sign-in mode this page does not recognise. Ask an operator to check the server configuration."
                    }
                }
            }
        }
    }
}

/// Read the public auth mode, keeping the page in a loading or error state
/// rather than guessing.
fn load_mode(
    mut config: Signal<Option<AuthConfig>>,
    mut loading: Signal<bool>,
    mut error: Signal<Option<String>>,
) {
    loading.set(true);
    error.set(None);
    spawn(async move {
        match AdminApi::new().mode().await {
            Ok(value) => {
                config.set(Some(value));
                loading.set(false);
            }
            Err(failure) => {
                error.set(Some(failure.user_message().to_owned()));
                loading.set(false);
            }
        }
    });
}
