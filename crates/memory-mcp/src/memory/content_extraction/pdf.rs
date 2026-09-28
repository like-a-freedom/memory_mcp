use lopdf::Document;

use super::MemoryError;

pub(crate) fn extract_text(bytes: &[u8]) -> Result<String, MemoryError> {
    let document = Document::load_mem(bytes)
        .map_err(|err| MemoryError::Validation(format!("failed to parse pdf bytes: {err}")))?;
    let page_numbers = document.get_pages().keys().copied().collect::<Vec<_>>();
    if page_numbers.is_empty() {
        return Ok(String::new());
    }

    document
        .extract_text(&page_numbers)
        .map_err(|err| MemoryError::Validation(format!("failed to extract pdf text: {err}")))
}
