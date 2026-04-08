use super::MemoryError;

pub(crate) fn extract_markdown(bytes: &[u8]) -> Result<String, MemoryError> {
    let raw = decode_utf8(bytes)?;
    Ok(raw.replace("**", "").replace("__", "").replace("#", " "))
}

pub(crate) fn extract_plain_text(bytes: &[u8]) -> Result<String, MemoryError> {
    decode_utf8(bytes)
}

fn decode_utf8(bytes: &[u8]) -> Result<String, MemoryError> {
    String::from_utf8(bytes.to_vec())
        .map_err(|err| MemoryError::Validation(format!("text fixture is not valid UTF-8: {err}")))
}
