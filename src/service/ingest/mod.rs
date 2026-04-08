use std::fs;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::models::IngestRequest;

use super::error::MemoryError;

mod chunker;
mod email;
mod office;
mod pdf;
mod text;
#[cfg(feature = "cli-watch")]
pub mod watcher;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TextChunk {
    pub label: Option<String>,
    pub content: String,
}

impl TextChunk {
    fn new(content: String) -> Self {
        Self {
            label: None,
            content,
        }
    }
}

trait DocumentParser {
    fn can_handle(&self, extension: &str) -> bool;
    fn parse(&self, bytes: &[u8]) -> Result<Vec<TextChunk>, MemoryError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FileFormat {
    Pdf,
    Docx,
    Xlsx,
    Pptx,
    Markdown,
    Text,
    Email,
}

impl DocumentParser for FileFormat {
    fn can_handle(&self, extension: &str) -> bool {
        matches!(
            (self, extension),
            (Self::Pdf, "pdf")
                | (Self::Docx, "docx")
                | (Self::Xlsx, "xlsx")
                | (Self::Pptx, "pptx")
                | (Self::Markdown, "md")
                | (Self::Markdown, "markdown")
                | (Self::Text, "txt")
                | (Self::Email, "eml")
        )
    }

    fn parse(&self, bytes: &[u8]) -> Result<Vec<TextChunk>, MemoryError> {
        let extracted = match self {
            Self::Pdf => pdf::extract_text(bytes)?,
            Self::Docx => office::extract_docx(bytes)?,
            Self::Xlsx => office::extract_xlsx(bytes)?,
            Self::Pptx => office::extract_pptx(bytes)?,
            Self::Markdown => text::extract_markdown(bytes)?,
            Self::Text => text::extract_plain_text(bytes)?,
            Self::Email => email::extract_email(bytes)?,
        };

        Ok(vec![TextChunk::new(extracted)])
    }
}

const FILE_PARSERS: [FileFormat; 7] = [
    FileFormat::Pdf,
    FileFormat::Docx,
    FileFormat::Xlsx,
    FileFormat::Pptx,
    FileFormat::Markdown,
    FileFormat::Text,
    FileFormat::Email,
];

struct PreparedIngestContent {
    content: String,
    source_id: Option<String>,
}

pub(crate) fn detect_ingest_transport(content: &str) -> &'static str {
    let path = Path::new(content);
    if path.exists() {
        if path.is_file() {
            return "file";
        }
        if path.is_dir() {
            return "directory";
        }
    }

    if looks_like_remote_url(content) {
        "url"
    } else {
        "inline"
    }
}

pub(crate) async fn prepare_ingest_request(
    mut request: IngestRequest,
) -> Result<IngestRequest, MemoryError> {
    if let Some(prepared) = maybe_prepare_path_content(&request.content)? {
        request.content = prepared.content;
        if let Some(source_id) = prepared.source_id {
            request.source_id = source_id;
        }
        return Ok(request);
    }
    if let Some(prepared) = maybe_prepare_directory_content(&request.content)? {
        request.content = prepared.content;
        if let Some(source_id) = prepared.source_id {
            request.source_id = source_id;
        }
        return Ok(request);
    }
    if let Some(prepared) = maybe_prepare_url_content(&request.content).await? {
        request.content = prepared.content;
        if let Some(source_id) = prepared.source_id {
            request.source_id = source_id;
        }
        return Ok(request);
    }

    Ok(request)
}

#[cfg(test)]
pub(crate) fn maybe_extract_path_content(content: &str) -> Result<Option<String>, MemoryError> {
    Ok(maybe_prepare_path_content(content)?.map(|prepared| prepared.content))
}

fn maybe_prepare_path_content(content: &str) -> Result<Option<PreparedIngestContent>, MemoryError> {
    let path = Path::new(content);
    if !path.exists() || !path.is_file() {
        return Ok(None);
    }

    Ok(Some(PreparedIngestContent {
        content: extract_file_content(path)?,
        source_id: Some(stable_transport_source_id_for_path(path)?),
    }))
}

#[cfg(test)]
fn maybe_extract_directory_content(content: &str) -> Result<Option<String>, MemoryError> {
    Ok(maybe_prepare_directory_content(content)?.map(|prepared| prepared.content))
}

fn maybe_prepare_directory_content(
    content: &str,
) -> Result<Option<PreparedIngestContent>, MemoryError> {
    let path = Path::new(content);
    if !path.exists() || !path.is_dir() {
        return Ok(None);
    }

    let mut collected = Vec::new();
    collect_directory_fragments(path, path, &mut collected)?;
    if collected.is_empty() {
        return Err(MemoryError::Validation(format!(
            "no supported ingest files found in {}",
            path.display()
        )));
    }

    Ok(Some(PreparedIngestContent {
        content: normalize_extracted_text(&collected.join("\n\n")),
        source_id: Some(stable_transport_source_id_for_path(path)?),
    }))
}

