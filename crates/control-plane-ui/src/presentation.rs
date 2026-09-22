//! Shared presentation for the embedded operator console.
//!
//! A state word, a status colour and a timestamp each appear in many places on
//! both the administrator and the account surfaces. They are defined once here
//! so the same backend fact cannot read differently on two pages.
//!
//! Nothing in this module touches the network, the DOM or a runtime asset.

use std::borrow::Cow;

/// Semantic weight of a state, which is what selects a badge colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    /// The object can be used.
    Success,
    /// The object is still becoming usable.
    Warning,
    /// The object cannot be used.
    Danger,
    /// A neutral or unrecognised state.
    Muted,
}

impl StatusTone {
    /// The `class` attribute of a badge in this tone.
    ///
    /// One mapping, used both by a state word the backend reported and by a
    /// label the console writes itself, so the two can never disagree about what
    /// a colour means.
    pub const fn badge_class(self) -> &'static str {
        match self {
            Self::Success => "status-badge status-badge--success",
            Self::Warning => "status-badge status-badge--warning",
            Self::Danger => "status-badge status-badge--danger",
            Self::Muted => "status-badge status-badge--muted",
        }
    }
}

/// A state word reported by the backend, as the operator should read it.
///
/// The vocabulary belongs to the backend and can grow without a console
/// release, so an unrecognised value is humanized rather than dropped:
/// `Custom state` is honest, an empty cell is not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status(String);

impl Status {
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// The exact word the backend sent.
    pub fn raw(&self) -> &str {
        &self.0
    }

    /// Short label for a table cell, a badge or a summary sentence.
    pub fn label(&self) -> Cow<'_, str> {
        match self.raw() {
            "active" => Cow::Borrowed("Active"),
            "ready" => Cow::Borrowed("Ready"),
            "reserved" => Cow::Borrowed("Reserved"),
            "namespace_creating" => Cow::Borrowed("Creating namespace"),
            "migrating" => Cow::Borrowed("Migrating"),
            "failed" => Cow::Borrowed("Failed"),
            "suspended" => Cow::Borrowed("Suspended"),
            "revoked" => Cow::Borrowed("Revoked"),
            "expired" => Cow::Borrowed("Expired"),
            "deleting" => Cow::Borrowed("Deleting"),
            "_" => Cow::Borrowed("Unknown"),
            other => match humanize(other) {
                Some(label) => Cow::Owned(label),
                None => Cow::Borrowed("Unknown"),
            },
        }
    }

    /// The colour family this state belongs to.
    pub fn tone(&self) -> StatusTone {
        match self.raw() {
            "active" | "ready" => StatusTone::Success,
            "reserved" | "namespace_creating" | "migrating" => StatusTone::Warning,
            // `suspended` belongs here by this module's own definition of the
            // tone: the object cannot be used. The state is deliberate — a
            // kill switch reads as danger, not as an unrecognised state.
            "failed" | "revoked" | "expired" | "deleting" | "suspended" => StatusTone::Danger,
            _ => StatusTone::Muted,
        }
    }

    /// The `class` attribute of the badge this state is rendered in.
    pub fn badge_class(&self) -> &'static str {
        self.tone().badge_class()
    }
}

/// Turn `custom_state` into `Custom state`.
///
/// Returns `None` when nothing readable is left, so the caller can fall back
/// instead of rendering an empty cell.
fn humanize(raw: &str) -> Option<String> {
    let mut label = String::with_capacity(raw.len());
    let mut capitalize_next = true;
    for character in raw.chars() {
        if character == '_' || character == '-' {
            if !label.is_empty() {
                label.push(' ');
            }
            capitalize_next = label.is_empty();
        } else if capitalize_next {
            label.extend(character.to_uppercase());
            capitalize_next = false;
        } else {
            label.push(character);
        }
    }
    (!label.is_empty()).then_some(label)
}

/// What an administrator may do to a client's keys.
///
/// Stated on every page that offers key issuance. It is defined once because it
/// is a claim about auditing and access, and two pages must not make it
/// differently.
pub const KEY_PRIVILEGE_NOTE: &str =
    "Administrators can issue keys that access this client's memory. Issuance is audited.";

