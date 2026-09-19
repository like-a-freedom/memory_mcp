//! Client-side router for the control-plane SPA.

use dioxus::prelude::*;
use dioxus_router::{Routable, Router as DioxusRouter};

use crate::pages::{
    admin_auth::{AdminActivationPage as AdminActivate, AdminLoginPage as AdminLogin, AdminReauthDialog as AdminReauth, AdminResetPage as AdminReset},
    admin_clients::{AdminClientDetailPage as AdminClientDetail, AdminClientListPage as AdminClientList},
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
    #[route("/admin/login")]
    AdminLogin {},
    #[route("/admin/activate")]
    AdminActivate {},
    #[route("/admin/reset")]
    AdminReset {},
    #[route("/admin/reauth")]
    AdminReauth {},
    #[route("/admin/clients")]
    AdminClientList {},
    #[route("/admin/clients/:client_id")]
    AdminClientDetail { client_id: String },
}

#[component]
pub fn AppRouter() -> Element {
    rsx! {
        DioxusRouter::<Route> {}
    }
}
