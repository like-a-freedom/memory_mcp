//! Filesystem-ingestion configuration (`MEMORY_INGESTION_INBOX`).
//!
//! When the variable is absent, filesystem ingestion is disabled and startup
//! behavior is unchanged. When present, it must be a non-empty absolute path
//! to an existing readable directory that is not a symlink.

use std::path::{Path, PathBuf};

use crate::error::MemoryError;

/// Environment variable that enables filesystem ingestion inside `serve`.
#[allow(dead_code)]
pub const ENV_INGESTION_INBOX: &str = "MEMORY_INGESTION_INBOX";

/// Validated filesystem-ingestion configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub struct FsWatchConfig {
    pub inbox: PathBuf,
}

/// Parses `MEMORY_INGESTION_INBOX` from the environment.
///
/// Returns `Ok(None)` when the variable is absent. A configured value must be
/// a non-empty absolute path to an existing readable directory that is not a
/// symlink; shell constructs such as `~` and `$HOME` are never expanded.
#[allow(dead_code)]
pub fn from_env() -> Result<Option<FsWatchConfig>, MemoryError> {
    let Some(raw) = std::env::var_os(ENV_INGESTION_INBOX) else {
        return Ok(None);
    };
    let value = raw.to_string_lossy();
    if value.trim().is_empty() {
        return Err(MemoryError::ConfigInvalid(format!(
            "{ENV_INGESTION_INBOX} must be a non-empty absolute directory path"
        )));
    }
    let inbox = PathBuf::from(raw);
    validate_inbox_root(&inbox)?;
    #[cfg(not(feature = "fs-watch"))]
    return Err(MemoryError::ConfigInvalid(format!(
        "{ENV_INGESTION_INBOX} is set, but this binary was built without the fs-watch feature"
    )));
    #[cfg(feature = "fs-watch")]
    Ok(Some(FsWatchConfig { inbox }))
}

/// Rejects non-absolute paths, symlinks, non-directories, and directories that
/// cannot be listed. Deliberately does not canonicalize into a different
/// configured identity and does not expand shell syntax.
#[cfg_attr(not(feature = "fs-watch"), allow(dead_code))]
fn validate_inbox_root(inbox: &Path) -> Result<(), MemoryError> {
    if !inbox.is_absolute() {
        return Err(MemoryError::ConfigInvalid(format!(
            "{ENV_INGESTION_INBOX} must be an absolute directory path, got `{}`",
            inbox.display()
        )));
    }
    let metadata = inbox.symlink_metadata().map_err(|err| {
        MemoryError::ConfigInvalid(format!(
            "{ENV_INGESTION_INBOX} must point to an existing directory, got `{}`: {err}",
            inbox.display()
        ))
    })?;
    if metadata.file_type().is_symlink() {
        return Err(MemoryError::ConfigInvalid(format!(
            "{ENV_INGESTION_INBOX} must not be a symlink, got `{}`",
            inbox.display()
        )));
    }
    if !metadata.is_dir() {
        return Err(MemoryError::ConfigInvalid(format!(
            "{ENV_INGESTION_INBOX} must be a directory, got `{}`",
            inbox.display()
        )));
    }
    inbox.read_dir().map_err(|err| {
        MemoryError::ConfigInvalid(format!(
            "{ENV_INGESTION_INBOX} must be a readable directory, got `{}`: {err}",
            inbox.display()
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_inbox_disables_filesystem_watch() {
        let _guard = crate::config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe { std::env::remove_var(ENV_INGESTION_INBOX) };
        assert_eq!(from_env().expect("absent env is valid"), None);
    }

    #[cfg(feature = "fs-watch")]
    #[test]
    fn valid_absolute_directory_enables_filesystem_watch() {
        let _guard = crate::config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp inbox");
        unsafe { std::env::set_var(ENV_INGESTION_INBOX, dir.path()) };
        let config = from_env().expect("valid inbox").expect("enabled");
        assert_eq!(config.inbox, dir.path());
    }

    #[test]
    fn empty_inbox_is_rejected() {
        let _guard = crate::config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe { std::env::set_var(ENV_INGESTION_INBOX, "   ") };
        let err = from_env().expect_err("empty value must be rejected");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)));
        assert!(err.to_string().contains(ENV_INGESTION_INBOX));
        assert!(err.to_string().contains("non-empty"));
    }

    #[test]
    fn relative_inbox_is_rejected() {
        let _guard = crate::config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        unsafe { std::env::set_var(ENV_INGESTION_INBOX, "relative/path") };
        let err = from_env().expect_err("relative value must be rejected");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)));
        assert!(err.to_string().contains(ENV_INGESTION_INBOX));
        assert!(err.to_string().contains("absolute"));
    }

    #[test]
    fn missing_inbox_is_rejected() {
        let _guard = crate::config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp inbox");
        let missing = dir.path().join("does-not-exist");
        unsafe { std::env::set_var(ENV_INGESTION_INBOX, &missing) };
        let err = from_env().expect_err("missing value must be rejected");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)));
        assert!(err.to_string().contains(ENV_INGESTION_INBOX));
        assert!(err.to_string().contains("existing"));
    }

    #[test]
    fn file_inbox_is_rejected() {
        let _guard = crate::config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp inbox");
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "x").expect("write file");
        unsafe { std::env::set_var(ENV_INGESTION_INBOX, &file) };
        let err = from_env().expect_err("file value must be rejected");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)));
        assert!(err.to_string().contains(ENV_INGESTION_INBOX));
        assert!(err.to_string().contains("directory"));
    }

    #[test]
    fn symlink_root_inbox_is_rejected() {
        let _guard = crate::config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp inbox");
        let target = dir.path().join("target");
        std::fs::create_dir_all(&target).expect("create target dir");
        let link = dir.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).expect("create symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(&target, &link).expect("create symlink");
        unsafe { std::env::set_var(ENV_INGESTION_INBOX, &link) };
        let err = from_env().expect_err("symlink root must be rejected");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)));
        assert!(err.to_string().contains(ENV_INGESTION_INBOX));
        assert!(err.to_string().contains("symlink"));
    }

    #[cfg(not(feature = "fs-watch"))]
    #[test]
    fn configured_inbox_is_rejected_without_feature() {
        let _guard = crate::config::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = tempfile::tempdir().expect("temp inbox");
        unsafe { std::env::set_var(ENV_INGESTION_INBOX, dir.path()) };
        let err = from_env().expect_err("configured inbox without fs-watch must be rejected");
        assert!(matches!(err, MemoryError::ConfigInvalid(_)));
        assert!(err.to_string().contains("without the fs-watch feature"));
    }
}
