//! The sign-in route, which serves whichever flow this deployment offers.
//!
//! `GET /api/v1/auth/config` decides: local mode renders the administrator
//! username/password form, OIDC mode keeps the provider redirect. An unknown
//! mode renders neither, so a misconfigured server cannot fall back to a flow it
//! does not actually offer.

use dioxus::prelude::*;

use crate::admin_api::{AdminApi, PATH_OIDC_AUTHORIZE};
use crate::components::admin_auth::AdminLoginForm;
use crate::components::alert::{Alert, AlertTone};

/// `/login`
#[component]
pub fn LoginPage() -> Element {
    // This is a client-side read with an explicit retry action, so the resource
    // owns loading, cancellation, and the latest result without a separate
    // effect that mutates its own state.
    let mut config = use_resource(|| async { AdminApi::new().mode().await });
    let retry = move |_| config.restart();

    rsx! {
        div { class: "container",
            // The heading is outside the match: a page that is still deciding
            // which flow to show still has a name, and an accessibility check
            // reads a page with no heading as a page with no content.
            h1 { "Sign in" }
            match config.read().as_ref() {
                None => rsx! {
                    Alert {
                        tone: AlertTone::Status,
                        message: Some("Checking how to sign in…".to_owned()),
                    }
                },
                Some(Err(failure)) => rsx! {
                    Alert {
                        tone: AlertTone::Error,
                        message: Some(failure.user_message().to_owned()),
                    }
                    button { r#type: "button", onclick: retry, "Try again" }
                },
                Some(Ok(mode)) if mode.is_local() => rsx! {
                    AdminLoginForm {}
                },
                Some(Ok(mode)) if mode.is_oidc() => rsx! {
                    p { "You will be redirected to your identity provider." }
                    // An anchor, not a button nested inside one: this leaves the
                    // SPA for the provider, and nesting interactive content is
                    // invalid and confuses assistive technology.
                    a { class: "button", href: PATH_OIDC_AUTHORIZE, "Sign in with OIDC" }
                },
                Some(Ok(_)) => rsx! {
                    Alert {
                        tone: AlertTone::Error,
                        message: Some(
                            "This deployment reported a sign-in mode this page does not recognise. Ask an operator to check the server configuration."
                                .to_owned(),
                        ),
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
