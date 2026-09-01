//! Client-side router for the control-plane SPA.

use dioxus::prelude::*;

use crate::pages::{delete, keys, login, status};

#[derive(Routable, Clone, PartialEq)]
pub enum Route {
    #[route("/")]
    Status {},
    #[route("/login")]
    Login {},
    #[route("/keys")]
    Keys {},
    #[route("/delete")]
    Delete {},
}

#[component]
pub fn Router() -> Element {
    rsx! {
        match routek {
            Route::Status {} => rsx! { status::StatusPage {} },
            Route::Login {} => rsx! { login::LoginPage {} },
            Route::Keys {} => rsx! { keys::KeysPage {} },
            Route::Delete {} => rsx! { delete::DeletePage {} },
        }
    }
}
