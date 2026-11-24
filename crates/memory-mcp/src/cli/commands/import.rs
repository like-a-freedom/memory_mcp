//! Explicit local-operator source imports.
//!
//! This adapter is the only CLI path that may read a caller-selected file or
//! fetch a caller-selected URL. MCP ingestion remains literal because the
//! shared writer receives only already-authorized content.

use std::io::Read;
use std::path::{Component, Path};

use cap_std::ambient_authority;
use cap_std::fs::Dir;

use crate::cli::args::{OperatorImportArgs, OperatorImportOperation};
use crate::cli::commands::write_response;
use crate::error::MemoryError;
use crate::memory::content_extraction::{
    fetch_operator_url_content, parse_operator_import_file, stable_transport_source_id,
};
use crate::service::MemoryService;
use crate::tools::params::IngestParams;

const MAX_OPERATOR_FILE_BYTES: u64 = 4 * 1024 * 1024;

pub async fn run(service: &MemoryService, args: OperatorImportArgs) -> Result<(), MemoryError> {
    let (source_type, source_id, content, t_ref, policy_tags) = match args.operation {
        OperatorImportOperation::File {
            root,
            path,
            source_type,
            t_ref,
            policy_tags,
        } => {
            let bytes = read_root_confined_file(&root, &path)?;
            let content = parse_operator_import_file(&path, &bytes)?;
            let source_key = format!("file:{}:{}", root.display(), path.display());
            (
                source_type,
                stable_transport_source_id(&source_key),
                content,
                t_ref,
                policy_tags,
            )
        }
        OperatorImportOperation::Url {
            url,
            source_type,
            t_ref,
            policy_tags,
        } => {
            let content = fetch_operator_url_content(&url).await?;
            let source_key = format!("url:{url}");
            (
                source_type,
                stable_transport_source_id(&source_key),
                content,
                t_ref,
                policy_tags,
            )
        }
    };

    let response = crate::tools::ingest(
        service,
        IngestParams {
            source_type,
            source_id,
            content,
            t_ref,
            t_ingested: None,
            policy_tags,
            source_group: None,
            source_position: None,
        },
    )
    .await?;
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))
}

fn read_root_confined_file(root: &Path, relative_path: &Path) -> Result<Vec<u8>, MemoryError> {
    if !root.is_absolute() || !is_safe_relative_path(relative_path) {
        return Err(MemoryError::Validation(
            "operator import requires an absolute root and a confined relative file path".into(),
        ));
    }

    let directory = Dir::open_ambient_dir(root, ambient_authority())
        .map_err(|_| MemoryError::Validation("operator import root could not be opened".into()))?;
    let file = directory
        .open(relative_path)
        .map_err(|_| MemoryError::Validation("operator import file is unavailable".into()))?;
    let metadata = file
        .metadata()
        .map_err(|_| MemoryError::Validation("operator import file is unavailable".into()))?;
    if !metadata.is_file() {
        return Err(MemoryError::Validation(
            "operator import target must be a regular file".into(),
        ));
    }
    if metadata.len() > MAX_OPERATOR_FILE_BYTES {
        return Err(MemoryError::Validation(
            "operator import file exceeds the 4 MiB limit".into(),
        ));
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_OPERATOR_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| MemoryError::Validation("operator import file could not be read".into()))?;
    if bytes.len() as u64 > MAX_OPERATOR_FILE_BYTES {
        return Err(MemoryError::Validation(
            "operator import file exceeds the 4 MiB limit".into(),
        ));
    }
    Ok(bytes)
}

fn is_safe_relative_path(path: &Path) -> bool {
    let mut saw_normal_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => saw_normal_component = true,
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return false;
            }
        }
    }
    saw_normal_component
}
