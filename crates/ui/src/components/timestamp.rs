//! An RFC 3339 timestamp rendered for a table cell.
//!
//! The visible text stays short enough to keep a row on one line, while
//! `datetime` keeps the machine-readable value and `title` lets a sighted
//! operator recover the exact instant without leaving the page.

use dioxus::prelude::*;

use crate::presentation::compact_timestamp;

/// `value` is the timestamp exactly as the backend reported it.
#[component]
pub fn Timestamp(value: String) -> Element {
    let compact = compact_timestamp(&value);
    rsx! {
        time {
            class: "timestamp",
            datetime: "{value}",
            title: "{value}",
            "{compact}"
        }
    }
}
