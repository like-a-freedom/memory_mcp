use crate::cli::args::ResolveArgs;
use crate::cli::commands::write_response;
use crate::service::MemoryError;
use crate::service::MemoryService;
use crate::tools::params::ResolveParams;

pub async fn run(service: &MemoryService, args: ResolveArgs) -> Result<(), MemoryError> {
    let params = ResolveParams {
        entity_type: args.entity_type,
        canonical_name: args.canonical_name,
        aliases: args.aliases,
    };
    let response = crate::tools::resolve(&service.build_context(), params).await?;
    write_response(&response).map_err(|err| MemoryError::Transient(err.to_string()))?;
    Ok(())
}
