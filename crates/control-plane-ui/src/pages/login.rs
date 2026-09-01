//! Login page — redirects to OIDC provider.

use dioxus::prelude::*;

#[component]
pub fn LoginPage() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Login" }
            p { "You will be redirected to your identity provider." }
            a { href: "/api/v1/auth/authorize", button { "Login with OIDC" } }
        }
    }
}
