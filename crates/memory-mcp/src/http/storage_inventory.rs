//! Read-only inventory of the HTTP profile's configured storage targets.
//!
//! Embedded stores are exclusive filesystem resources. Resolve both targets
//! before connecting either one so aliases and nested directories cannot cause
//! partial startup side effects.

use std::path::{Path, PathBuf};

use crate::config::SurrealTargetConfig;
use crate::error::MemoryError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HttpStorageInventory {
    pub(crate) share_control_engine_with_tenants: bool,
}

#[derive(Debug, PartialEq, Eq)]
enum StorageTarget {
    Embedded(PathBuf),
    Memory,
    Remote,
}

pub(crate) fn preflight(
    control: &SurrealTargetConfig,
    tenant: &SurrealTargetConfig,
) -> Result<HttpStorageInventory, MemoryError> {
    validate_binding(control)?;
    validate_binding(tenant)?;

    let control_target = classify_target(control)?;
    let tenant_target = classify_target(tenant)?;

    let share_control_engine_with_tenants = match (&control_target, &tenant_target) {
        (StorageTarget::Embedded(control_path), StorageTarget::Embedded(tenant_path)) => {
            if control_path == tenant_path {
                if !same_credentials(control, tenant) {
                    return Err(MemoryError::ConfigInvalid(
                        "control and tenant targets use the same SurrealKV path with different credentials".into(),
                    ));
                }
                true
            } else {
                if control_path.starts_with(tenant_path) || tenant_path.starts_with(control_path) {
                    return Err(MemoryError::ConfigInvalid(
                        "control and tenant SurrealKV paths must not overlap".into(),
                    ));
                }
                false
            }
        }
        (StorageTarget::Memory, StorageTarget::Memory)
        | (StorageTarget::Remote, StorageTarget::Remote) => {
            control.url.trim() == tenant.url.trim() && same_credentials(control, tenant)
        }
        _ => false,
    };

    let same_binding = control.namespace.trim() == tenant.namespace.trim()
        && control.database.trim() == tenant.database.trim();
    let same_target = match (&control_target, &tenant_target) {
        (StorageTarget::Embedded(control_path), StorageTarget::Embedded(tenant_path)) => {
            control_path == tenant_path
        }
        (StorageTarget::Memory, StorageTarget::Memory) => {
            control.url.trim() == tenant.url.trim() && same_credentials(control, tenant)
        }
        (StorageTarget::Remote, StorageTarget::Remote) => control.url.trim() == tenant.url.trim(),
        _ => false,
    };
    if same_binding && same_target {
        return Err(MemoryError::ConfigInvalid(
            "control and tenant storage must use different namespace/database bindings".into(),
        ));
    }

    Ok(HttpStorageInventory {
        share_control_engine_with_tenants,
    })
}

fn validate_binding(target: &SurrealTargetConfig) -> Result<(), MemoryError> {
    if !super::registry::storage::is_safe_identifier(target.namespace.trim()) {
        return Err(MemoryError::Validation(
            "registry namespace is not a safe identifier".into(),
        ));
    }
    if !super::registry::storage::is_safe_identifier(target.database.trim()) {
        return Err(MemoryError::Validation(
            "registry database is not a safe identifier".into(),
        ));
    }
    Ok(())
}

fn classify_target(target: &SurrealTargetConfig) -> Result<StorageTarget, MemoryError> {
    let url = target.url.trim();
    if url == "mem" || url == "mem://" {
        return Ok(StorageTarget::Memory);
    }

    if let Some(path) = url
        .strip_prefix("surrealkv://")
        .or_else(|| url.strip_prefix("rocksdb://"))
    {
        if path.trim().is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "SurrealKV storage path must not be empty".into(),
            ));
        }
        return canonical_store_path(Path::new(path)).map(StorageTarget::Embedded);
    }

    if ["ws://", "wss://"]
        .iter()
        .any(|scheme| url.starts_with(scheme) && !url[scheme.len()..].trim().is_empty())
    {
        if target.username.trim().is_empty() || target.password.trim().is_empty() {
            return Err(MemoryError::ConfigInvalid(
                "remote SurrealDB credentials must not be empty".into(),
            ));
        }
        return Ok(StorageTarget::Remote);
    }

    Err(MemoryError::ConfigInvalid(
        "unsupported SurrealDB storage URL scheme".into(),
    ))
}

fn same_credentials(left: &SurrealTargetConfig, right: &SurrealTargetConfig) -> bool {
    left.username == right.username && left.password == right.password
}

