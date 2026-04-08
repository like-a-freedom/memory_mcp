mod common;

use std::fs;
use std::path::Path;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use memory_mcp::models::{AssembleContextRequest, IngestRequest};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Debug, Clone, Copy)]
enum DocumentIngestInput {
    Fixture(&'static str),
    UrlFixture(&'static str),
    DirectoryRecursive,
}

#[derive(Debug)]
struct DocumentIngestEvalCase {
    id: &'static str,
    description: &'static str,
    source_type: &'static str,
    input: DocumentIngestInput,
    query: &'static str,
    must_contain: &'static str,
}

#[derive(Debug, Default)]
struct DocumentIngestSummary {
    total_cases: usize,
    passed_cases: usize,
}

struct PreparedCaseInput {
    content: String,
    display: String,
    temp_dir: Option<TempDir>,
    server_task: Option<tokio::task::JoinHandle<()>>,
}

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("docs")
        .join(name)
}

fn eval_cases() -> Vec<DocumentIngestEvalCase> {
    vec![
        DocumentIngestEvalCase {
            id: "pdf-hello-world",
            description: "pdf fixture should expose Hello World through ingest dispatch",
            source_type: "document",
            input: DocumentIngestInput::Fixture("sample.pdf"),
            query: "Hello World",
            must_contain: "Hello World",
        },
        DocumentIngestEvalCase {
            id: "docx-page-one",
            description: "docx fixture should expose paragraph text through ingest dispatch",
            source_type: "document",
            input: DocumentIngestInput::Fixture("sample.docx"),
            query: "I am a test document",
            must_contain: "I am a test document",
        },
        DocumentIngestEvalCase {
            id: "xlsx-shared-strings",
            description: "xlsx fixture should expose shared string text through ingest dispatch",
            source_type: "document",
            input: DocumentIngestInput::Fixture("sample.xlsx"),
            query: "Test spreadsheet",
            must_contain: "Test spreadsheet",
        },
        DocumentIngestEvalCase {
            id: "pptx-slide-title",
            description: "pptx fixture should expose slide text through ingest dispatch",
            source_type: "document",
            input: DocumentIngestInput::Fixture("sample.pptx"),
            query: "Title of the first slide",
            must_contain: "Title of the first slide",
        },
        DocumentIngestEvalCase {
            id: "markdown-known-phrase",
            description: "markdown fixture should expose inline text through ingest dispatch",
            source_type: "document",
            input: DocumentIngestInput::Fixture("sample.md"),
            query: "Maple markdown action item",
            must_contain: "Maple markdown action item",
        },
        DocumentIngestEvalCase {
            id: "eml-body-text",
            description: "email fixture should expose body text through ingest dispatch",
            source_type: "email",
            input: DocumentIngestInput::Fixture("sample.eml"),
            query: "Cedar email follow-up",
            must_contain: "Cedar email follow-up",
        },
        DocumentIngestEvalCase {
            id: "url-html-headline",
            description: "url ingest should fetch HTML and expose stripped text through ingest dispatch",
            source_type: "document",
            input: DocumentIngestInput::UrlFixture("sample.html"),
            query: "Aspen url ingest briefing",
            must_contain: "Aspen url ingest briefing",
        },
        DocumentIngestEvalCase {
            id: "dir-recursive-email",
            description: "directory ingest should recurse into nested supported files and expose their text",
            source_type: "document",
            input: DocumentIngestInput::DirectoryRecursive,
            query: "Cedar email follow-up",
            must_contain: "Cedar email follow-up",
        },
    ]
}

async fn prepare_case_input(case: &DocumentIngestEvalCase) -> PreparedCaseInput {
    match case.input {
        DocumentIngestInput::Fixture(name) => {
            let path = fixture_path(name);
            assert!(path.exists(), "fixture {:?} should exist", path);
            PreparedCaseInput {
                content: path.to_string_lossy().into_owned(),
                display: path.display().to_string(),
                temp_dir: None,
                server_task: None,
            }
        }
        DocumentIngestInput::UrlFixture(name) => prepare_url_case_input(name).await,
        DocumentIngestInput::DirectoryRecursive => prepare_directory_case_input(),
    }
}

async fn prepare_url_case_input(fixture_name: &str) -> PreparedCaseInput {
    let body_path = fixture_path(fixture_name);
    let body = fs::read_to_string(&body_path).expect("url fixture should be readable");
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("test http listener should bind");
    let address = listener
        .local_addr()
        .expect("test http listener should expose local address");
    let response_body = body.clone();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener
            .accept()
            .await
            .expect("test http listener should accept one request");
        let mut request_buffer = [0_u8; 2048];
        let _ = stream.read(&mut request_buffer).await;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body,
        );
        stream
            .write_all(response.as_bytes())
            .await
            .expect("test http server should write response");
    });

    PreparedCaseInput {
        content: format!("http://{address}/fixture.html"),
        display: body_path.display().to_string(),
        temp_dir: None,
        server_task: Some(server_task),
    }
}

