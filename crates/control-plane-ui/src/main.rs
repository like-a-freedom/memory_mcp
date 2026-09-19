#![allow(non_snake_case)]

use dioxus::prelude::*;

mod admin_api;
mod api;
mod pages;
mod router;

fn main() {
    dioxus::launch(App);
}

#[component]
fn App() -> Element {
    rsx! {
        router::AppRouter {}
    }
}
