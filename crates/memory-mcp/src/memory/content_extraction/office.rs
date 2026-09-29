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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    /// A minimal OOXML zip archive holding `entries`, in the order given.
    fn archive(entries: &[(&str, &str)]) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buffer);
            let options = SimpleFileOptions::default();
            for (name, body) in entries {
                writer.start_file(*name, options).expect("start file");
                writer.write_all(body.as_bytes()).expect("write file");
            }
            writer.finish().expect("finish archive");
        }
        buffer.into_inner()
    }

    /// A well-formed `word/document.xml` carrying `text`.
    fn document_xml(text: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><w:document xmlns:w="w"><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:document>"#
        )
    }

    /// A well-formed `xl/worksheets/sheet1.xml` carrying `text` in a cell.
    fn worksheet_xml(text: &str) -> String {
        format!(
            r#"<?xml version="1.0"?><worksheet xmlns="s"><sheetData><row><c><v>{text}</v></c></row></sheetData></worksheet>"#
        )
    }

    /// A well-formed `ppt/slides/slide1.xml` carrying `text`.
    fn slide_xml(text: &str) -> String {
        format!(r#"<?xml version="1.0"?><p:sld xmlns:p="p" xmlns:a="a"><a:t>{text}</a:t></p:sld>"#)
    }

    #[test]
    fn a_docx_yields_its_document_text() {
        let bytes = archive(&[("word/document.xml", &document_xml("hello docx"))]);

        let observed = extract_docx(&bytes).expect("docx parses");

        assert!(observed.contains("hello docx"));
    }

    #[test]
    fn a_docx_without_a_document_part_is_rejected() {
        let bytes = archive(&[("word/styles.xml", "<x/>")]);

        let observed = extract_docx(&bytes);

        assert!(
            observed.is_err(),
            "an archive with no body is not a document"
        );
    }

    #[test]
    fn bytes_that_are_not_a_zip_archive_are_rejected() {
        let observed = extract_docx(b"this is not a zip archive");

        assert!(observed.is_err());
    }

    #[test]
    fn a_docx_with_malformed_xml_is_rejected() {
        let bytes = archive(&[("word/document.xml", "<w:p>unclosed")]);

        let observed = extract_docx(&bytes);

        assert!(
            observed.is_err(),
            "a truncated body must not be silently dropped"
        );
    }

    #[test]
    fn an_xlsx_yields_its_shared_strings() {
        let bytes = archive(&[(
            "xl/sharedStrings.xml",
            r#"<?xml version="1.0"?><sst xmlns="s"><si><t>shared</t></si></sst>"#,
        )]);

        let observed = extract_xlsx(&bytes).expect("xlsx parses");

        assert!(observed.contains("shared"));
    }

    #[test]
    fn an_xlsx_yields_its_worksheet_cells() {
        let bytes = archive(&[("xl/worksheets/sheet1.xml", &worksheet_xml("42"))]);

        let observed = extract_xlsx(&bytes).expect("xlsx parses");

        assert!(observed.contains("42"));
    }

    #[test]
    fn an_xlsx_combines_shared_strings_and_cells() {
        let bytes = archive(&[
            (
                "xl/sharedStrings.xml",
                r#"<?xml version="1.0"?><sst xmlns="s"><si><t>shared</t></si></sst>"#,
            ),
            ("xl/worksheets/sheet1.xml", &worksheet_xml("42")),
        ]);

        let observed = extract_xlsx(&bytes).expect("xlsx parses");

        assert!(observed.contains("shared") && observed.contains("42"));
    }

    #[test]
    fn an_xlsx_with_no_workbook_parts_yields_no_text() {
        let bytes = archive(&[("docProps/app.xml", "<x/>")]);

        let observed = extract_xlsx(&bytes).expect("an empty workbook is not an error");

        assert!(observed.is_empty());
    }

    #[test]
    fn a_pptx_yields_its_slide_text() {
        let bytes = archive(&[("ppt/slides/slide1.xml", &slide_xml("hello slide"))]);

        let observed = extract_pptx(&bytes).expect("pptx parses");

        assert!(observed.contains("hello slide"));
    }

    #[test]
    fn a_pptx_yields_its_slides_in_name_order() {
        let bytes = archive(&[
            ("ppt/slides/slide2.xml", &slide_xml("second")),
            ("ppt/slides/slide1.xml", &slide_xml("first")),
        ]);

        let observed = extract_pptx(&bytes).expect("pptx parses");

        let first_at = observed.find("first").expect("first slide is present");
        let second_at = observed.find("second").expect("second slide is present");
        assert!(
            first_at < second_at,
            "slides are read in sorted order so the deck reads in sequence"
        );
    }

    #[test]
    fn a_pptx_ignores_non_slide_parts() {
        let bytes = archive(&[
            ("ppt/slides/slide1.xml", &slide_xml("visible")),
            ("ppt/slideLayouts/slideLayout1.xml", &slide_xml("hidden")),
        ]);

        let observed = extract_pptx(&bytes).expect("pptx parses");

        assert!(observed.contains("visible"));
        assert!(
            !observed.contains("hidden"),
            "layout parts are not slide text"
        );
    }

    #[test]
    fn a_pptx_with_no_slides_yields_no_text() {
        let bytes = archive(&[("docProps/app.xml", "<x/>")]);

        let observed = extract_pptx(&bytes).expect("an empty deck is not an error");

        assert!(observed.is_empty());
    }

    #[test]
    fn inline_whitespace_is_collapsed() {
        let observed = normalize_inline_whitespace("a   b\n\tc");

        assert_eq!(observed, "a b c");
    }
}
