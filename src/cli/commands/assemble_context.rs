use crate::cli::args::AssembleContextArgs;
use crate::cli::commands::write_response;
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::AssembleContextParams;

pub async fn run(service: &MemoryService, args: AssembleContextArgs) -> Result<(), MemoryError> {
    let params = AssembleContextParams {
        query: args.query,
        scope: args.scope,
        project: args.project,
        fact_types: args.fact_types,
        as_of: args.as_of,
        budget: args.budget,
        view_mode: args.view_mode,
        window_start: args.window_start,
        window_end: args.window_end,
    };
    let response = crate::tools::assemble_context(&service.build_context(), params).await?;
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))?;
    Ok(())
}
