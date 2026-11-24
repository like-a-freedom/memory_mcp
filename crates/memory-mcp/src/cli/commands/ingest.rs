use std::io::Read;

use crate::cli::args::IngestArgs;
use crate::cli::commands::write_response;
use crate::error::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::IngestParams;

const MAX_STDIN_CONTENT_BYTES: u64 = 8 * 1024 * 1024;

pub async fn run(service: &MemoryService, args: IngestArgs) -> Result<(), MemoryError> {
    let content = match args.content {
        Some(content) => content,
        None if args.content_stdin => tokio::task::spawn_blocking(read_stdin_content)
            .await
            .map_err(|error| MemoryError::Transient(error.to_string()))??,
        None => {
            return Err(MemoryError::Validation(
                "episode content is required".into(),
            ));
        }
    };
    let params = IngestParams {
        source_type: args.source_type,
        source_id: args.source_id,
        content,
        t_ref: args.t_ref,
        t_ingested: args.t_ingested,
        policy_tags: args.policy_tags,
        source_group: None,
        source_position: None,
    };
    let response = crate::tools::ingest(service, params).await?;
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))?;
    Ok(())
}

fn read_stdin_content() -> Result<String, MemoryError> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .lock()
        .take(MAX_STDIN_CONTENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| MemoryError::Validation("could not read episode content from stdin".into()))?;
    if bytes.len() as u64 > MAX_STDIN_CONTENT_BYTES {
        return Err(MemoryError::Validation(
            "stdin episode content exceeds 8 MiB".into(),
        ));
    }
    String::from_utf8(bytes)
        .map_err(|_| MemoryError::Validation("stdin episode content must be UTF-8".into()))
}
