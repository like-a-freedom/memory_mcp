use crate::cli::args::ExplainArgs;
use crate::cli::commands::write_response;
use crate::error::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::ExplainParams;

pub async fn run(service: &MemoryService, args: ExplainArgs) -> Result<(), MemoryError> {
    let params = ExplainParams {
        context_items: args.context_items,
        compact: crate::tools::parsers::default_compact(),
        item_offset: args.item_offset,
        source_offset: args.source_offset,
        source_limit_scalars: args.source_limit_scalars,
        provenance_offset: args.provenance_offset,
    };
    let response = crate::tools::explain(service, params).await?;
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))?;
    Ok(())
}
