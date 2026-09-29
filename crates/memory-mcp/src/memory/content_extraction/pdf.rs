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

#[cfg(test)]
mod tests {
    use super::*;

    /// The real `sample.pdf` fixture, which carries the phrase "Hello World".
    fn fixture() -> Vec<u8> {
        std::fs::read(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests")
                .join("fixtures")
                .join("docs")
                .join("sample.pdf"),
        )
        .expect("pdf fixture is readable")
    }

    #[test]
    fn a_pdf_yields_its_text() {
        let observed = extract_text(&fixture()).expect("pdf parses");

        assert!(observed.contains("Hello World"));
    }

    #[test]
    fn bytes_that_are_not_a_pdf_are_rejected() {
        let observed = extract_text(b"this is not a pdf at all");

        assert!(
            matches!(observed, Err(MemoryError::Validation(_))),
            "a non-PDF must be refused, not silently ingested as empty"
        );
    }

    #[test]
    fn a_truncated_pdf_is_rejected() {
        let mut truncated = fixture();
        truncated.truncate(truncated.len() / 2);

        let observed = extract_text(&truncated);

        assert!(observed.is_err(), "a truncated body must not read as empty");
    }

    #[test]
    fn an_empty_byte_slice_is_rejected() {
        let observed = extract_text(b"");

        assert!(observed.is_err());
    }
}