async fn maybe_prepare_url_content(
    content: &str,
) -> Result<Option<PreparedIngestContent>, MemoryError> {
    if !looks_like_remote_url(content) {
        return Ok(None);
    }

    let response = reqwest::get(content).await.map_err(|err| {
        MemoryError::Validation(format!("failed to fetch ingest url {content}: {err}"))
    })?;
    let status = response.status();
    if !status.is_success() {
        return Err(MemoryError::Validation(format!(
            "failed to fetch ingest url {content}: http {status}"
        )));
    }

    let body = response.text().await.map_err(|err| {
        MemoryError::Validation(format!("failed to read ingest url body {content}: {err}"))
    })?;
    let extracted = normalize_extracted_text(&strip_html_to_text(&body));
    if extracted.is_empty() {
        return Err(MemoryError::Validation(format!(
            "no extractable text found at ingest url {content}"
        )));
    }

    Ok(Some(PreparedIngestContent {
        content: finalize_text_chunks(vec![TextChunk::new(extracted)])?,
        source_id: Some(stable_transport_source_id(content)),
    }))
}

fn extract_file_content(path: &Path) -> Result<String, MemoryError> {
    let bytes = fs::read(path).map_err(|err| {
        MemoryError::Validation(format!(
            "failed to read ingest file {}: {err}",
            path.display()
        ))
    })?;
    let format = detect_format(path, &bytes)?;
    let prepared = finalize_text_chunks(format.parse(&bytes)?)?;
    if prepared.is_empty() {
        return Err(MemoryError::Validation(format!(
            "no extractable text found in {}",
            path.display()
        )));
    }

    Ok(prepared)
}

fn detect_format(path: &Path, bytes: &[u8]) -> Result<FileFormat, MemoryError> {
    let extension = detect_extension_hint(path, bytes)?;
    FILE_PARSERS
        .into_iter()
        .find(|parser| parser.can_handle(&extension))
        .ok_or_else(|| unsupported_format(path))
}

fn detect_extension_hint(path: &Path, bytes: &[u8]) -> Result<String, MemoryError> {
    if bytes.starts_with(b"%PDF-") {
        return Ok("pdf".to_string());
    }

    path.extension()
        .and_then(|value| value.to_str())
        .map(|value| value.to_ascii_lowercase())
        .ok_or_else(|| unsupported_format(path))
}

fn finalize_text_chunks(chunks: Vec<TextChunk>) -> Result<String, MemoryError> {
    let normalized = chunks
        .into_iter()
        .filter_map(|chunk| {
            let content = normalize_extracted_text(&chunk.content);
            if content.is_empty() {
                None
            } else {
                Some(TextChunk {
                    label: chunk.label,
                    content,
                })
            }
        })
        .collect::<Vec<_>>();

    if normalized.is_empty() {
        return Ok(String::new());
    }

    Ok(chunker::render_chunks(&chunker::chunk_text(normalized)))
}

fn stable_transport_source_id_for_path(path: &Path) -> Result<String, MemoryError> {
    let canonical = path.canonicalize().map_err(|err| {
        MemoryError::Validation(format!(
            "failed to canonicalize ingest path {}: {err}",
            path.display()
        ))
    })?;
    Ok(stable_transport_source_id(
        canonical.to_string_lossy().as_ref(),
    ))
}

fn stable_transport_source_id(seed: &str) -> String {
    hex::encode(Sha256::digest(seed.as_bytes()))
}

fn normalize_extracted_text(raw: &str) -> String {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn collect_directory_fragments(
    root: &Path,
    directory: &Path,
    fragments: &mut Vec<String>,
) -> Result<(), MemoryError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|err| {
            MemoryError::Validation(format!(
                "failed to read ingest directory {}: {err}",
                directory.display()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            MemoryError::Validation(format!(
                "failed to enumerate ingest directory {}: {err}",
                directory.display()
            ))
        })?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type().map_err(|err| {
            MemoryError::Validation(format!(
                "failed to inspect ingest path {}: {err}",
                path.display()
            ))
        })?;
        if file_type.is_dir() {
            collect_directory_fragments(root, &path, fragments)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }

        if let Ok(extracted) = extract_file_content(&path) {
            let relative = path.strip_prefix(root).unwrap_or(path.as_path());
            fragments.push(format!("File: {}\n{}", relative.display(), extracted));
        }
    }

    Ok(())
}

fn looks_like_remote_url(content: &str) -> bool {
    content.starts_with("http://") || content.starts_with("https://")
}

fn strip_html_to_text(raw: &str) -> String {
    static SCRIPT_STYLE_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    static TAG_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();

    let without_scripts = SCRIPT_STYLE_RE
        .get_or_init(|| {
            regex::Regex::new(r"(?is)<(script|style)[^>]*>.*?</(script|style)>")
                .expect("script/style regex should compile")
        })
        .replace_all(raw, " ");
    let without_tags = TAG_RE
        .get_or_init(|| regex::Regex::new(r"(?is)<[^>]+>").expect("tag regex should compile"))
        .replace_all(&without_scripts, " ");

    without_tags
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
}

