//! Fallback route for a path the console does not serve.
//!
//! The bundle is a single-page app served from one entry document, so any path
//! under it answers with that document and the router decides what to render. A
//! path it does not know — a stale bookmark, a mistyped URL — would otherwise
//! leave the operator on a blank shell with no way back. It gets a page with a
//! way back instead.
//!
//! The message is a paragraph rather than an error panel: nothing failed, the
//! address simply does not exist, and an alarm-coloured alert on load reports a
//! problem the operator does not have.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::routes::Route;

#[component]
pub fn PageNotFound(route: Vec<String>) -> Element {
    // Rendered as text, never as markup: the segments come from the URL, so they
    // are operator-supplied input like any other.
    let path = route.join("/");

    rsx! {
        div { class: "container",
            h1 { "Page not found" }
            p {
                "This console has no page at "
                code { "/{path}" }
                ". Check the address, or start from one of these."
            }
            nav { class: "actions", "aria-label": "Ways back",
                Link { class: "button", to: Route::Status {}, "Account status" }
                Link { class: "button", to: Route::Login {}, "Sign in" }
            }
        }
    }
}
