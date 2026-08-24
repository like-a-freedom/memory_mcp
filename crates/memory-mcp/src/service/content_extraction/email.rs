use super::MemoryError;

pub(crate) fn extract_email(bytes: &[u8]) -> Result<String, MemoryError> {
    let raw = String::from_utf8(bytes.to_vec()).map_err(|err| {
        MemoryError::Validation(format!("email fixture is not valid UTF-8: {err}"))
    })?;
    let normalized = raw.replace("\r\n", "\n");
    let (headers, body) = normalized
        .split_once("\n\n")
        .unwrap_or((normalized.as_str(), ""));

    let subject = headers
        .lines()
        .find_map(|line| line.strip_prefix("Subject:"))
        .map(str::trim);
    let from = headers
        .lines()
        .find_map(|line| line.strip_prefix("From:"))
        .map(str::trim);

    let mut parts = Vec::new();
    if let Some(subject) = subject
        && !subject.is_empty()
    {
        parts.push(format!("Subject: {subject}"));
    }
    if let Some(from) = from
        && !from.is_empty()
    {
        parts.push(format!("From: {from}"));
    }
    if !body.trim().is_empty() {
        parts.push(body.trim().to_string());
    }

    Ok(parts.join("\n"))
}

/// Parses the structured `Date` header of an EML document into UTC. Returns
/// `None` when the header is absent or unparseable; deliberately never falls
/// back to arbitrary body dates.
#[cfg(feature = "fs-watch")]
pub(crate) fn parse_email_date_header(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let normalized = raw.replace("\r\n", "\n");
    let (headers, _) = normalized.split_once("\n\n").unwrap_or((&normalized, ""));
    let date = headers.lines().find_map(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with("Date:") {
            Some(trimmed.trim_start_matches("Date:").trim())
        } else {
            None
        }
    })?;
    parse_email_date_value(date)
}

/// Parses a single RFC-2822 style email date value.
#[cfg(feature = "fs-watch")]
pub(crate) fn parse_email_date_value(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc2822(value.trim())
        .map(|parsed| parsed.with_timezone(&chrono::Utc))
        .ok()
}

#[cfg(all(test, feature = "fs-watch"))]
mod tests {
    use super::*;

    #[test]
    fn parses_structured_date_header() {
        let raw = "From: alice@example.com\nDate: Wed, 12 Aug 2026 10:30:00 +0000\n\nBody";
        let parsed = parse_email_date_header(raw).expect("date header");
        assert_eq!(parsed.to_rfc3339(), "2026-08-12T10:30:00+00:00");
    }

    #[test]
    fn date_header_missing_returns_none() {
        let raw = "From: alice@example.com\nSubject: no date\n\nBody";
        assert_eq!(parse_email_date_header(raw), None);
    }

    #[test]
    fn unparseable_date_header_returns_none() {
        let raw = "Date: someday soon\n\nBody";
        assert_eq!(parse_email_date_header(raw), None);
    }

    #[test]
    fn date_with_timezone_offset_is_normalized_to_utc() {
        let parsed = parse_email_date_value("Thu, 13 Aug 2026 09:00:00 +0200").expect("parsed");
        assert_eq!(parsed.to_rfc3339(), "2026-08-13T07:00:00+00:00");
    }
}
