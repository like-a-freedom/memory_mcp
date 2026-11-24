mod common;

use std::path::PathBuf;

use clap::Parser;
use memory_mcp::cli::args::{OperatorImportArgs, OperatorImportOperation};
use memory_mcp::cli::{Cli, Command};
use memory_mcp::knowledge::api::owned_episode_scan;
use memory_mcp::service::{MemoryError, MemoryService};

async fn episodes(service: &MemoryService) -> Result<Vec<serde_json::Value>, MemoryError> {
    let port = memory_mcp::knowledge::infra::KnowledgeReadAdapter::new(
        service.db_client_for_port(),
        service.namespace_for_port(),
    );
    owned_episode_scan(&port).await
}

fn file_args(root: PathBuf, path: PathBuf) -> OperatorImportArgs {
    OperatorImportArgs {
        operation: OperatorImportOperation::File {
            root,
            path,
            source_type: "doc".into(),
            t_ref: "2026-11-22T12:00:00Z".into(),
            policy_tags: Vec::new(),
        },
    }
}

#[test]
fn cli_exposes_explicit_import_file_operation() {
    let parsed = Cli::try_parse_from([
        "memory_mcp",
        "import",
        "file",
        "--root",
        "/tmp/source",
        "--path",
        "memo.md",
        "--source-type",
        "doc",
        "--t-ref",
        "2026-11-22T12:00:00Z",
    ])
    .expect("explicit import command should parse");

    assert!(matches!(parsed.command, Some(Command::Import(_))));
}

#[tokio::test]
async fn operator_file_import_stores_parsed_content_and_hashes_source_path()
-> Result<(), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let root = tempfile::tempdir()?;
    let source = root.path().join("memo.md");
    std::fs::write(&source, "# Maple Import\nThe release date is November 22.")?;

    memory_mcp::cli::commands::run_import(
        &service,
        file_args(root.path().to_path_buf(), "memo.md".into()),
    )
    .await?;

    let stored = episodes(&service).await?;
    assert_eq!(stored.len(), 1);
    assert!(
        stored[0]["content"]
            .as_str()
            .unwrap()
            .contains("Maple Import")
    );
    assert_ne!(
        stored[0]["source_id"].as_str(),
        Some(source.to_string_lossy().as_ref())
    );
    Ok(())
}

#[tokio::test]
async fn operator_file_import_refuses_credential_content_without_persisting()
-> Result<(), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let root = tempfile::tempdir()?;
    let secret = "sensitive-import-canary-value";
    std::fs::write(
        root.path().join("credential.md"),
        format!("# Local setup\nAPI_KEY={secret}"),
    )?;

    let error = memory_mcp::cli::commands::run_import(
        &service,
        file_args(root.path().to_path_buf(), "credential.md".into()),
    )
    .await
    .expect_err("credential-bearing imported content must be refused");

    assert!(!error.to_string().contains(secret));
    assert!(episodes(&service).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn operator_file_import_refuses_a_file_over_the_size_limit()
-> Result<(), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let root = tempfile::tempdir()?;
    std::fs::write(
        root.path().join("large.txt"),
        vec![b'x'; 4 * 1024 * 1024 + 1],
    )?;

    let error = memory_mcp::cli::commands::run_import(
        &service,
        file_args(root.path().to_path_buf(), "large.txt".into()),
    )
    .await
    .expect_err("oversized input must be refused");

    assert!(error.to_string().contains("4 MiB limit"));
    assert!(episodes(&service).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn operator_file_import_refuses_parent_traversal_without_persisting()
-> Result<(), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let root = tempfile::tempdir()?;
    let error = memory_mcp::cli::commands::run_import(
        &service,
        file_args(root.path().to_path_buf(), "../outside.txt".into()),
    )
    .await
    .expect_err("parent traversal must be rejected");

    assert!(error.to_string().contains("confined relative file path"));
    assert!(episodes(&service).await?.is_empty());
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn operator_file_import_refuses_symlink_escape_without_persisting()
-> Result<(), Box<dyn std::error::Error>> {
    use std::os::unix::fs::symlink;

    let service = common::make_service().await;
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    let secret_path = outside.path().join("outside.txt");
    std::fs::write(&secret_path, "should never be imported")?;
    symlink(&secret_path, root.path().join("escape.txt"))?;

    let result = memory_mcp::cli::commands::run_import(
        &service,
        file_args(root.path().to_path_buf(), "escape.txt".into()),
    )
    .await;

    assert!(result.is_err(), "symlink escape must be refused");
    assert!(episodes(&service).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn operator_url_import_rejects_credential_query_before_network_access()
-> Result<(), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let secret = "operator-url-secret-canary";
    let result = memory_mcp::cli::commands::run_import(
        &service,
        OperatorImportArgs {
            operation: OperatorImportOperation::Url {
                url: format!("https://example.invalid/document?access_token={secret}"),
                source_type: "web".into(),
                t_ref: "2026-11-22T12:00:00Z".into(),
                policy_tags: Vec::new(),
            },
        },
    )
    .await;

    let error = result.expect_err("credential-like query parameter must be refused");
    assert!(
        !error.to_string().contains(secret),
        "validation errors must not echo credentials"
    );
    assert!(episodes(&service).await?.is_empty());
    Ok(())
}

#[tokio::test]
async fn operator_url_import_refuses_dns_resolving_to_loopback_before_connecting()
-> Result<(), Box<dyn std::error::Error>> {
    use std::time::Duration;

    let service = common::make_service().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let result = memory_mcp::cli::commands::run_import(
        &service,
        OperatorImportArgs {
            operation: OperatorImportOperation::Url {
                url: format!("http://localhost:{port}/document"),
                source_type: "web".into(),
                t_ref: "2026-11-22T12:00:00Z".into(),
                policy_tags: Vec::new(),
            },
        },
    )
    .await;

    let error = result.expect_err("localhost DNS answers must be rejected as non-public");
    assert!(error.to_string().contains("non-public address"));
    assert!(episodes(&service).await?.is_empty());
    assert!(
        tokio::time::timeout(Duration::from_millis(250), listener.accept())
            .await
            .is_err(),
        "URL validation must reject loopback DNS answers before connecting"
    );
    Ok(())
}
