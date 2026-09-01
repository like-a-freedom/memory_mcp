#![allow(non_snake_case)]

use dioxus::prelude::*;

#[allow(dead_code)]
mod api;

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        div {
            h1 { "Memory MCP Control Plane" }
            p { "Login to manage your account." }
            a { href: "/api/v1/auth/authorize", "Login with OIDC" }
        }
    }
}
