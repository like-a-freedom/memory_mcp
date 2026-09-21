//! The console's routes.
//!
//! One enum is the single source of truth for the URL space: a page cannot exist
//! without a variant here, and a `Link` cannot point at a route that was never
//! declared. Layouts are attached with `#[layout]`, so the signed-in console
//! renders its own chrome around both of its routes without either page
//! repeating it.

use dioxus::prelude::*;
use dioxus_router::{Routable, Router as DioxusRouter};

use crate::layouts::console::ConsoleLayout;
use crate::pages::admin_auth::{
    AdminActivationPage as AdminActivate, AdminLoginPage as AdminLogin,
    AdminReauthPage as AdminReauth, AdminResetPage as AdminReset,
};
use crate::pages::admin_client::AdminClientDetailPage as AdminClientDetail;
use crate::pages::admin_clients::AdminClientListPage as AdminClientList;
use crate::pages::delete::DeletePage as Delete;
use crate::pages::keys::KeysPage as Keys;
use crate::pages::login::LoginPage as Login;
use crate::pages::not_found::PageNotFound;
use crate::pages::status::StatusPage as Status;

/// `/` and the account pages are the OIDC surface; `/admin/*` is the local
/// administrator surface, and only the client routes are signed-in console.
#[derive(Routable, Clone, PartialEq)]
#[rustfmt::skip]
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

    #[layout(ConsoleLayout)]
        #[route("/admin/clients")]
        AdminClientList {},

        #[route("/admin/clients/:account_id")]
        AdminClientDetail { account_id: String },
    #[end_layout]

    // Must stay last: this variant matches every path no earlier variant took,
    // so a URL the console does not serve renders a page instead of nothing.
    #[route("/:..route")]
    PageNotFound { route: Vec<String> },
}

#[component]
pub fn AppRouter() -> Element {
    rsx! {
        DioxusRouter::<Route> {}
    }
}
