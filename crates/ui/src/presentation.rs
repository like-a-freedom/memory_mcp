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

/// Convert a JS `getTimezoneOffset()` reading into minutes east of UTC.
///
/// JavaScript reports UTC-minus-local, so the sign is inverted here and
/// nowhere else. The reading is in minutes already, which is what lets a
/// half-hour zone survive; it is not converted to hours and back.
///
/// A non-finite or out-of-range reading returns `None`: nothing on earth is
/// more than 14 hours from UTC, so such a value is a bad clock rather than a
/// zone, and applying it would shift a timestamp by a day.
///
/// Part of this module's public surface: the conversion is separable from the
/// single `js_sys` call that feeds it, which is what makes it host-testable.
/// Off-wasm nothing calls it — the browser read is compiled out — so the
/// unused warning there is expected rather than a dangling function.
#[allow(dead_code)]
pub fn offset_minutes_from(js_offset_minutes: f64) -> Option<i32> {
    if !js_offset_minutes.is_finite() || js_offset_minutes.abs() > 14.0 * 60.0 {
        return None;
    }
    Some(-(js_offset_minutes as i32))
}

/// The browser's UTC offset, in minutes, **for the given instant**.
///
/// The instant is a parameter rather than an implicit "now" on purpose: a zone
/// that observes DST resolves different offsets for a winter moment and a
/// summer one, so a timestamp rendered with the current offset would be wrong
/// for half the year.
///
/// `js_sys` has no host implementation, so this is compiled for wasm only and
/// the caller falls back to the stored value elsewhere. Splitting it this way
/// is the same seam `admin_api::browser_now_millis` uses.
#[cfg(target_arch = "wasm32")]
pub fn browser_offset_minutes(instant_millis: f64) -> Option<i32> {
    // `Date::new` takes `&JsValue`, and `f64: Into<JsValue>` is provided by
    // wasm-bindgen, so `&instant_millis.into()` is a `&JsValue` built from the
    // millisecond count. A numeric JsValue is milliseconds since the epoch, so
    // this needs no direct `wasm-bindgen` dependency — `js-sys` is already one.
    //
    // This reads the zone rules **for this instant**: `getTimezoneOffset` is
    // `(tv - LocalTime(tv)) / 60000`, and `LocalTime` resolves the political
    // rules "in effect at tv" (ES `LocalTZA`). That is why the instant is
    // passed in rather than read from the clock — a fixed "current" offset
    // would be wrong for half the year in any DST zone.
    let date = js_sys::Date::new(&instant_millis.into());
    offset_minutes_from(date.get_timezone_offset())
}

#[cfg(not(target_arch = "wasm32"))]
pub fn browser_offset_minutes(_instant_millis: f64) -> Option<i32> {
    None
}

/// Render an RFC 3339 instant in a given zone, at minute precision.
///
/// `offset_minutes` is the zone's offset **for this instant**, which is why it
/// is a parameter rather than a value read once: a zone that observes DST
/// resolves different offsets for a winter moment and a summer one, and a
/// timestamp rendered with the current offset would be wrong for half the year.
///
/// The stored value's own offset is data, not a zone to re-apply — the instant
/// is parsed to epoch millis first, so a `+02:00` value converts from UTC rather
/// than being treated as already local.
///
/// No zone label is printed: the zone is the reader's, and naming one would
/// misstate where the value came from. A value that does not parse is returned
/// unchanged, so a malformed timestamp is never silently shifted.
pub fn local_timestamp(value: &str, offset_minutes: Option<i32>) -> String {
    let Some(offset) = offset_minutes else {
        return compact_timestamp(value);
    };
    let Some(instant) = crate::admin_api::parse_rfc3339_millis(value) else {
        return value.to_owned();
    };
    let shifted = instant + i64::from(offset) * 60_000;
    let day_millis = 86_400_000;
    // Floor-divide, so an instant before the epoch lands on the previous day
    // rather than truncating toward zero and reading a day late.
    let days = shifted.div_euclid(day_millis);
    let within_day = shifted.rem_euclid(day_millis);
    let (year, month, day) = crate::admin_api::civil_from_days(days);
    let hours = within_day / 3_600_000;
    let minutes = (within_day % 3_600_000) / 60_000;
    format!("{year:04}-{month:02}-{day:02} {hours:02}:{minutes:02}")
}

