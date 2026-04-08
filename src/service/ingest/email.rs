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
