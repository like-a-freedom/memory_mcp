//! CLI administrator commands.
//!
//! Two families share one adapter because both connect only to the control
//! registry through [`AdminCliConfig`], before `MemoryService`/NER construction:
//! administrator lifecycle (`create`, `recover`) and the deployment's browser
//! authentication methods (`auth-methods remove`, ADR-0057).
//!
//! Every arm resolves and guards its preconditions **before** opening the
//! registry, so a refused command neither creates nor writes one.

use crate::cli::admin_config::AdminCliConfig;
use crate::cli::args::{AdminOperation, AuthMethodsArgs};
use crate::error::MemoryError;
use crate::http::config::BrowserAuthMethod;
use crate::http::registry::{RegistryStore, SurrealRegistryStore};
use crate::service::local_admin::auth::{
    AdminManagementService, LocalAdminAuthority, compute_fingerprints,
};
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

    match operation {
        AdminOperation::AuthMethods(args) => {
            let method = parse_method(&args)?;
            guard_removal(method, &config.auth_methods, &config.operator_identities)?;
            let store = connect(&config).await?;
            remove_method(store, method).await
        }
        AdminOperation::Create { username } => {
            require_local_method(&config.auth_methods)?;
            let store = connect(&config).await?;
            let service = join_authority(&config, store).await?;
            create_admin(&config, &service, &username).await
        }
        AdminOperation::Recover { username } => {
            require_local_method(&config.auth_methods)?;
            let store = connect(&config).await?;
            let service = join_authority(&config, store).await?;
            recover_admin(&config, &service, &username).await
        }
    }
}

async fn connect(config: &AdminCliConfig) -> Result<Arc<SurrealRegistryStore>, MemoryError> {
    Ok(Arc::new(
        SurrealRegistryStore::connect(&config.control_db)
            .await
            .map_err(|e| MemoryError::Storage(format!("control registry connect: {e}")))?,
    ))
}

/// Spec §6: creating or recovering a local administrator requires the local
/// method. Writing those records in a deployment that authenticates browsers
/// through an identity provider alone would produce records nothing can use.
fn require_local_method(configured: &[BrowserAuthMethod]) -> Result<(), MemoryError> {
    if configured.contains(&BrowserAuthMethod::Local) {
        return Ok(());
    }
    Err(MemoryError::ConfigInvalid(
        "admin commands require the 'local' browser authentication method \
         (MEMORY_MCP_HTTP_AUTH_METHODS=local)"
            .into(),
    ))
}

/// The guarded removal preconditions (ADR-0057).
///
/// Two refusals, both about leaving the deployment administrable:
///
/// - The method must already be absent from the configuration. Startup
///   reconciliation is additive, so removing a method the configuration still
///   enables would be undone at the next start — the operator would believe they
///   had turned it off while the deployment kept serving it.
/// - The local method may not be removed while no operator identity is
///   configured. `local` is the only method that depends on no external service,
///   so it is how a deployment recovers from a broken provider; dropping it
///   before anyone can administer the deployment through the provider is the
///   lockout this rule exists to prevent.
fn guard_removal(
    method: BrowserAuthMethod,
    configured: &[BrowserAuthMethod],
    operator_identities: &[String],
) -> Result<(), MemoryError> {
    if configured.contains(&method) {
        return Err(MemoryError::Validation(format!(
            "'{}' is still enabled by MEMORY_MCP_HTTP_AUTH_METHODS; remove it from the \
             configuration first, or the next start would add it straight back",
            method.as_str()
        )));
    }
    if method == BrowserAuthMethod::Local && operator_identities.is_empty() {
        return Err(MemoryError::Validation(
            "removing the 'local' method would leave no operator identity able to administer \
             this deployment; configure MEMORY_MCP_HTTP_OPERATOR_IDENTITIES first, which is \
             what makes 'SSO only' safe"
                .into(),
        ));
    }
    Ok(())
}

fn parse_method(args: &AuthMethodsArgs) -> Result<BrowserAuthMethod, MemoryError> {
    let crate::cli::args::AuthMethodsOperation::Remove { method } = &args.operation;
    BrowserAuthMethod::parse(method).ok_or_else(|| {
        let known = BrowserAuthMethod::ALL
            .iter()
            .map(|known| known.as_str())
            .collect::<Vec<_>>()
            .join("', '");
        MemoryError::Validation(format!(
            "unknown browser authentication method '{method}'; expected one of '{known}'"
        ))
    })
}

