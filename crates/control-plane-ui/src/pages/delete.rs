//! Account deletion page.

use dioxus::prelude::*;

#[component]
pub fn DeletePage() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Delete Account" }
            nav {
                a { href: "/", "Back to Status" }
            }
        }
    }
}
