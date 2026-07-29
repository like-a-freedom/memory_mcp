use crate::cli::args::InvalidateArgs;
use crate::cli::commands::write_response;
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::InvalidateParams;

pub async fn run(service: &MemoryService, args: InvalidateArgs) -> Result<(), MemoryError> {
    let params = InvalidateParams {
        fact_id: args.fact_id,
        reason: args.reason,
        t_invalid: args.t_invalid,
    };
    let response = crate::tools::invalidate(&service.build_context(), params).await?;
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))?;
    Ok(())
}
