use crate::cli::args::IngestArgs;
use crate::cli::commands::write_response;
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::IngestParams;

pub async fn run(service: &MemoryService, args: IngestArgs) -> Result<(), MemoryError> {
    let params = IngestParams {
        source_type: args.source_type,
        source_id: args.source_id,
        content: args.content,
        t_ref: args.t_ref,
        scope: args.scope,
        project: args.project,
        t_ingested: args.t_ingested,
        visibility_scope: args.visibility_scope,
        policy_tags: args.policy_tags,
    };
    let response = crate::tools::ingest(service, params).await?;
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))?;
    Ok(())
}
