use crate::cli::args::ExplainArgs;
use crate::cli::commands::write_response;
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::ExplainParams;

pub async fn run(service: &MemoryService, args: ExplainArgs) -> Result<(), MemoryError> {
    let params = ExplainParams {
        context_items: args.context_items,
        compact: crate::tools::parsers::default_compact(),
    };
    let response = crate::tools::explain(&service.build_context(), params).await?;
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))?;
    Ok(())
}
