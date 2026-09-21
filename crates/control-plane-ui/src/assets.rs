//! Build-time asset declarations.
//!
//! Both assets are named to the bundler rather than loaded by the app, which is
//! what keeps the shipped CSP at `style-src 'self'` / `img-src 'self'` and the
//! console free of any runtime asset directory. Neither is referenced from Rust
//! code, so each is bound to an anonymous `const` — there is nothing to mark as
//! unused.

use dioxus::prelude::*;

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
/// reads a styled page.
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