fn unsupported_format(path: &Path) -> MemoryError {
    MemoryError::Validation(format!(
        "unsupported ingest file format for {}",
        path.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    use chrono::Utc;
    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    fn fixture_path(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("docs")
            .join(name)
    }

    #[test]
    fn extracts_known_phrases_from_supported_fixtures() {
        let cases = [
            ("sample.pdf", "Hello World"),
            ("sample.docx", "I am a test document"),
            ("sample.xlsx", "Test spreadsheet"),
            ("sample.pptx", "Title of the first slide"),
            ("sample.md", "Maple markdown action item"),
            ("sample.eml", "Cedar email follow-up"),
        ];

        for (fixture_name, expected_phrase) in cases {
            let path = fixture_path(fixture_name);
            let extracted = maybe_extract_path_content(path.to_string_lossy().as_ref())
                .expect("fixture extraction should succeed")
                .expect("fixture path should be detected");
            assert!(
                extracted.contains(expected_phrase),
                "fixture {} should contain {:?}, got {:?}",
                path.display(),
                expected_phrase,
                extracted,
            );
        }
    }

    #[test]
    fn ignores_plain_inline_content() {
        assert!(
            maybe_extract_path_content("inline content")
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn extracts_known_phrases_from_recursive_directory() {
        let temp_dir = tempdir().expect("temp dir should exist");
        let root_markdown = temp_dir.path().join("sample.md");
        std::fs::copy(fixture_path("sample.md"), &root_markdown)
            .expect("markdown fixture should copy");

        let nested_dir = temp_dir.path().join("nested/mail");
        std::fs::create_dir_all(&nested_dir).expect("nested directory should exist");
        std::fs::copy(fixture_path("sample.eml"), nested_dir.join("sample.eml"))
            .expect("email fixture should copy");
        std::fs::write(temp_dir.path().join("ignored.json"), "{}")
            .expect("unsupported file should write");

        let extracted = maybe_extract_directory_content(temp_dir.path().to_string_lossy().as_ref())
            .expect("directory extraction should succeed")
            .expect("directory path should be detected");
        assert!(extracted.contains("Maple markdown action item"));
        assert!(extracted.contains("Cedar email follow-up"));
    }

    #[test]
    fn strips_known_phrase_from_html_fixture() {
        let html = std::fs::read_to_string(fixture_path("sample.html"))
            .expect("html fixture should be readable");
        let extracted = normalize_extracted_text(&strip_html_to_text(&html));
        assert!(extracted.contains("Aspen url ingest briefing"));
    }

    #[tokio::test]
    async fn prepare_ingest_request_rewrites_file_source_id_to_canonical_path_hash() {
        let temp_dir = tempdir().expect("temp dir should exist");
        let file_path = temp_dir.path().join("memo.txt");
        std::fs::write(&file_path, "spruce follow-up memo")
            .expect("fixture file should be writable");

        let canonical = file_path
            .canonicalize()
            .expect("file path should canonicalize");
        let expected_hash = hex::encode(Sha256::digest(canonical.to_string_lossy().as_bytes()));

        let prepared = prepare_ingest_request(IngestRequest {
            source_type: "document".to_string(),
            source_id: "caller-provided".to_string(),
            content: file_path.to_string_lossy().into_owned(),
            t_ref: Utc::now(),
            scope: "org".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        })
        .await
        .expect("file ingest request should prepare");

        assert_eq!(prepared.source_id, expected_hash);
        assert!(prepared.content.contains("spruce follow-up memo"));
    }

    #[tokio::test]
    async fn prepare_ingest_request_chunks_large_file_content_with_overlap() {
        let temp_dir = tempdir().expect("temp dir should exist");
        let file_path = temp_dir.path().join("long-note.txt");
        let content = (1..=450)
            .map(|index| format!("w{index:04}"))
            .collect::<Vec<_>>()
            .join(" ");
        std::fs::write(&file_path, content).expect("long fixture file should be writable");

        let prepared = prepare_ingest_request(IngestRequest {
            source_type: "document".to_string(),
            source_id: "caller-provided".to_string(),
            content: file_path.to_string_lossy().into_owned(),
            t_ref: Utc::now(),
            scope: "org".to_string(),
            project: None,
            t_ingested: None,
            visibility_scope: None,
            policy_tags: vec![],
        })
        .await
        .expect("long file ingest request should prepare");

        assert!(prepared.content.contains("Chunk 1/2"));
        assert!(prepared.content.contains("Chunk 2/2"));
        assert!(prepared.content.contains("w0351 w0352 w0353"));
        assert!(prepared.content.contains("w0401 w0402 w0403"));
    }
}