/// Remove one method from the durable policy (ADR-0057).
///
/// The store narrows the set and advances the epoch in one transaction with the
/// audit row, so the command itself only reports the result and what the
/// deployment must now be configured with.
async fn remove_method(
    store: Arc<SurrealRegistryStore>,
    method: BrowserAuthMethod,
) -> Result<(), MemoryError> {
    let policy = store.remove_browser_auth_method(method).await?;
    let enabled = policy
        .methods
        .iter()
        .map(|method| method.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let output = serde_json::json!({
        "removed_method": method.as_str(),
        "enabled_methods": enabled,
        "epoch": policy.epoch.to_string(),
        "guidance": format!(
            "set MEMORY_MCP_HTTP_AUTH_METHODS={enabled} and restart; every browser session was \
             invalidated by the new epoch"
        ),
    });
    println!("{}", render_admin_output(&output)?);
    Ok(())
}

/// Reconcile the durable browser-auth policy exactly as the server does, so a
/// command never writes against a policy the server would refuse to join
/// (ADR-0057). A set that also enables `oidc` is accepted, and the same
/// fingerprints bind this write to the running deployment's keys.
async fn join_authority(
    config: &AdminCliConfig,
    store: Arc<SurrealRegistryStore>,
) -> Result<AdminManagementService, MemoryError> {
    let fingerprints =
        compute_fingerprints(&config.session_key, &config.csrf_key).map_err(admin_error)?;
    let policy = RegistryStore::reconcile_browser_policy(
        store.as_ref(),
        &config.auth_methods,
        Some(fingerprints),
    )
    .await
    .map_err(|e| MemoryError::Storage(format!("reconcile browser policy: {e}")))?;

    let authority = LocalAdminAuthority::join(store, config.session_key, config.csrf_key, policy)
        .map_err(|e| MemoryError::Storage(format!("join policy: {e}")))?;
    Ok(AdminManagementService::new(authority))
}

async fn create_admin(
    config: &AdminCliConfig,
    service: &AdminManagementService,
    username: &str,
) -> Result<(), MemoryError> {
    let normalized = crate::service::local_admin::policy::normalize_username(username)
        .map_err(|e| MemoryError::Validation(e.to_string()))?;
    let challenge = service
        .create_admin(&normalized, &request_context())
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

async fn recover_admin(
    config: &AdminCliConfig,
    service: &AdminManagementService,
    username: &str,
) -> Result<(), MemoryError> {
    let normalized = crate::service::local_admin::policy::normalize_username(username)
        .map_err(|e| MemoryError::Validation(e.to_string()))?;
    let challenge = service
        .recover_admin(&normalized, &request_context())
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

fn request_context() -> RequestContext {
    RequestContext {
        request_id: uuid::Uuid::new_v4(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::AuthMethodsOperation;

    fn remove_args(method: &str) -> AuthMethodsArgs {
        AuthMethodsArgs {
            operation: AuthMethodsOperation::Remove {
                method: method.to_string(),
            },
        }
    }

    fn operator() -> Vec<String> {
        vec!["https://idp.example|ab12".to_string()]
    }

    /// Removing a method the configuration still enables would be undone at the
    /// next start, so the operator would believe it had been turned off while
    /// the deployment kept serving it.
    #[test]
    fn removing_a_configured_method_is_refused() {
        let refused = guard_removal(
            BrowserAuthMethod::Local,
            &[BrowserAuthMethod::Local],
            &operator(),
        );
        assert!(
            matches!(
                refused,
                Err(MemoryError::Validation(ref message))
                    if message.contains("still enabled")
            ),
            "a still-configured method must be refused, got {refused:?}"
        );
    }

    /// ADR-0057: the last-administrator rule. Without an operator identity the
    /// deployment would have nobody who can administer it once `local` is gone.
    #[test]
    fn removing_local_without_an_operator_identity_is_refused() {
        let refused = guard_removal(BrowserAuthMethod::Local, &[BrowserAuthMethod::Oidc], &[]);
        assert!(
            matches!(
                refused,
                Err(MemoryError::Validation(ref message))
                    if message.contains("MEMORY_MCP_HTTP_OPERATOR_IDENTITIES")
            ),
            "the refusal must name the variable that fixes it, got {refused:?}"
        );
    }

    #[test]
    fn removing_local_with_an_operator_identity_is_allowed() {
        guard_removal(
            BrowserAuthMethod::Local,
            &[BrowserAuthMethod::Oidc],
            &operator(),
        )
        .expect("'SSO only' is the state the operator identity exists to make safe");
    }

    /// The rule is about losing the local door specifically: removing `oidc`
    /// leaves that door, so it needs no operator identity.
    #[test]
    fn removing_oidc_does_not_require_an_operator_identity() {
        guard_removal(BrowserAuthMethod::Oidc, &[BrowserAuthMethod::Local], &[])
            .expect("the local door still administers the deployment");
    }

    #[test]
    fn a_known_method_name_parses() {
        assert_eq!(
            parse_method(&remove_args("local")).expect("local is a method"),
            BrowserAuthMethod::Local
        );
        assert_eq!(
            parse_method(&remove_args("oidc")).expect("oidc is a method"),
            BrowserAuthMethod::Oidc
        );
    }

    #[test]
    fn an_unknown_method_name_is_refused_and_lists_the_known_ones() {
        let refused = parse_method(&remove_args("saml"));
        assert!(
            matches!(
                refused,
                Err(MemoryError::Validation(ref message))
                    if message.contains("'local'") && message.contains("'oidc'")
            ),
            "the refusal must list the known tokens, got {refused:?}"
        );
    }
}
