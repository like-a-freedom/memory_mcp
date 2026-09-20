//! Login page — the flow this deployment serves.
//!
//! `GET /api/v1/auth/config` decides: local mode renders the administrator
//! username/password form, OIDC mode keeps the existing provider redirect. An
//! unknown mode renders neither, so a misconfigured server cannot fall back to
//! a flow it does not actually offer.

use dioxus::prelude::*;

use crate::admin_api::{AdminApi, PATH_OIDC_AUTHORIZE};
use crate::pages::admin_auth::AdminLoginForm;

#[component]
pub fn LoginPage() -> Element {
    // This is a client-side read with an explicit retry action, so the resource
    // owns loading, cancellation, and the latest result without a separate
    // effect that mutates its own state.
    let mut config = use_resource(|| async { AdminApi::new().mode().await });
    let retry = move |_| config.restart();

    rsx! {
        div { class: "container",
            match config.read().as_ref() {
                None => rsx! {
                    p { class: "status", role: "status", "aria-live": "polite", "Checking how to sign in…" }
                },
                Some(Err(failure)) => rsx! {
                    h1 { "Sign in" }
                    p { class: "error", role: "alert", "aria-live": "assertive", "{failure.user_message()}" }
                    button { r#type: "button", onclick: retry, "Try again" }
                },
                Some(Ok(mode)) if mode.is_local() => rsx! {
                    h1 { "Sign in" }
                    AdminLoginForm {}
                },
                Some(Ok(mode)) if mode.is_oidc() => rsx! {
                    h1 { "Sign in" }
                    p { "You will be redirected to your identity provider." }
                    // An anchor, not a button nested inside one: this leaves the
                    // SPA for the provider, and nesting interactive content is
                    // invalid and confuses assistive technology.
                    a { class: "button", href: PATH_OIDC_AUTHORIZE, "Sign in with OIDC" }
                },
                Some(Ok(_)) => rsx! {
                    h1 { "Sign in" }
                    p { class: "error", role: "alert",
                        "This deployment reported a sign-in mode this page does not recognise. Ask an operator to check the server configuration."
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The OIDC button must point at the route the server actually mounts.
    /// Guessing the `/api/v1` prefix (as an earlier revision did) produces a
    /// link that 404s for every OIDC deployment.
    #[test]
    fn the_oidc_button_targets_the_mounted_authorize_route() {
        assert_eq!(PATH_OIDC_AUTHORIZE, "/auth/oidc/authorize");
        assert!(
            !PATH_OIDC_AUTHORIZE.starts_with("/api/v1"),
            "the OIDC flow is mounted outside the API prefix"
        );
    }
}
