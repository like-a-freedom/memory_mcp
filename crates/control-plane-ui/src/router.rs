//! Client-side router for the control-plane SPA.

use dioxus::prelude::*;
use dioxus_router::{Routable, Router as DioxusRouter};

use crate::pages::{
    delete::DeletePage as Delete, keys::KeysPage as Keys, login::LoginPage as Login,
    status::StatusPage as Status,
};

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
pub fn AppRouter() -> Element {
    rsx! {
        DioxusRouter::<Route> {}
    }
}
