//! The control-plane console.
//!
//! A single-page WebAssembly application, embedded in the `memory_mcp_http`
//! binary and served from it. This file is the entry point only: parsing,
//! dispatch and the shell live elsewhere.

#![allow(non_snake_case)]

use crate::layouts::app::App;

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
    dioxus::launch(App);
}
