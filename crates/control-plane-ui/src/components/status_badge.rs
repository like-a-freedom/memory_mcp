//! Status badges.
//!
//! Two entry points, because there are two honest sources for a badge's text:
//! a state word the backend reported, and a short label this console writes
//! itself. Both share one tone-to-class mapping, so a colour means the same
//! thing whichever produced the label.

use dioxus::prelude::*;

use crate::presentation::{Status, StatusTone};

/// A state word reported by the backend.
#[component]
pub fn StatusBadge(value: String) -> Element {
    let status = Status::new(value);
    let class = status.badge_class();
    let label = status.label();
    rsx! {
        span { class: "{class}", "{label}" }
    }
}

/// A short label this console writes, in a tone it chooses.
///
/// For facts the backend does not name — "Provisioning" is the console's word
/// for a state it derived from several backend fields, not a state word the
/// backend reports.
#[component]
pub fn ToneBadge(tone: StatusTone, label: String) -> Element {
    let class = tone.badge_class();
    rsx! {
        span { class: "{class}", "{label}" }
    }
}
