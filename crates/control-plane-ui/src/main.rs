#![allow(non_snake_case)]

use dioxus::prelude::*;

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