/// Make an RFC 3339 timestamp compact enough for a table cell while the exact
/// value stays available in the element's `datetime` and `title` attributes.
///
/// Minutes are the precision an operator acts at: seconds, fractional seconds
/// and offset seconds are noise that forces every value to be parsed by eye.
/// A UTC instant (either `Z` or `+00:00`, with or without fractional seconds)
/// reads as `2026-09-23 19:59 UTC`; another offset keeps itself.
pub fn compact_timestamp(value: &str) -> String {
    let trimmed = value.trim();
    let Some(separator) = trimmed.find(['T', ' ']) else {
        return trimmed.to_owned();
    };
    let date = &trimmed[..separator];
    let remainder = &trimmed[separator + 1..];
    let time = remainder.get(..5).unwrap_or(remainder);
    // The offset is the last `+` or `-` after the date; everything between the
    // clock and the offset is the fractional second being dropped here.
    let offset = trimmed
        .rfind(['+', '-'])
        .filter(|at| *at > separator)
        .map(|at| &trimmed[at..]);
    let zone = match offset {
        Some("+00:00") => " UTC",
        Some(offset) => return format!("{date} {time} {offset}"),
        None if trimmed.ends_with('Z') => " UTC",
        None => "",
    };
    format!("{date} {time}{zone}")
}

#[cfg(test)]
mod tests {
    use super::{Status, StatusTone, compact_timestamp};

    #[test]
    fn known_states_use_plain_operator_labels() {
        assert_eq!(
            Status::new("namespace_creating").label(),
            "Creating namespace"
        );
        assert_eq!(Status::new("ready").label(), "Ready");
        assert_eq!(Status::new("failed").label(), "Failed");
        assert_eq!(Status::new("reserved").label(), "Reserved");
    }

    #[test]
    fn unknown_states_are_humanized_without_leaking_empty_copy() {
        assert_eq!(Status::new("custom_state").label(), "Custom state");
        assert_eq!(Status::new("").label(), "Unknown");
        assert_eq!(Status::new("_").label(), "Unknown");
    }

    #[test]
    fn semantic_tones_follow_state_meaning() {
        assert_eq!(Status::new("ready").tone(), StatusTone::Success);
        assert_eq!(Status::new("migrating").tone(), StatusTone::Warning);
        assert_eq!(Status::new("failed").tone(), StatusTone::Danger);
        assert_eq!(Status::new("unknown").tone(), StatusTone::Muted);
        // A suspended client cannot be used either, whatever produced the
        // state: the tone answers that question, not whose idea it was.
        assert_eq!(Status::new("suspended").tone(), StatusTone::Danger);
    }

    #[test]
    fn every_tone_has_its_own_badge_class() {
        // The mapping is what makes a state readable at a glance, so a tone that
        // silently shared another tone's colour would be a defect.
        let classes = [
            Status::new("ready").badge_class(),
            Status::new("migrating").badge_class(),
            Status::new("failed").badge_class(),
            Status::new("custom").badge_class(),
        ];
        for (index, class) in classes.iter().enumerate() {
            assert!(class.starts_with("status-badge "));
            assert!(
                !classes[..index].contains(class),
                "{class} is shared by two tones"
            );
        }
    }

    #[test]
    fn the_raw_word_survives_for_debug_contexts() {
        assert_eq!(
            Status::new("namespace_creating").label(),
            "Creating namespace"
        );
        assert_eq!(
            Status::new("namespace_creating").raw(),
            "namespace_creating"
        );
    }

    #[test]
    fn timestamps_are_readable_but_keep_the_original_precision_elsewhere() {
        assert_eq!(
            compact_timestamp("2026-09-19T10:15:00Z"),
            "2026-09-19 10:15 UTC"
        );
        assert_eq!(
            compact_timestamp("2026-09-19T10:15:00+00:00"),
            "2026-09-19 10:15 UTC"
        );
        assert_eq!(
            compact_timestamp("2026-09-19T10:15:00+02:00"),
            "2026-09-19 10:15 +02:00"
        );
        assert_eq!(compact_timestamp("not-a-date"), "not-a-date");
    }

    #[test]
    fn fractional_seconds_never_reach_the_visible_zone() {
        // The two shapes the backend actually sends. A `.707974+00:00` suffix
        // used to pass through as the "zone", printing microseconds in every
        // table cell and in the session bar.
        assert_eq!(
            compact_timestamp("2026-09-23T19:59:41.707974+00:00"),
            "2026-09-23 19:59 UTC"
        );
        assert_eq!(
            compact_timestamp("2026-09-22T20:08:03.877263Z"),
            "2026-09-22 20:08 UTC"
        );
    }
}
