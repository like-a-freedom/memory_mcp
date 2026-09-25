//! State shared by the operator console's pages.
//!
//! Everything here is deliberately free of markup: a page renders this state,
//! it does not live in it. Keeping the two apart is what lets the state be
//! tested without a browser and rendered without duplicating logic.

pub mod account_session;
pub mod admin_session;
pub mod paged_query;
pub mod polling;
pub mod query;