fn canonical_store_path(path: &Path) -> Result<PathBuf, MemoryError> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|error| {
                MemoryError::ConfigInvalid(format!(
                    "could not resolve the current directory for SurrealKV storage: {error}"
                ))
            })?
            .join(path)
    };
    let mut ancestor = absolute;
    let mut missing_components = Vec::new();
    loop {
        match std::fs::symlink_metadata(&ancestor) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = ancestor.file_name() else {
                    return Err(MemoryError::ConfigInvalid(
                        "SurrealKV storage path has no existing ancestor".into(),
                    ));
                };
                missing_components.push(name.to_os_string());
                if !ancestor.pop() {
                    return Err(MemoryError::ConfigInvalid(
                        "SurrealKV storage path has no existing ancestor".into(),
                    ));
                }
            }
            Err(error) => {
                return Err(MemoryError::ConfigInvalid(format!(
                    "could not inspect SurrealKV storage path: {error}"
                )));
            }
        }
    }

    let mut canonical = std::fs::canonicalize(&ancestor).map_err(|error| {
        MemoryError::ConfigInvalid(format!(
            "could not resolve an existing ancestor of the SurrealKV storage path: {error}"
        ))
    })?;
    let ancestor_metadata = std::fs::metadata(&canonical).map_err(|error| {
        MemoryError::ConfigInvalid(format!(
            "could not inspect an existing ancestor of the SurrealKV storage path: {error}"
        ))
    })?;
    if !ancestor_metadata.is_dir() {
        return Err(MemoryError::ConfigInvalid(
            "SurrealKV storage path must be inside a directory".into(),
        ));
    }
    for component in missing_components.iter().rev() {
        canonical.push(component);
    }

    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(url: String) -> SurrealTargetConfig {
        SurrealTargetConfig {
            url,
            username: "root".into(),
            password: "root".into(),
            database: "memory".into(),
            namespace: "main".into(),
        }
    }

    #[test]
    fn separate_local_paths_are_independent_storage_targets() {
        let directory = tempfile::tempdir().expect("temporary storage parent");
        let control_path = directory.path().join("control");
        let tenant_path = directory.path().join("tenant");

        let inventory = preflight(
            &target(format!("surrealkv://{}", control_path.display())),
            &target(format!("rocksdb://{}", tenant_path.display())),
        )
        .expect("separate local targets should be accepted");

        assert!(!inventory.share_control_engine_with_tenants);
        assert!(!control_path.exists());
        assert!(!tenant_path.exists());
    }

    #[test]
    fn mixed_local_and_remote_targets_use_independent_engines() {
        let directory = tempfile::tempdir().expect("temporary storage parent");
        let control_path = directory.path().join("control");

        let inventory = preflight(
            &target(format!("surrealkv://{}", control_path.display())),
            &target("wss://db.example.test".into()),
        )
        .expect("mixed local and remote targets should be accepted");

        assert!(!inventory.share_control_engine_with_tenants);
        assert!(!control_path.exists());
    }

    #[test]
    fn http_remote_urls_are_rejected_when_the_registry_connector_does_not_support_them() {
        let control = target("http://db.example.test".into());
        let tenant = target("https://db.example.test".into());

        let error = preflight(&control, &tenant)
            .expect_err("HTTP schemes must not pass preflight without a matching connector");

        assert!(
            error
                .to_string()
                .contains("unsupported SurrealDB storage URL scheme")
        );
    }

    #[cfg(unix)]
    #[test]
    fn store_paths_resolve_symlinks_before_parent_components() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary storage parent");
        let actual_parent = directory.path().join("actual").join("nested");
        std::fs::create_dir_all(&actual_parent).expect("create symlink target");
        let alias = directory.path().join("alias");
        symlink(&actual_parent, &alias).expect("create symlink");
        let control_path = alias.join("..").join("shared");
        let tenant_path = actual_parent
            .parent()
            .expect("parent directory")
            .join("shared");
        let control = target(format!("surrealkv://{}", control_path.display()));
        let mut tenant = target(format!("rocksdb://{}", tenant_path.display()));
        tenant.namespace = "tenant".into();
        tenant.database = "tenant".into();

        let inventory = preflight(&control, &tenant)
            .expect("paths resolving to one physical store should be recognized as aliases");

        assert!(inventory.share_control_engine_with_tenants);
    }

    #[test]
    fn identical_local_bindings_are_rejected_before_engine_connection() {
        let directory = tempfile::tempdir().expect("temporary storage parent");
        let path = directory.path().join("shared");
        let target = target(format!("surrealkv://{}", path.display()));

        let error = preflight(&target, &target)
            .expect_err("control and tenant cannot share one namespace/database binding");

        assert!(
            error
                .to_string()
                .contains("different namespace/database bindings")
        );
        assert!(!path.exists());
    }
}
