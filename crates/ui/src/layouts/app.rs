//! The application shell.
//!
//! One `<main>` landmark, one skip link, one header — rendered once, above the
//! router, so every route is inside the same document structure and a route
//! cannot accidentally ship without a landmark.

use dioxus::prelude::*;

use crate::routes::AppRouter;

/// The single `<main>` landmark every route renders into.
///
/// The signed-in administrator banner is not here: it is part of the console
/// layout, because only the signed-in console has a session.
#[component]
pub fn App() -> Element {
    rsx! {
        a { class: "skip-link", href: "#main-content", "Skip to content" }
        header {
            class: "app-header",
            div { class: "app-header-inner",
                span { class: "app-brand", "Memory MCP" }
                // A whitespace text node between the spans: flex ignores it
                // visually, while text extraction and screen readers stop
                // reading "Memory MCPcontrol plane".
                " "
                span { class: "app-brand-suffix", "control plane" }
            }
        }
        main { id: "main-content", tabindex: "-1", AppRouter {} }
    }
}
