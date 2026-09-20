#![allow(non_snake_case)]

use dioxus::prelude::*;

mod admin_api;
mod api;
mod pages;
mod presentation;
mod router;

/// Declares the operator console stylesheet to the bundler.
///
/// `asset!` content-hashes the file and copies it into the bundle, so the link
/// the CLI writes into the document head is same-origin and the shipped
/// `style-src 'self'` policy is sufficient: no inline `<style>`, no third-party
/// host, no runtime injection, and no flash of unstyled content while
/// WebAssembly boots.
///
/// `with_static_head` is what puts the `<link>` in the served HTML rather than
/// adding it from the app at runtime. That matters twice over: the shell paints
/// before the module executes, and a client that cannot run WebAssembly still
/// reads a styled page. It is deliberately an anonymous `const` — the asset is
/// referenced by the build, not by Rust code, so there is nothing to bind and
/// nothing to mark as unused.
const _: Asset = asset!(
    "/assets/main.css",
    AssetOptions::css().with_static_head(true)
);

/// The tab icon.
///
/// `with_hash_suffix(false)` keeps it at a stable `/assets/favicon.svg` so
/// `index.html` can reference it directly. The link has to be static: a browser
/// probes `/favicon.ico` before the module boots, so an icon the app injects at
/// runtime arrives after the 404, and the tab stays generic until then.
const _: Asset = asset!(
    "/assets/favicon.svg",
    AssetOptions::builder().with_hash_suffix(false)
);

fn main() {
    dioxus::launch(App);
}

/// Application shell: the single `<main>` landmark every route renders into.
///
/// The signed-in administrator banner is not here — it is per-page, because it
/// is read from the page's own session signal.
#[component]
fn App() -> Element {
    rsx! {
        a { class: "skip-link", href: "#main-content", "Skip to content" }
        header {
            class: "app-header",
            div { class: "app-header-inner",
                span { class: "app-brand", "Memory MCP" }
                span { class: "app-brand-suffix", "control plane" }
            }
        }
        main { id: "main-content", tabindex: "-1", router::AppRouter {} }
    }
}
