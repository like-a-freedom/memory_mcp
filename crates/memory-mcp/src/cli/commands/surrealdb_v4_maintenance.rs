use std::sync::Arc;

use crate::bootstrap::stdio::apply_primary_catalog_for_maintenance;
use crate::cli::args::SurrealdbV4MaintenanceArgs;
use crate::config::SurrealConfig;
use crate::error::MemoryError;
use crate::storage::SurrealDbClient;
use crate::storage::v4_maintenance::RecoveryAuthorization;

pub async fn run(args: SurrealdbV4MaintenanceArgs) -> Result<(), MemoryError> {
    let config = SurrealConfig::from_env()?;
    let namespace = config.active_namespace().as_str().to_owned();
    let database = config.db_name.clone();
    let client = Arc::new(SurrealDbClient::connect(&config).await?);
    let recovery = if args.recover_ambiguous && args.confirm_no_prior_submitter_can_resume {
        RecoveryAuthorization::ConfirmNoPriorSubmitterCanResume
    } else {
        RecoveryAuthorization::None
    };

    apply_primary_catalog_for_maintenance(
        &client,
        &namespace,
        !config.embedded,
        config.allow_remote_v4_index_rebuild,
        recovery,
    )
    .await?;

    super::write_response(&serde_json::json!({
        "status": "completed",
        "role": args.role,
        "namespace": namespace,
        "database": database,
    }))
    .map_err(|error| MemoryError::Transient(error.to_string()))
}
