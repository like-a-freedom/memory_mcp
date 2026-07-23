use crate::cli::args::ExtractArgs;
use crate::cli::commands::write_response;
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::ExtractParams;

pub async fn run(service: &MemoryService, args: ExtractArgs) -> Result<(), MemoryError> {
    let params = ExtractParams {
        episode_id: args.episode_id,
        content: args.content,
        text: args.text,
        source_type: args.source_type,
        source_id: args.source_id,
        t_ref: args.t_ref,
        scope: args.scope,
        zero_shot_labels: args.zero_shot_labels,
    };
    let response = crate::tools::extract(&service.build_context(), params).await?;
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))?;
    Ok(())
}
