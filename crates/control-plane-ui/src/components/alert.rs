//! A message with a semantic tone.
//!
//! Colour, ARIA role and politeness are all chosen from the tone rather than
//! written out at each call site, because getting them wrong is invisible: a
//! failure that is not announced reads, to a screen-reader operator, as nothing
//! having happened at all.
//!
//! Standing guidance (`hint`) and an empty state (`empty`) are deliberately not
//! tones here: neither is a message about something that just happened, so
//! neither belongs in a live region.

use dioxus::prelude::*;

/// How urgent a message is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlertTone {
    /// A failure the operator has to act on.
    Error,
    /// Something the operator should know that is not a failure.
    Warning,
    /// An operation completed as intended.
    Success,
    /// Progress, or an outcome that is neither failure nor success.
    Status,
}

impl AlertTone {
    const fn class(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warning => "warning",
            Self::Success => "success",
            Self::Status => "status",
        }
    }

    /// Errors and warnings interrupt; outcomes wait their turn.
    const fn live(self) -> &'static str {
        match self {
            Self::Error | Self::Warning => "assertive",
            Self::Success | Self::Status => "polite",
        }
    }

    const fn role(self) -> &'static str {
        match self {
            Self::Error | Self::Warning | Self::Success => "alert",
            Self::Status => "status",
        }
    }
}

/// A one-line message.
///
/// The element is rendered even when `message` is absent: an empty `.error` or
/// `.status` is hidden by the stylesheet, so a form's live region can stay
/// mounted from the first render instead of being created along with the text
/// it is meant to announce.
#[component]
pub fn Alert(
    tone: AlertTone,
    /// The message to show. Absent renders the empty container.
    #[props(default)]
    message: Option<String>,
) -> Element {
    let class = tone.class();
    let role = tone.role();
    let live = tone.live();
    rsx! {
        p { class: "{class}", role: "{role}", "aria-live": "{live}",
            if let Some(message) = message {
                "{message}"
            }
        }
    }
}
