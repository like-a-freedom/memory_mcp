//! Shared presentation helpers for the embedded operator console.
//!
//! These helpers keep labels, timestamps, and semantic state colors consistent
//! across the local-admin and OIDC account surfaces without adding a UI library
//! or a runtime asset.

/// Turn a backend status into a short operator-facing label.
pub fn status_label(value: &str) -> String {
    match value {
        "active" => "Active".to_owned(),
        "ready" => "Ready".to_owned(),
        "reserved" => "Provisioning".to_owned(),
        "namespace_creating" => "Creating namespace".to_owned(),
        "migrating" => "Migrating".to_owned(),
        "failed" => "Failed".to_owned(),
        "suspended" => "Suspended".to_owned(),
        "revoked" => "Revoked".to_owned(),
        "expired" => "Expired".to_owned(),
        "deleting" => "Deleting".to_owned(),
        "_" => "Unknown".to_owned(),
        other => {
            let mut label = String::with_capacity(other.len());
            let mut uppercase_next = true;
            for character in other.chars() {
                if character == '_' || character == '-' {
                    if !label.is_empty() {
                        label.push(' ');
                    }
                    uppercase_next = label.is_empty();
                } else if uppercase_next {
                    label.extend(character.to_uppercase());
                    uppercase_next = false;
                } else {
                    label.push(character);
                }
            }
            if label.is_empty() {
                "Unknown".to_owned()
            } else {
                label
            }
        }
    }
}

/// Return the shared status badge class for a backend state.
pub fn status_badge_class(value: &str) -> &'static str {
    match value {
        "active" | "ready" => "status-badge status-badge--success",
        "reserved" | "namespace_creating" | "migrating" => "status-badge status-badge--warning",
        "failed" | "revoked" | "expired" | "deleting" => "status-badge status-badge--danger",
        _ => "status-badge status-badge--muted",
    }
}

/// Make an RFC 3339 timestamp compact enough for a table while retaining its
/// exact value in the element's `datetime` and `title` attributes.
pub fn compact_timestamp(value: &str) -> String {
    let trimmed = value.trim();
    let Some(separator) = trimmed.find('T') else {
        return trimmed.to_owned();
    };
    let date = &trimmed[..separator];
    let remainder = &trimmed[separator + 1..];
    let time = remainder.get(..8).unwrap_or(remainder);
    let zone = remainder.get(8..).unwrap_or_default();
    let zone = if zone == "Z" { " UTC" } else { zone };
    format!("{date} {time}{zone}")
}

#[cfg(test)]
mod tests {
    use super::{compact_timestamp, status_badge_class, status_label};

    #[test]
    fn known_states_use_plain_operator_labels() {
        assert_eq!(status_label("namespace_creating"), "Creating namespace");
        assert_eq!(status_label("ready"), "Ready");
        assert_eq!(status_label("failed"), "Failed");
    }

    #[test]
    fn unknown_states_are_humanized_without_leaking_empty_copy() {
        assert_eq!(status_label("custom_state"), "Custom state");
        assert_eq!(status_label(""), "Unknown");
        assert_eq!(status_label("_"), "Unknown");
    }

    #[test]
    fn semantic_badge_classes_follow_state_meaning() {
        assert!(status_badge_class("ready").contains("success"));
        assert!(status_badge_class("migrating").contains("warning"));
        assert!(status_badge_class("failed").contains("danger"));
        assert!(status_badge_class("unknown").contains("muted"));
    }

    #[test]
    fn timestamps_are_readable_but_keep_the_original_precision_elsewhere() {
        assert_eq!(
            compact_timestamp("2026-09-19T10:15:00Z"),
            "2026-09-19 10:15:00 UTC"
        );
        assert_eq!(
            compact_timestamp("2026-09-19T10:15:00+00:00"),
            "2026-09-19 10:15:00+00:00"
        );
        assert_eq!(compact_timestamp("not-a-date"), "not-a-date");
    }
}
