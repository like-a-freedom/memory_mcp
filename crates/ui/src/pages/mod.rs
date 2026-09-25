//! One module per route, plus the submodules a single page needs.
//!
//! A module belongs here only if a route renders it. Anything shared by two
//! pages lives in `components/`, state lives in `state/`, and anything a page
//! needs on its own — the sections of the client detail page, for instance —
//! lives in a directory beside that page.

pub mod admin_auth;
pub mod admin_client;
pub mod admin_clients;
pub mod delete;
pub mod keys;
pub mod login;
pub mod not_found;
pub mod status;