fn prepare_directory_case_input() -> PreparedCaseInput {
    let temp_dir = tempfile::tempdir().expect("temp dir should be created");
    copy_fixture_into_dir("sample.md", temp_dir.path(), "sample.md");
    copy_fixture_into_dir("sample.eml", temp_dir.path(), "nested/mail/sample.eml");
    fs::write(
        temp_dir.path().join("ignored.json"),
        r#"{"ignored":true,"reason":"unsupported extension"}"#,
    )
    .expect("unsupported fixture should be writable");

    PreparedCaseInput {
        content: temp_dir.path().to_string_lossy().into_owned(),
        display: temp_dir.path().display().to_string(),
        temp_dir: Some(temp_dir),
        server_task: None,
    }
}

fn copy_fixture_into_dir(fixture_name: &str, root: &Path, relative_target: &str) {
    let source = fixture_path(fixture_name);
    let destination = root.join(relative_target);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).expect("fixture parent directories should be created");
    }
    fs::copy(&source, &destination).expect("fixture should be copied into temp dir");
}

#[tokio::test]
#[ignore]
async fn run_document_ingest_evals() {
    let cases = eval_cases();
    let mut summary = DocumentIngestSummary::default();

    for case in cases {
        summary.total_cases += 1;
        let service = common::make_service().await;
        let prepared = prepare_case_input(&case).await;

        service
            .ingest(
                IngestRequest {
                    source_type: case.source_type.to_string(),
                    source_id: format!("fixture:{}", case.id),
                    content: prepared.content.clone(),
                    t_ref: "2026-04-07T10:00:00Z"
                        .parse::<DateTime<Utc>>()
                        .expect("static timestamp should parse"),
                    scope: "org".to_string(),
                    project: None,
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
            .unwrap_or_else(|err| panic!("case {} failed to ingest: {err}", case.id));

        let items = service
            .assemble_context(AssembleContextRequest {
                query: case.query.to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now() + chrono::Duration::seconds(1)),
                budget: 5,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            })
            .await
            .unwrap_or_else(|err| panic!("case {} failed to assemble: {err}", case.id));

        let retrieved_contents = items
            .iter()
            .map(|item| item.content.as_str())
            .collect::<Vec<_>>();
        let case_passed = retrieved_contents
            .iter()
            .any(|content| content.contains(case.must_contain));

        if case_passed {
            summary.passed_cases += 1;
        }

        if let Some(server_task) = prepared.server_task {
            if server_task.is_finished() {
                server_task
                    .await
                    .expect("test http server should finish cleanly");
            } else {
                server_task.abort();
            }
        }

        let _ = prepared.temp_dir.as_ref();
        assert!(
            case_passed,
            "case {} ({}) failed: input={} query={:?} expected_fragment={:?} retrieved_contents={:?}",
            case.id,
            case.description,
            prepared.display,
            case.query,
            case.must_contain,
            retrieved_contents,
        );
    }

    println!(
        "suite=eval_document_ingest total={} passed={}",
        summary.total_cases, summary.passed_cases,
    );
}
