use std::io::{Cursor, Read, Seek};

use roxmltree::Document;
use zip::ZipArchive;
use zip::result::ZipError;

use super::MemoryError;

pub(crate) fn extract_docx(bytes: &[u8]) -> Result<String, MemoryError> {
    let mut archive = open_archive(bytes)?;
    let xml = read_named_file(&mut archive, "word/document.xml")?.ok_or_else(|| {
        MemoryError::Validation("docx archive is missing word/document.xml".to_string())
    })?;
    Ok(collect_xml_text(&xml)?.join("\n"))
}

pub(crate) fn extract_xlsx(bytes: &[u8]) -> Result<String, MemoryError> {
    let mut archive = open_archive(bytes)?;
    let mut fragments = Vec::new();

    if let Some(shared_strings) = read_named_file(&mut archive, "xl/sharedStrings.xml")? {
        fragments.extend(collect_xml_text(&shared_strings)?);
    }

    for worksheet_name in archive_file_names(&mut archive, "xl/worksheets/", ".xml")? {
        if let Some(xml) = read_named_file(&mut archive, &worksheet_name)? {
            fragments.extend(collect_xml_text(&xml)?);
        }
    }

    Ok(fragments.join("\n"))
}

pub(crate) fn extract_pptx(bytes: &[u8]) -> Result<String, MemoryError> {
    let mut archive = open_archive(bytes)?;
    let mut fragments = Vec::new();

    for slide_name in archive_file_names(&mut archive, "ppt/slides/slide", ".xml")? {
        if let Some(xml) = read_named_file(&mut archive, &slide_name)? {
            fragments.extend(collect_xml_text(&xml)?);
        }
    }

    Ok(fragments.join("\n"))
}

fn open_archive(bytes: &[u8]) -> Result<ZipArchive<Cursor<&[u8]>>, MemoryError> {
    ZipArchive::new(Cursor::new(bytes))
        .map_err(|err| MemoryError::Validation(format!("failed to open OOXML archive: {err}")))
}

fn read_named_file<R>(
    archive: &mut ZipArchive<R>,
    name: &str,
) -> Result<Option<String>, MemoryError>
where
    R: Read + Seek,
{
    match archive.by_name(name) {
        Ok(mut file) => {
            let mut contents = String::new();
            file.read_to_string(&mut contents).map_err(|err| {
                MemoryError::Validation(format!("failed to read archive member {name}: {err}"))
            })?;
            Ok(Some(contents))
        }
        Err(ZipError::FileNotFound) => Ok(None),
        Err(err) => Err(MemoryError::Validation(format!(
            "failed to access archive member {name}: {err}"
        ))),
    }
}

fn archive_file_names<R>(
    archive: &mut ZipArchive<R>,
    prefix: &str,
    suffix: &str,
) -> Result<Vec<String>, MemoryError>
where
    R: Read + Seek,
{
    let mut names = Vec::new();
    for index in 0..archive.len() {
        let name = {
            let file = archive.by_index(index).map_err(|err| {
                MemoryError::Validation(format!("failed to inspect archive entry {index}: {err}"))
            })?;
            file.name().to_string()
        };
        if name.starts_with(prefix) && name.ends_with(suffix) {
            names.push(name);
        }
    }
    names.sort();
    Ok(names)
}

fn collect_xml_text(xml: &str) -> Result<Vec<String>, MemoryError> {
    let document = Document::parse(xml)
        .map_err(|err| MemoryError::Validation(format!("failed to parse xml payload: {err}")))?;

    Ok(document
        .descendants()
        .filter_map(|node| node.text())
        .map(normalize_inline_whitespace)
        .filter(|text| !text.is_empty())
        .collect())
}

fn normalize_inline_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
