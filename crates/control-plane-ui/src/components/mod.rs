//! Reusable presentation shared by more than one page.
//!
//! Nothing here is hoisted "for later": a component lives in this module only
//! once a second page needs it. Anything with a single consumer stays next to
//! the page that renders it, where it can be read in place.

pub mod admin_auth;
pub mod alert;
pub mod modal;
pub mod one_time_secret;
pub mod session_bar;
pub mod status_badge;
pub mod timestamp;
