//! An RFC 3339 timestamp rendered for a table cell.
//!
//! The visible text stays short enough to keep a row on one line, while
//! `datetime` keeps the machine-readable value and `title` lets a sighted
//! operator recover the exact instant without leaving the page.

use dioxus::prelude::*;

/// `value` is the timestamp exactly as the backend reported it.
///
/// The visible text is in the reader's own timezone — an operator reading an
/// expiry in UTC has to do arithmetic to answer "has this already passed".
/// `datetime` and `title` keep the backend's exact instant, so the machine
/// readable value and the hover text remain the authority.
#[component]
pub fn Timestamp(value: String) -> Element {
    let shown = crate::presentation::render_in_browser_zone(&value);
    rsx! {
        time {
            class: "timestamp",
            datetime: "{value}",
            title: "{value}",
            "{shown}"
        }
    }
}
