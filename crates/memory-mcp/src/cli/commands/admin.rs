//! CLI admin create/recover commands.
//!
//! Runs before MemoryService/NER construction. Connects only to
//! the control registry via AdminCliConfig.

use crate::cli::admin_config::AdminCliConfig;
use crate::cli::args::AdminOperation;
use crate::error::MemoryError;
use crate::http::registry::SurrealRegistryStore;
use crate::service::local_admin::auth::{AdminManagementService, LocalAdminAuthority};
use crate::service::local_admin::contracts::{LocalAdminError, RequestContext};
use std::sync::Arc;

/// Convert a local-admin domain failure into a safe `MemoryError` category.
///
/// The runner's formatter only ever sees this category plus the wrapper's own
/// constant Display string: `LocalAdminError` renders no storage detail, so no
/// raw database error text can reach the operator's terminal or the logs.
pub(crate) fn admin_error(error: LocalAdminError) -> MemoryError {
    match error {
        LocalAdminError::InvalidInput(message) => MemoryError::Validation(message),
        LocalAdminError::Infrastructure(_) | LocalAdminError::Unavailable => {
            MemoryError::Storage("local administrator store unavailable".into())
        }
        other => MemoryError::Auth(other.to_string()),
    }
}

pub async fn run(operation: AdminOperation) -> Result<(), MemoryError> {
    let config = AdminCliConfig::from_env()?;

    // Connect to control registry
    let store = Arc::new(
        SurrealRegistryStore::connect(&config.control_db)
            .await
            .map_err(|e| MemoryError::Storage(format!("control registry connect: {e}")))?,
    );

    let authority = LocalAdminAuthority::join(store, config.session_key, config.csrf_key)
        .await
        .map_err(|e| MemoryError::Storage(format!("join policy: {e}")))?;

    let service = AdminManagementService::new(authority);
    let request = RequestContext {
        request_id: uuid::Uuid::new_v4(),
    };

    match operation {
        AdminOperation::Create { username } => {
            // Normalize the username first
            let normalized = crate::service::local_admin::policy::normalize_username(&username)
                .map_err(|e| MemoryError::Validation(e.to_string()))?;
            let challenge = service
                .create_admin(&normalized, &request)
                .await
                .map_err(admin_error)?;
            let output = serde_json::json!({
                "admin_id": challenge.issued.admin_id,
                "username": challenge.issued.username,
                "code": challenge.code,
                "expires_at": challenge.issued.expires_at.to_rfc3339(),
                "activation_url": format!("{}/admin/activate", config.public_base_url),
            });
            println!("{}", render_admin_output(&output)?);
            Ok(())
        }
        AdminOperation::Recover { username } => {
            let normalized = crate::service::local_admin::policy::normalize_username(&username)
                .map_err(|e| MemoryError::Validation(e.to_string()))?;
            let challenge = service
                .recover_admin(&normalized, &request)
                .await
                .map_err(admin_error)?;
            let output = serde_json::json!({
                "admin_id": challenge.issued.admin_id,
                "username": challenge.issued.username,
                "code": challenge.code,
                "expires_at": challenge.issued.expires_at.to_rfc3339(),
                "reset_url": format!("{}/admin/reset", config.public_base_url),
            });
            println!("{}", render_admin_output(&output)?);
            Ok(())
        }
    }
}

/// Serialize the CLI's one-time output for stdout.
///
/// The value is a flat object of strings, so serialization cannot fail in
/// practice; the `Result` exists so the failure mode is a typed error
/// rather than a panic or a silently empty line on a secret-bearing
/// output path.
fn render_admin_output(output: &serde_json::Value) -> Result<String, MemoryError> {
    serde_json::to_string_pretty(output)
        .map_err(|e| MemoryError::Storage(format!("admin output serialization: {e}")))
}
