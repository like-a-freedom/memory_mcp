//! Local admin client management pages.

use dioxus::prelude::*;

/// Client list page.
#[component]
pub fn AdminClientListPage() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Clients" }
            div { class: "actions",
                button {
                    onclick: move |_| {
                        // TODO: navigate to create client
                    },
                    "Create Client"
                }
            }
            div { class: "client-list" }
            // TODO: fetch and display clients
        }
    }
}

/// Client detail page.
#[component]
pub fn AdminClientDetailPage(client_id: String) -> Element {
    rsx! {
        div { class: "container",
            h1 { "Client Detail" }
            p { "Client ID: {client_id}" }
            div { class: "actions",
                button {
                    onclick: move |_| {
                        // TODO: issue key
                    },
                    "Issue Key"
                }
                button {
                    onclick: move |_| {
                        // TODO: suspend client
                    },
                    "Suspend"
                }
            }
            div { class: "keys-section" }
            // TODO: fetch and display keys
        }
    }
}

/// Client keys page.
#[component]
pub fn AdminClientKeysPage() -> Element {
    rsx! {
        div { class: "container",
            h1 { "Client Keys" }
            div { class: "actions",
                button {
                    onclick: move |_| {
                        // TODO: issue new key
                    },
                    "Issue New Key"
                }
            }
            div { class: "key-list" }
            // TODO: fetch and display keys
        }
    }
}