/// Render an instant in the browser's timezone, falling back to the stored
/// value when that is unknown.
///
/// This is the single entry point the `Timestamp` component calls, and the
/// single place the browser is consulted. A value that does not parse, or a
/// build with no browser to ask, both fall through to
/// [`compact_timestamp`] — so a timestamp is never shifted on a guess.
pub fn render_in_browser_zone(value: &str) -> String {
    let Some(instant) = crate::admin_api::parse_rfc3339_millis(value) else {
        return compact_timestamp(value);
    };
    local_timestamp(value, browser_offset_minutes(instant as f64))
}

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
    use super::{
        Status, StatusTone, browser_offset_minutes, compact_timestamp, local_timestamp,
        offset_minutes_from, render_in_browser_zone,
    };

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
    fn a_negative_offset_shifts_the_clock_backward_across_a_day_boundary() {
        assert_eq!(
            local_timestamp("2026-10-02T02:30:00Z", Some(-300)),
            "2026-10-01 21:30"
        );
    }

    #[test]
    fn half_hour_and_quarter_hour_zones_are_supported() {
        // Zones that are not a whole number of hours are the offsets most likely
        // to break arithmetic that assumes otherwise.
        assert_eq!(
            local_timestamp("2026-10-02T13:47:00Z", Some(330)),
            "2026-10-02 19:17"
        );
        assert_eq!(
            local_timestamp("2026-10-02T13:47:00Z", Some(345)),
            "2026-10-02 19:32"
        );
    }

    #[test]
    fn a_whole_hour_zone_across_a_month_boundary_is_exact() {
        assert_eq!(
            local_timestamp("2026-03-01T00:30:00Z", Some(60)),
            "2026-03-01 01:30"
        );
        assert_eq!(
            local_timestamp("2026-03-01T00:30:00Z", Some(-60)),
            "2026-02-28 23:30"
        );
    }

    #[test]
    fn a_shift_across_a_leap_day_is_exact() {
        // February 2028 has 29 days; a shift landing on the 29th proves the day
        // arithmetic reads the calendar rather than assuming a month length.
        assert_eq!(
            local_timestamp("2028-02-28T23:30:00Z", Some(60)),
            "2028-02-29 00:30"
        );
    }

    #[test]
    fn an_unknown_offset_leaves_the_value_exactly_as_it_arrived() {
        assert_eq!(
            local_timestamp("2026-10-02T13:47:00Z", None),
            "2026-10-02 13:47 UTC"
        );
        assert_eq!(local_timestamp("not-a-date", Some(60)), "not-a-date");
        assert_eq!(local_timestamp("", Some(60)), "");
    }

    #[test]
    fn a_borrowed_offset_in_the_value_is_not_mistaken_for_the_browser_zone() {
        // The stored value's own offset is data, not a zone to re-apply: a
        // `+02:00` instant converts from UTC rather than being treated as already
        // local. 13:47+02:00 is 11:47 UTC, so a UTC browser shows 11:47.
        assert_eq!(
            local_timestamp("2026-10-02T13:47:00+02:00", Some(0)),
            "2026-10-02 11:47"
        );
    }

    #[test]
    fn an_instant_before_the_epoch_shifts_to_the_previous_day() {
        assert_eq!(
            local_timestamp("1969-12-31T23:30:00Z", Some(-60)),
            "1969-12-31 22:30"
        );
        assert_eq!(
            local_timestamp("1970-01-01T00:30:00Z", Some(-60)),
            "1969-12-31 23:30"
        );
    }

    #[test]
    fn a_browser_offset_is_minutes_east_of_utc_with_the_sign_inverted() {
        // JS `getTimezoneOffset()` is UTC-minus-local, so a browser in UTC+1
        // reports -60 and the conversion must hand back +60. A browser in
        // UTC+5:30 reports -330 and must hand back +330 — the reading is
        // already in minutes, and rounding it to hours would lose the half.
        assert_eq!(offset_minutes_from(-60.0), Some(60));
        assert_eq!(offset_minutes_from(-330.0), Some(330));
        assert_eq!(offset_minutes_from(-345.0), Some(345));
        assert_eq!(offset_minutes_from(0.0), Some(0));
        assert_eq!(offset_minutes_from(60.0), Some(-60));
        assert_eq!(offset_minutes_from(f64::NAN), None);
        assert_eq!(offset_minutes_from(f64::INFINITY), None);
        // Nothing on earth is 15 hours from UTC; an out-of-range reading is a bad
        // clock, not a zone, and must not be applied to a timestamp.
        assert_eq!(offset_minutes_from(15.0 * 60.0), None);
        assert_eq!(offset_minutes_from(-15.0 * 60.0), None);
    }

    #[test]
    fn rendering_without_a_browser_keeps_the_stored_value() {
        // Off-wasm the offset is always None, so this is what the host suite pins:
        // the exact instant the backend sent, never a shifted one. In a browser the
        // same call renders the reader's own zone and drops the " UTC" label — so
        // these expectations describe the host fallback specifically, not browser
        // output.
        assert_eq!(
            render_in_browser_zone("2026-10-02T13:47:00Z"),
            "2026-10-02 13:47 UTC"
        );
        assert_eq!(
            render_in_browser_zone("2026-10-02T13:47:00+02:00"),
            "2026-10-02 13:47 +02:00"
        );
        assert_eq!(render_in_browser_zone("not-a-date"), "not-a-date");
        assert_eq!(render_in_browser_zone(""), "");
    }

    #[test]
    fn no_browser_means_no_offset_rather_than_a_guessed_one() {
        // `browser_offset_minutes` is the one function whose correctness the host
        // suite cannot reach, because it is compiled out. What this pins is the
        // contract the wasm build relies on: off-wasm it answers `None`, so the
        // fallback is the stored value rather than an assumed zero offset. A `Some`
        // here would mean every host-rendered timestamp silently claimed UTC.
        assert_eq!(browser_offset_minutes(0.0), None);
        assert_eq!(browser_offset_minutes(1_788_000_000_000.0), None);
    }

    #[test]
    fn a_positive_offset_shifts_the_clock_forward() {
        assert_eq!(
            local_timestamp("2026-10-02T13:47:00Z", Some(180)),
            "2026-10-02 16:47"
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
