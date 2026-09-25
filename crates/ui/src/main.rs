//! The control-plane console.
//!
//! A single-page WebAssembly application, embedded in the `memory_mcp_http`
//! binary and served from it. This file is the entry point only: parsing,
//! dispatch and the shell live elsewhere.

#![allow(non_snake_case)]

use std::rc::Rc;

use crate::layouts::app::App;
use dioxus_web::{Config, WebHistory};

mod admin_api;
mod api;
mod assets;
mod base;
mod components;
mod inert;
mod layouts;
mod pages;
mod presentation;
mod routes;
mod state;

fn main() {
    // The router prefix is the same runtime value the fetch layer uses
    // (`crate::base::base_path`) — the base `memory_mcp_http` stamped into
    // this document. `do_scroll_restoration: true` matches `WebHistory`'s own
    // default (`Default` is `new(None, true)`), so root deployments keep
    // today's behavior byte for byte.
    let history = WebHistory::new(Some(crate::base::base_path()), true);
    dioxus::LaunchBuilder::new()
        .with_cfg(Config::new().history(Rc::new(history)))
        .launch(App);
}
